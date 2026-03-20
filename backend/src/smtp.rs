use std::{io::BufReader, net::IpAddr};

use anyhow::Context;
use mail_auth::{
    AuthenticatedMessage, DkimResult, DmarcResult, MessageAuthenticator, SpfResult,
    dmarc::verify::DmarcParameters, spf::verify::SpfParameters,
};
use mail_parser::MessageParser;
use rustls_pemfile::{certs, private_key};
use smtpd::{
    Response as SmtpResponse, ServerConfig, Session, SmtpConfig, SmtpHandler, SmtpHandlerFactory,
    TlsConfig, TlsMode, start_server,
};
use tracing::{error, info, warn};

use crate::{
    auth::{ensure_allowed_domain, ensure_not_expired, normalize_address},
    db::find_mailbox_by_address,
    inbound::{
        AuthVerdict, InboundAuthReport, InboundContext, default_bounce_address,
        ingest_inbound_message_with_context, organizational_domain, sender_domain,
    },
    models::{AppConfig, AppError, AppState, InboundMessageRequest, SmtpTlsMode},
};

pub async fn serve_smtp(state: AppState) -> anyhow::Result<()> {
    let Some(bind_addr) = state.config.smtp_bind_addr.clone() else {
        return Ok(());
    };

    let config = build_smtp_config(&state.config)?;

    info!(bind_addr = %bind_addr, hostname = %config.hostname, "kooixmail smtp ingress listening");
    start_server(config, SmtpIngressFactory { state }).await?;
    Ok(())
}

struct SmtpIngressFactory {
    state: AppState,
}

struct SmtpIngressHandler {
    state: AppState,
}

impl SmtpHandlerFactory for SmtpIngressFactory {
    type Handler = SmtpIngressHandler;

    fn new_handler(&self, _session: &Session) -> Self::Handler {
        SmtpIngressHandler {
            state: self.state.clone(),
        }
    }
}

#[smtpd::async_trait]
impl SmtpHandler for SmtpIngressHandler {
    async fn handle_rcpt(
        &mut self,
        _session: &Session,
        to: &str,
    ) -> Result<SmtpResponse, smtpd::Error> {
        let recipient = normalize_address(to).map_err(map_recipient_error)?;
        ensure_allowed_domain(&self.state.config.domains, &recipient)
            .map_err(map_recipient_error)?;

        let mailbox = find_mailbox_by_address(&self.state.db, &recipient)
            .await
            .map_err(map_internal_error)?
            .ok_or_else(|| mailbox_unavailable("mailbox not found"))?;
        ensure_not_expired(&mailbox).map_err(map_recipient_error)?;

        Ok(SmtpResponse::Default)
    }

    async fn handle_email(
        &mut self,
        session: &Session,
        data: Vec<u8>,
    ) -> Result<SmtpResponse, smtpd::Error> {
        let parsed = MessageParser::default().parse(data.as_slice());
        if parsed.is_none() {
            warn!(
                remote_ip = %session.remote_ip,
                data_len = data.len(),
                "failed to parse RFC822 message, falling back to raw body"
            );
        }
        let authenticated = parsed
            .as_ref()
            .map(|message| AuthenticatedMessage::from_parsed(message, false))
            .or_else(|| AuthenticatedMessage::parse(data.as_slice()));
        let from_address = parsed
            .as_ref()
            .and_then(first_from_address)
            .or_else(|| {
                (!session.from.is_empty())
                    .then_some(session.from.as_str())
                    .and_then(|value| normalize_address(value).ok())
            })
            .unwrap_or_else(|| default_bounce_address(&self.state.config));
        let from_name = parsed
            .as_ref()
            .and_then(first_from_name)
            .map(ToOwned::to_owned)
            .or_else(|| {
                session
                    .from
                    .is_empty()
                    .then_some("Mail Delivery System".to_string())
            });
        let subject = parsed
            .as_ref()
            .and_then(|message| message.subject())
            .map(ToOwned::to_owned);
        let html = parsed
            .as_ref()
            .and_then(|message| message.body_html(0))
            .map(|body| body.into_owned());
        let text = parsed
            .as_ref()
            .and_then(|message| message.body_text(0))
            .map(|body| body.into_owned())
            .filter(|body| !body.trim().is_empty())
            .or_else(|| Some(fallback_text_body(data.as_slice())));
        let auth = evaluate_auth_report(
            self.state.mail_auth.as_deref(),
            &self.state.config,
            session,
            authenticated.as_ref(),
            &from_address,
        )
        .await;

        let mut delivered: usize = 0;
        let mut last_error: Option<smtpd::Error> = None;

        for recipient in &session.to {
            let payload = InboundMessageRequest {
                to: recipient.clone(),
                from_name: from_name.clone(),
                from_address: from_address.clone(),
                subject: subject.clone(),
                text: text.clone(),
                html: html.clone(),
            };
            let context = InboundContext {
                trusted: false,
                remote_ip: Some(session.remote_ip.clone()),
                helo_domain: (!session.remote_name.is_empty())
                    .then_some(session.remote_name.clone()),
                mail_from: (!session.from.is_empty()).then_some(session.from.clone()),
                raw_message_len: Some(data.len()),
                auth: auth.clone(),
            };

            match ingest_inbound_message_with_context(&self.state, payload, context).await {
                Ok(_) => delivered += 1,
                Err(error) => {
                    warn!(recipient = %recipient, ?error, "smtp ingress rejected message for recipient");
                    last_error = Some(map_ingest_error(error));
                }
            }
        }

        if delivered == 0 {
            return Err(last_error.unwrap_or_else(|| {
                map_internal_error("no recipients accepted")
            }));
        }

        Ok(SmtpResponse::ok(format!(
            "queued for {delivered}/{} recipient(s)",
            session.to.len()
        )))
    }
}

fn first_from_address(message: &mail_parser::Message<'_>) -> Option<String> {
    message
        .from()
        .and_then(|from| from.first())
        .and_then(|entry| entry.address())
        .and_then(|value| normalize_address(value).ok())
}

fn first_from_name<'a>(message: &'a mail_parser::Message<'_>) -> Option<&'a str> {
    message
        .from()
        .and_then(|from| from.first())
        .and_then(|entry| entry.name())
}

fn fallback_text_body(data: &[u8]) -> String {
    let raw = String::from_utf8_lossy(data);
    raw.split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .or_else(|| raw.split_once("\n\n").map(|(_, body)| body))
        .unwrap_or(raw.as_ref())
        .trim()
        .to_string()
}

fn build_smtp_config(config: &AppConfig) -> anyhow::Result<SmtpConfig> {
    let mut smtp_config = SmtpConfig {
        appname: "KooixMail".to_string(),
        hostname: config.smtp_hostname.clone(),
        bind_addr: config
            .smtp_bind_addr
            .clone()
            .unwrap_or_else(|| "127.0.0.1:2525".to_string()),
        disable_reverse_dns: true,
        max_message_size: Some(config.ingress_max_message_bytes),
        ..Default::default()
    };

    smtp_config.tls_mode = match config.smtp_tls_mode {
        SmtpTlsMode::Disabled => TlsMode::Disabled,
        SmtpTlsMode::StartTls => TlsMode::Explicit(load_tls_config(config)?),
        SmtpTlsMode::RequireStartTls => TlsMode::Required(load_tls_config(config)?),
        SmtpTlsMode::ImplicitTls => TlsMode::Implicit(load_tls_config(config)?),
    };

    if smtp_config.bind_addr.ends_with(":25")
        && matches!(config.smtp_tls_mode, SmtpTlsMode::Disabled)
    {
        anyhow::bail!(
            "refusing to start SMTP on port 25 without TLS; set SMTP_TLS_MODE=starttls or require-starttls"
        );
    }

    Ok(smtp_config)
}

fn load_tls_config(config: &AppConfig) -> anyhow::Result<TlsConfig> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let cert_path = config.smtp_tls_cert_path.as_deref().ok_or_else(|| {
        anyhow::anyhow!("SMTP_TLS_CERT_PATH is required when SMTP_TLS_MODE is enabled")
    })?;
    let key_path = config.smtp_tls_key_path.as_deref().ok_or_else(|| {
        anyhow::anyhow!("SMTP_TLS_KEY_PATH is required when SMTP_TLS_MODE is enabled")
    })?;

    let mut cert_reader = BufReader::new(std::fs::File::open(cert_path)?);
    let chain = certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .context("failed to read SMTP TLS certificate chain")?;
    let mut key_reader = BufReader::new(std::fs::File::open(key_path)?);
    let key = private_key(&mut key_reader)
        .context("failed to read SMTP TLS private key")?
        .ok_or_else(|| anyhow::anyhow!("SMTP TLS private key not found"))?;

    let server = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .context("failed to construct SMTP TLS server config")?;
    Ok(TlsConfig::Rustls(server))
}

async fn evaluate_auth_report(
    authenticator: Option<&MessageAuthenticator>,
    config: &AppConfig,
    session: &Session<'_>,
    authenticated: Option<&AuthenticatedMessage<'_>>,
    from_address: &str,
) -> Option<InboundAuthReport> {
    let authenticator = authenticator?;
    let authenticated = authenticated?;
    let ip = session.remote_ip.parse::<IpAddr>().ok()?;
    let helo_domain = if session.remote_name.trim().is_empty() {
        config.smtp_hostname.as_str()
    } else {
        session.remote_name.trim()
    };
    let mail_from = if session.from.trim().is_empty() {
        from_address
    } else {
        session.from.trim()
    };

    let dkim_output = authenticator.verify_dkim(authenticated).await;
    let spf_output = authenticator
        .verify_spf(SpfParameters::verify(
            ip,
            helo_domain,
            &config.smtp_hostname,
            mail_from,
        ))
        .await;
    let dmarc_output = authenticator
        .verify_dmarc(
            DmarcParameters::new(
                authenticated,
                &dkim_output,
                sender_domain(mail_from).unwrap_or(helo_domain),
                &spf_output,
            )
            .with_domain_suffix_fn(organizational_domain),
        )
        .await;

    Some(InboundAuthReport {
        spf: map_spf_verdict(spf_output.result()),
        dkim: map_dkim_verdict(&dkim_output),
        dmarc: map_dmarc_verdict(
            dmarc_output.dmarc_record().is_some(),
            dmarc_output.spf_result(),
            dmarc_output.dkim_result(),
        ),
        header_from_domain: authenticated
            .from
            .first()
            .and_then(|value| sender_domain(value))
            .map(ToOwned::to_owned),
    })
}

fn map_spf_verdict(result: SpfResult) -> AuthVerdict {
    match result {
        SpfResult::Pass => AuthVerdict::Pass,
        SpfResult::Fail => AuthVerdict::Fail,
        SpfResult::SoftFail => AuthVerdict::SoftFail,
        SpfResult::Neutral => AuthVerdict::Neutral,
        SpfResult::TempError => AuthVerdict::TempError,
        SpfResult::PermError => AuthVerdict::PermError,
        SpfResult::None => AuthVerdict::None,
    }
}

fn map_dkim_verdict(results: &[mail_auth::DkimOutput<'_>]) -> AuthVerdict {
    if results
        .iter()
        .any(|result| result.result() == &DkimResult::Pass)
    {
        AuthVerdict::Pass
    } else if results
        .iter()
        .any(|result| matches!(result.result(), DkimResult::TempError(_)))
    {
        AuthVerdict::TempError
    } else if results
        .iter()
        .any(|result| matches!(result.result(), DkimResult::PermError(_)))
    {
        AuthVerdict::PermError
    } else if results
        .iter()
        .any(|result| matches!(result.result(), DkimResult::Fail(_)))
    {
        AuthVerdict::Fail
    } else if results
        .iter()
        .any(|result| matches!(result.result(), DkimResult::Neutral(_)))
    {
        AuthVerdict::Neutral
    } else if results.is_empty() {
        AuthVerdict::Skipped
    } else {
        AuthVerdict::None
    }
}

fn map_dmarc_verdict(
    has_record: bool,
    spf_result: &DmarcResult,
    dkim_result: &DmarcResult,
) -> AuthVerdict {
    if matches!(spf_result, DmarcResult::Pass) || matches!(dkim_result, DmarcResult::Pass) {
        AuthVerdict::Pass
    } else if matches!(spf_result, DmarcResult::TempError(_))
        || matches!(dkim_result, DmarcResult::TempError(_))
    {
        AuthVerdict::TempError
    } else if matches!(spf_result, DmarcResult::PermError(_))
        || matches!(dkim_result, DmarcResult::PermError(_))
    {
        AuthVerdict::PermError
    } else if matches!(spf_result, DmarcResult::Fail(_))
        || matches!(dkim_result, DmarcResult::Fail(_))
    {
        AuthVerdict::Fail
    } else if has_record {
        AuthVerdict::None
    } else {
        AuthVerdict::Skipped
    }
}

fn mailbox_unavailable(message: impl Into<String>) -> smtpd::Error {
    smtpd::Error::from(SmtpResponse::new(550, message.into(), Some("5.1.1".into())))
}

fn map_recipient_error(error: AppError) -> smtpd::Error {
    match error {
        AppError::BadRequest(message) => {
            smtpd::Error::from(SmtpResponse::new(550, message, Some("5.1.3".into())))
        }
        AppError::Unauthorized(message) | AppError::Forbidden(message) => {
            smtpd::Error::from(SmtpResponse::new(550, message, Some("5.7.1".into())))
        }
        AppError::NotFound(message) => mailbox_unavailable(message),
        AppError::Conflict(message) => {
            smtpd::Error::from(SmtpResponse::new(550, message, Some("5.5.0".into())))
        }
        AppError::TooManyRequests(message) => {
            smtpd::Error::from(SmtpResponse::new(451, message, Some("4.7.1".into())))
        }
        AppError::Internal(error) => map_internal_error(error),
    }
}

fn map_ingest_error(error: AppError) -> smtpd::Error {
    match error {
        AppError::Internal(error) => map_internal_error(error),
        other => map_recipient_error(other),
    }
}

fn map_internal_error(error: impl std::fmt::Debug) -> smtpd::Error {
    error!(?error, "smtp ingress internal failure");
    smtpd::Error::from(SmtpResponse::new(
        451,
        "local error in processing",
        Some("4.3.0".into()),
    ))
}

#[cfg(test)]
mod tests {
    use smtpd::{SmtpConfig, SmtpHandler};
    use tokio::{
        io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
        net::TcpStream,
        task::JoinHandle,
    };

    use crate::{
        db::list_messages_by_mailbox,
        models::{AppConfig, AppState, SmtpTlsMode},
        test_support::{
            build_test_state, reserve_local_addr, seed_mailbox, wait_for_tcp, write_tls_fixture,
        },
    };

    use super::{SmtpIngressHandler, build_smtp_config, serve_smtp};

    fn build_session<'a>(config: &'a SmtpConfig) -> smtpd::Session<'a> {
        smtpd::Session::new(
            config,
            "127.0.0.1".to_string(),
            "localhost".to_string(),
            false,
        )
    }

    #[tokio::test]
    async fn handle_rcpt_accepts_existing_mailbox() {
        let state = build_test_state(&["kooixmail.local"]).await;
        seed_mailbox(&state, "smtpbox@kooixmail.local").await;
        let mut handler = SmtpIngressHandler { state };
        let config = SmtpConfig::default();
        let session = build_session(&config);

        let response = handler
            .handle_rcpt(&session, "smtpbox@kooixmail.local")
            .await
            .unwrap();

        assert!(response.is_default());
    }

    #[tokio::test]
    async fn handle_rcpt_rejects_unknown_mailbox() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let mut handler = SmtpIngressHandler { state };
        let config = SmtpConfig::default();
        let session = build_session(&config);

        let error = handler
            .handle_rcpt(&session, "missing@kooixmail.local")
            .await
            .unwrap_err();

        assert!(error.to_string().contains("550 5.1.1 mailbox not found"));
    }

    #[tokio::test]
    async fn handle_email_parses_rfc822_and_persists_message() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let mailbox = seed_mailbox(&state, "smtpbox@kooixmail.local").await;
        let mut handler = SmtpIngressHandler {
            state: state.clone(),
        };
        let config = SmtpConfig::default();
        let mut session = build_session(&config);
        session.from = "bounce@relay.test".to_string();
        session.to.push("smtpbox@kooixmail.local".to_string());
        session.got_from = true;

        let response = handler
            .handle_email(
                &session,
                concat!(
                    "From: Alice Example <a@sender.test>\r\n",
                    "To: smtpbox@kooixmail.local\r\n",
                    "Subject: SMTP test\r\n",
                    "\r\n",
                    "hello from smtp\r\n"
                )
                .as_bytes()
                .to_vec(),
            )
            .await
            .unwrap();

        assert!(response.to_string().contains("queued for 1/1 recipient"));

        let stored = list_messages_by_mailbox(&state.db, &mailbox.id, 100, 0)
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].subject, "SMTP test");
        assert_eq!(stored[0].from_name, "Alice Example");
        assert_eq!(stored[0].from_address, "a@sender.test");
        assert!(stored[0].text_body.contains("hello from smtp"));
    }

    #[tokio::test]
    async fn socket_smtp_dialogue_persists_message() {
        let mut state = build_test_state(&["kooixmail.local"]).await;
        let mailbox = seed_mailbox(&state, "socketbox@kooixmail.local").await;
        state.config.smtp_bind_addr = Some(reserve_local_addr());
        let bind_addr = state.config.smtp_bind_addr.clone().unwrap();
        let task = spawn_smtp(state.clone());
        wait_for_tcp(&bind_addr).await;

        let stream = TcpStream::connect(&bind_addr).await.unwrap();
        let (reader_half, mut writer_half) = stream.into_split();
        let mut reader = BufReader::new(reader_half);

        assert!(read_response(&mut reader).await.starts_with("220 "));
        write_command(&mut writer_half, "EHLO localhost\r\n").await;
        let ehlo = read_response(&mut reader).await;
        assert!(ehlo.contains("250-"));

        write_command(&mut writer_half, "MAIL FROM:<relay@sender.test>\r\n").await;
        assert!(read_response(&mut reader).await.starts_with("250 "));
        write_command(&mut writer_half, "RCPT TO:<socketbox@kooixmail.local>\r\n").await;
        assert!(read_response(&mut reader).await.starts_with("250 "));
        write_command(&mut writer_half, "DATA\r\n").await;
        assert!(read_response(&mut reader).await.starts_with("354 "));
        write_command(
            &mut writer_half,
            concat!(
                "From: Alice Example <relay@sender.test>\r\n",
                "To: socketbox@kooixmail.local\r\n",
                "Subject: socket smtp\r\n",
                "\r\n",
                "hello over tcp\r\n.\r\n"
            ),
        )
        .await;
        let queued = read_response(&mut reader).await;
        assert!(queued.contains("queued for 1/1 recipient"));

        let stored = list_messages_by_mailbox(&state.db, &mailbox.id, 100, 0)
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].subject, "socket smtp");
        assert!(stored[0].text_body.contains("hello over tcp"));

        task.abort();
    }

    #[tokio::test]
    async fn socket_starttls_dialogue_is_advertised() {
        let mut state = build_test_state(&["kooixmail.local"]).await;
        state.config.smtp_bind_addr = Some(reserve_local_addr());
        state.config.smtp_tls_mode = SmtpTlsMode::StartTls;
        let (cert_path, key_path) = write_tls_fixture();
        state.config.smtp_tls_cert_path = Some(cert_path.display().to_string());
        state.config.smtp_tls_key_path = Some(key_path.display().to_string());
        let bind_addr = state.config.smtp_bind_addr.clone().unwrap();
        let task = spawn_smtp(state);
        wait_for_tcp(&bind_addr).await;

        let stream = TcpStream::connect(&bind_addr).await.unwrap();
        let (reader_half, mut writer_half) = stream.into_split();
        let mut reader = BufReader::new(reader_half);

        assert!(read_response(&mut reader).await.starts_with("220 "));
        write_command(&mut writer_half, "EHLO localhost\r\n").await;
        let ehlo = read_response(&mut reader).await;
        assert!(ehlo.contains("250-STARTTLS"));
        write_command(&mut writer_half, "STARTTLS\r\n").await;
        assert!(
            read_response(&mut reader)
                .await
                .starts_with("220 Ready to start TLS")
        );

        task.abort();
    }

    #[test]
    fn build_smtp_config_enables_starttls_mode() {
        let (cert_path, key_path) = write_tls_fixture();
        let config = build_smtp_config(&AppConfig {
            domains: vec!["kooixmail.local".to_string()],
            ingress_token: None,
            smtp_bind_addr: Some("127.0.0.1:2525".to_string()),
            smtp_hostname: "mx.kooixmail.local".to_string(),
            smtp_tls_mode: SmtpTlsMode::RequireStartTls,
            smtp_tls_cert_path: Some(cert_path.display().to_string()),
            smtp_tls_key_path: Some(key_path.display().to_string()),
            ingress_max_message_bytes: 262_144,
            ingress_rate_limit_per_minute: 30,
            ingress_require_spf: false,
            ingress_require_dkim: false,
            ingress_require_dmarc: false,
            ingress_protect_local_domains: false,
            ingress_greylist_enabled: false,
            ingress_greylist_delay_secs: 60,
            ingress_rbl_zones: vec![],
        })
        .unwrap();
        assert!(config.tls_mode.has_tls());
        assert!(config.tls_mode.tls_mandatory());
    }

    fn spawn_smtp(state: AppState) -> JoinHandle<()> {
        tokio::spawn(async move {
            let _ = serve_smtp(state).await;
        })
    }

    async fn write_command(writer: &mut tokio::net::tcp::OwnedWriteHalf, data: &str) {
        writer.write_all(data.as_bytes()).await.unwrap();
        writer.flush().await.unwrap();
    }

    async fn read_response(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> String {
        let mut first = String::new();
        reader.read_line(&mut first).await.unwrap();
        let mut response = first.clone();

        if first.len() >= 4 && first.as_bytes()[3] == b'-' {
            let code = &first[..3];
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                response.push_str(&line);
                if line.starts_with(&format!("{code} ")) {
                    break;
                }
            }
        }

        response
    }

    #[test]
    fn build_smtp_config_rejects_port25_without_tls() {
        let result = build_smtp_config(&AppConfig {
            domains: vec!["kooixmail.local".to_string()],
            ingress_token: None,
            smtp_bind_addr: Some("0.0.0.0:25".to_string()),
            smtp_hostname: "mx.kooixmail.local".to_string(),
            smtp_tls_mode: SmtpTlsMode::Disabled,
            smtp_tls_cert_path: None,
            smtp_tls_key_path: None,
            ingress_max_message_bytes: 262_144,
            ingress_rate_limit_per_minute: 30,
            ingress_require_spf: false,
            ingress_require_dkim: false,
            ingress_require_dmarc: false,
            ingress_protect_local_domains: false,
            ingress_greylist_enabled: false,
            ingress_greylist_delay_secs: 60,
            ingress_rbl_zones: vec![],
        });
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("refusing to start SMTP on port 25 without TLS")
        );
    }

    #[tokio::test]
    async fn socket_starttls_full_handshake_and_delivery() {
        use rustls_pki_types::ServerName;
        use std::sync::Arc;
        use tokio_rustls::TlsConnector;

        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        let mut state = build_test_state(&["kooixmail.local"]).await;
        let mailbox = seed_mailbox(&state, "tlsbox@kooixmail.local").await;
        state.config.smtp_bind_addr = Some(reserve_local_addr());
        state.config.smtp_tls_mode = SmtpTlsMode::StartTls;
        let (cert_path, key_path) = write_tls_fixture();
        state.config.smtp_tls_cert_path = Some(cert_path.display().to_string());
        state.config.smtp_tls_key_path = Some(key_path.display().to_string());
        let bind_addr = state.config.smtp_bind_addr.clone().unwrap();
        let task = spawn_smtp(state.clone());
        wait_for_tcp(&bind_addr).await;

        // Phase 1: plaintext — EHLO + STARTTLS
        let stream = TcpStream::connect(&bind_addr).await.unwrap();
        let (reader_half, mut writer_half) = stream.into_split();
        let mut reader = BufReader::new(reader_half);

        assert!(read_response(&mut reader).await.starts_with("220 "));
        write_command(&mut writer_half, "EHLO localhost\r\n").await;
        let ehlo = read_response(&mut reader).await;
        assert!(ehlo.contains("250-STARTTLS"));

        write_command(&mut writer_half, "STARTTLS\r\n").await;
        let starttls_resp = read_response(&mut reader).await;
        assert!(starttls_resp.starts_with("220 Ready to start TLS"));

        // Phase 2: TLS upgrade
        let mut root_store = rustls::RootCertStore::empty();
        let cert_pem = std::fs::read(&cert_path).unwrap();
        let certs: Vec<_> = rustls_pemfile::certs(&mut &cert_pem[..])
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        for cert in &certs {
            root_store.add(cert.clone()).unwrap();
        }
        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();
        let connector = TlsConnector::from(Arc::new(tls_config));

        let tcp_stream = reader.into_inner().reunite(writer_half).unwrap();
        let server_name = ServerName::try_from("foobar.com").unwrap();
        let tls_stream = connector.connect(server_name, tcp_stream).await.unwrap();

        // Phase 3: encrypted SMTP dialogue
        let (tls_reader, mut tls_writer) = tokio::io::split(tls_stream);
        let mut tls_buf = BufReader::new(tls_reader);

        // Re-EHLO after TLS
        tls_writer
            .write_all(b"EHLO localhost\r\n")
            .await
            .unwrap();
        tls_writer.flush().await.unwrap();
        let ehlo2 = read_tls_response(&mut tls_buf).await;
        assert!(ehlo2.contains("250"));

        tls_writer
            .write_all(b"MAIL FROM:<secure@sender.test>\r\n")
            .await
            .unwrap();
        tls_writer.flush().await.unwrap();
        assert!(read_tls_response(&mut tls_buf).await.starts_with("250 "));

        tls_writer
            .write_all(b"RCPT TO:<tlsbox@kooixmail.local>\r\n")
            .await
            .unwrap();
        tls_writer.flush().await.unwrap();
        assert!(read_tls_response(&mut tls_buf).await.starts_with("250 "));

        tls_writer.write_all(b"DATA\r\n").await.unwrap();
        tls_writer.flush().await.unwrap();
        assert!(read_tls_response(&mut tls_buf).await.starts_with("354 "));

        tls_writer
            .write_all(
                concat!(
                    "From: Secure Sender <secure@sender.test>\r\n",
                    "To: tlsbox@kooixmail.local\r\n",
                    "Subject: TLS delivery\r\n",
                    "\r\n",
                    "encrypted hello\r\n.\r\n"
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        tls_writer.flush().await.unwrap();
        let queued = read_tls_response(&mut tls_buf).await;
        assert!(queued.contains("queued for 1/1 recipient"));

        // Verify DB
        let stored = crate::db::list_messages_by_mailbox(&state.db, &mailbox.id, 100, 0)
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].subject, "TLS delivery");
        assert!(stored[0].text_body.contains("encrypted hello"));

        task.abort();
    }

    async fn read_tls_response<R: tokio::io::AsyncRead + Unpin>(
        reader: &mut BufReader<R>,
    ) -> String {
        use tokio::io::AsyncBufReadExt;
        let mut first = String::new();
        reader.read_line(&mut first).await.unwrap();
        let mut response = first.clone();

        if first.len() >= 4 && first.as_bytes()[3] == b'-' {
            let code = &first[..3];
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                response.push_str(&line);
                if line.starts_with(&format!("{code} ")) {
                    break;
                }
            }
        }

        response
    }
}
