use std::net::IpAddr;

use tokio::net::lookup_host;
use tracing::warn;
use uuid::Uuid;

use crate::{
    auth::{ensure_allowed_domain, ensure_not_expired, normalize_address, now_ts},
    db::{find_mailbox_by_address, insert_attachment, insert_message},
    models::{AppConfig, AppError, AppState, InboundAttachment, InboundMessageRequest, MessageRow},
    routes::broadcast_event,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AuthVerdict {
    Pass,
    Fail,
    SoftFail,
    Neutral,
    None,
    TempError,
    PermError,
    #[default]
    Skipped,
}

#[derive(Debug, Clone, Default)]
pub struct InboundAuthReport {
    pub spf: AuthVerdict,
    pub dkim: AuthVerdict,
    pub dmarc: AuthVerdict,
    pub header_from_domain: Option<String>,
}

impl InboundAuthReport {
    pub fn any_pass(&self) -> bool {
        self.spf == AuthVerdict::Pass
            || self.dkim == AuthVerdict::Pass
            || self.dmarc == AuthVerdict::Pass
    }
}

#[derive(Debug, Clone, Default)]
pub struct InboundContext {
    pub trusted: bool,
    pub remote_ip: Option<String>,
    pub helo_domain: Option<String>,
    pub mail_from: Option<String>,
    pub raw_message_len: Option<usize>,
    pub auth: Option<InboundAuthReport>,
}

impl InboundContext {
    pub fn trusted_http() -> Self {
        Self {
            trusted: true,
            ..Self::default()
        }
    }
}

pub async fn ingest_inbound_message(
    state: &AppState,
    payload: InboundMessageRequest,
) -> Result<MessageRow, AppError> {
    ingest_inbound_message_with_context(state, payload, InboundContext::trusted_http(), vec![])
        .await
}

pub async fn ingest_inbound_message_with_context(
    state: &AppState,
    payload: InboundMessageRequest,
    context: InboundContext,
    attachments: Vec<InboundAttachment>,
) -> Result<MessageRow, AppError> {
    let recipient = normalize_address(&payload.to)?;
    ensure_allowed_domain(&state.config.domains, &recipient)?;

    let sender = normalize_address(&payload.from_address)?;
    enforce_ingress_policies(state, &payload, &sender, &context).await?;

    let mailbox = find_mailbox_by_address(&state.db, &recipient)
        .await?
        .ok_or_else(|| AppError::NotFound("mailbox not found".to_string()))?;
    ensure_not_expired(&mailbox)?;

    let message_id = Uuid::new_v4().to_string();
    let now = now_ts();
    let text_body = payload.text.unwrap_or_default();
    let subject = payload
        .subject
        .unwrap_or_else(|| "(no subject)".to_string());
    let from_name = payload
        .from_name
        .unwrap_or_else(|| "Unknown sender".to_string());

    insert_message(
        &state.db,
        &message_id,
        &mailbox.id,
        &mailbox.address,
        &from_name,
        &sender,
        &subject,
        &text_body,
        payload.html.as_deref(),
        now,
    )
    .await?;

    for attachment in &attachments {
        let att_id = Uuid::new_v4().to_string();
        insert_attachment(
            &state.db,
            &att_id,
            &message_id,
            &attachment.filename,
            &attachment.content_type,
            &attachment.disposition,
            attachment.data.len() as i64,
            &attachment.data,
        )
        .await
        .map_err(AppError::Internal)?;
    }

    broadcast_event(state, &mailbox.id, &message_id, "message.created").await;

    Ok(MessageRow {
        id: message_id,
        mailbox_id: mailbox.id,
        to_address: mailbox.address,
        from_name,
        from_address: sender,
        subject,
        text_body,
        html_body: payload.html,
        seen: 0,
        created_at: now,
        updated_at: now,
    })
}

async fn enforce_ingress_policies(
    state: &AppState,
    payload: &InboundMessageRequest,
    sender: &str,
    context: &InboundContext,
) -> Result<(), AppError> {
    let message_size = context
        .raw_message_len
        .unwrap_or_else(|| estimate_message_size(payload));
    if message_size > state.config.ingress_max_message_bytes {
        return Err(AppError::BadRequest(format!(
            "message exceeds ingress size limit of {} bytes",
            state.config.ingress_max_message_bytes
        )));
    }

    if context.trusted {
        return Ok(());
    }

    enforce_rbl(state, context).await?;
    enforce_rate_limit(state, context, sender).await?;
    enforce_sender_authentication(state, sender, context)?;
    enforce_greylist(state, context, sender, &payload.to).await?;
    Ok(())
}

async fn enforce_rate_limit(
    state: &AppState,
    context: &InboundContext,
    sender: &str,
) -> Result<(), AppError> {
    if state.config.ingress_rate_limit_per_minute == 0 {
        return Ok(());
    }

    let key = context
        .remote_ip
        .as_deref()
        .map(|ip| format!("ip:{ip}"))
        .unwrap_or_else(|| format!("sender:{sender}"));
    let window_start = now_ts() - 60;

    let mut limits = state.ingress_limits.write().await;
    let hits = limits.entry(key).or_default();
    hits.retain(|timestamp| *timestamp > window_start);
    if hits.len() >= state.config.ingress_rate_limit_per_minute {
        return Err(AppError::TooManyRequests(
            "ingress rate limit exceeded".to_string(),
        ));
    }
    hits.push(now_ts());
    Ok(())
}

fn enforce_sender_authentication(
    state: &AppState,
    sender: &str,
    context: &InboundContext,
) -> Result<(), AppError> {
    let auth = context.auth.as_ref();

    if state.config.ingress_require_spf && !has_pass(auth, |report| report.spf) {
        return Err(AppError::Forbidden(
            "spf validation required for smtp ingress".to_string(),
        ));
    }
    if state.config.ingress_require_dkim && !has_pass(auth, |report| report.dkim) {
        return Err(AppError::Forbidden(
            "dkim validation required for smtp ingress".to_string(),
        ));
    }
    if state.config.ingress_require_dmarc && !has_pass(auth, |report| report.dmarc) {
        return Err(AppError::Forbidden(
            "dmarc validation required for smtp ingress".to_string(),
        ));
    }

    let protected_domain = auth
        .and_then(|report| report.header_from_domain.as_deref())
        .or_else(|| sender_domain(sender));
    if state.config.ingress_protect_local_domains
        && protected_domain
            .is_some_and(|domain| state.config.domains.iter().any(|item| item == domain))
        && !auth.is_some_and(InboundAuthReport::any_pass)
    {
        return Err(AppError::Forbidden(
            "local sender domain requires spf, dkim, or dmarc pass".to_string(),
        ));
    }

    Ok(())
}

async fn enforce_rbl(state: &AppState, context: &InboundContext) -> Result<(), AppError> {
    if state.config.ingress_rbl_zones.is_empty() {
        return Ok(());
    }
    let ip = match context.remote_ip.as_deref().and_then(|s| s.parse::<IpAddr>().ok()) {
        Some(IpAddr::V4(v4)) => v4,
        _ => return Ok(()),
    };

    let octets = ip.octets();
    let reversed = format!("{}.{}.{}.{}", octets[3], octets[2], octets[1], octets[0]);

    for zone in &state.config.ingress_rbl_zones {
        let query = format!("{reversed}.{zone}:0");
        match lookup_host(&query).await {
            Ok(mut addrs) => {
                if addrs.any(|addr| {
                    matches!(addr.ip(), IpAddr::V4(v4) if v4.octets()[0] == 127)
                }) {
                    warn!(ip = %ip, zone = %zone, "rbl hit — rejecting");
                    return Err(AppError::Forbidden(format!(
                        "rejected: {ip} listed in {zone}"
                    )));
                }
            }
            Err(_) => { /* NXDOMAIN = not listed, continue */ }
        }
    }
    Ok(())
}

async fn enforce_greylist(
    state: &AppState,
    context: &InboundContext,
    sender: &str,
    recipient: &str,
) -> Result<(), AppError> {
    if !state.config.ingress_greylist_enabled {
        return Ok(());
    }

    let ip = context.remote_ip.as_deref().unwrap_or("unknown");
    let key = format!("grey:{ip}:{sender}:{recipient}");
    let now = now_ts();
    let delay = state.config.ingress_greylist_delay_secs as i64;

    let mut greylist = state.greylist.write().await;
    match greylist.get(&key).copied() {
        Some(first_seen) if now - first_seen >= delay => {
            // 已过延迟期，放行
            Ok(())
        }
        Some(_) => {
            // 仍在延迟期内，临时拒绝
            Err(AppError::TooManyRequests(
                "greylisted, please retry later".to_string(),
            ))
        }
        None => {
            greylist.insert(key, now);
            Err(AppError::TooManyRequests(
                "greylisted, please retry later".to_string(),
            ))
        }
    }
}

fn has_pass(
    auth: Option<&InboundAuthReport>,
    selector: impl FnOnce(&InboundAuthReport) -> AuthVerdict,
) -> bool {
    auth.map(selector)
        .is_some_and(|verdict| verdict == AuthVerdict::Pass)
}

fn estimate_message_size(payload: &InboundMessageRequest) -> usize {
    payload.to.len()
        + payload.from_address.len()
        + payload.from_name.as_deref().unwrap_or_default().len()
        + payload.subject.as_deref().unwrap_or_default().len()
        + payload.text.as_deref().unwrap_or_default().len()
        + payload.html.as_deref().unwrap_or_default().len()
}

pub fn sender_domain(address: &str) -> Option<&str> {
    address.rsplit_once('@').map(|(_, domain)| domain)
}

pub fn organizational_domain(domain: &str) -> &str {
    let mut dots = domain.rmatch_indices('.');
    let Some(_) = dots.next() else {
        return domain;
    };
    let Some((penultimate_dot, _)) = dots.next() else {
        return domain;
    };
    &domain[penultimate_dot + 1..]
}

pub fn default_bounce_address(config: &AppConfig) -> String {
    let domain = config
        .domains
        .first()
        .cloned()
        .unwrap_or_else(|| config.smtp_hostname.clone());
    format!("mailer-daemon@{domain}")
}

#[cfg(test)]
mod tests {
    use crate::{
        db::list_messages_by_mailbox,
        models::InboundMessageRequest,
        test_support::{build_test_state, seed_mailbox},
    };

    use super::{
        AuthVerdict, InboundAuthReport, InboundContext, default_bounce_address,
        ingest_inbound_message, ingest_inbound_message_with_context,
    };

    #[tokio::test]
    async fn ingest_inbound_message_persists_message_for_existing_mailbox() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let mailbox = seed_mailbox(&state, "inbox@kooixmail.local").await;

        let row = ingest_inbound_message(
            &state,
            InboundMessageRequest {
                to: "inbox@kooixmail.local".to_string(),
                from_name: Some("Sender".to_string()),
                from_address: "a@sender.test".to_string(),
                subject: Some("hello".to_string()),
                text: Some("body".to_string()),
                html: None,
            },
        )
        .await
        .unwrap();

        assert_eq!(row.mailbox_id, mailbox.id);
        assert_eq!(row.subject, "hello");
        assert_eq!(row.from_address, "a@sender.test");

        let stored = list_messages_by_mailbox(&state.db, &mailbox.id, 100, 0)
            .await
            .unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].text_body, "body");
    }

    #[tokio::test]
    async fn ingest_inbound_message_rejects_unknown_domain() {
        let state = build_test_state(&["kooixmail.local"]).await;

        let result = ingest_inbound_message(
            &state,
            InboundMessageRequest {
                to: "inbox@example.com".to_string(),
                from_name: None,
                from_address: "a@sender.test".to_string(),
                subject: None,
                text: Some("body".to_string()),
                html: None,
            },
        )
        .await;

        match result {
            Err(error) => assert!(
                error
                    .to_string()
                    .contains("domain example.com is not enabled")
            ),
            Ok(_) => panic!("expected inbound ingestion to reject disabled domain"),
        }
    }

    #[test]
    fn default_bounce_address_prefers_first_domain() {
        let address = default_bounce_address(&crate::models::AppConfig {
            domains: vec!["kooixmail.local".to_string(), "quack.local".to_string()],
            ingress_token: None,
            smtp_bind_addr: None,
            smtp_hostname: "mx.kooixmail.local".to_string(),
            smtp_tls_mode: crate::models::SmtpTlsMode::Disabled,
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

        assert_eq!(address, "mailer-daemon@kooixmail.local");
    }

    #[tokio::test]
    async fn ingress_policy_rejects_rate_limit_burst() {
        let mut state = build_test_state(&["kooixmail.local"]).await;
        state.config.ingress_rate_limit_per_minute = 1;
        seed_mailbox(&state, "inbox@kooixmail.local").await;

        ingest_inbound_message_with_context(
            &state,
            InboundMessageRequest {
                to: "inbox@kooixmail.local".to_string(),
                from_name: None,
                from_address: "a@sender.test".to_string(),
                subject: None,
                text: Some("one".to_string()),
                html: None,
            },
            InboundContext {
                trusted: false,
                remote_ip: Some("203.0.113.10".to_string()),
                ..InboundContext::default()
            },
            vec![],
        )
        .await
        .unwrap();

        let result = ingest_inbound_message_with_context(
            &state,
            InboundMessageRequest {
                to: "inbox@kooixmail.local".to_string(),
                from_name: None,
                from_address: "a@sender.test".to_string(),
                subject: None,
                text: Some("two".to_string()),
                html: None,
            },
            InboundContext {
                trusted: false,
                remote_ip: Some("203.0.113.10".to_string()),
                ..InboundContext::default()
            },
            vec![],
        )
        .await;

        assert!(matches!(
            result,
            Err(crate::models::AppError::TooManyRequests(_))
        ));
    }

    #[tokio::test]
    async fn ingress_policy_rejects_local_domain_spoof_without_auth_pass() {
        let mut state = build_test_state(&["kooixmail.local"]).await;
        state.config.ingress_protect_local_domains = true;
        seed_mailbox(&state, "inbox@kooixmail.local").await;

        let result = ingest_inbound_message_with_context(
            &state,
            InboundMessageRequest {
                to: "inbox@kooixmail.local".to_string(),
                from_name: Some("Duck Impostor".to_string()),
                from_address: "spoof@kooixmail.local".to_string(),
                subject: None,
                text: Some("forged".to_string()),
                html: None,
            },
            InboundContext {
                trusted: false,
                remote_ip: Some("203.0.113.10".to_string()),
                auth: Some(InboundAuthReport {
                    spf: AuthVerdict::Fail,
                    dkim: AuthVerdict::Fail,
                    dmarc: AuthVerdict::Fail,
                    header_from_domain: Some("kooixmail.local".to_string()),
                }),
                ..InboundContext::default()
            },
            vec![],
        )
        .await;

        assert!(matches!(result, Err(crate::models::AppError::Forbidden(_))));
    }

    #[tokio::test]
    async fn greylist_rejects_first_attempt_then_allows_after_delay() {
        let mut state = build_test_state(&["kooixmail.local"]).await;
        state.config.ingress_greylist_enabled = true;
        state.config.ingress_greylist_delay_secs = 1;
        seed_mailbox(&state, "inbox@kooixmail.local").await;

        let payload = InboundMessageRequest {
            to: "inbox@kooixmail.local".to_string(),
            from_name: None,
            from_address: "a@sender.test".to_string(),
            subject: Some("grey".to_string()),
            text: Some("body".to_string()),
            html: None,
        };
        let context = InboundContext {
            trusted: false,
            remote_ip: Some("198.51.100.1".to_string()),
            ..InboundContext::default()
        };

        // 首次 — 应被 greylist 拒绝
        let result =
            ingest_inbound_message_with_context(&state, payload.clone(), context.clone(), vec![]).await;
        assert!(
            matches!(result, Err(crate::models::AppError::TooManyRequests(_))),
            "first attempt should be greylisted"
        );

        // 立即重试 — 仍在延迟期内，应被拒绝
        let result =
            ingest_inbound_message_with_context(&state, payload.clone(), context.clone(), vec![]).await;
        assert!(
            matches!(result, Err(crate::models::AppError::TooManyRequests(_))),
            "immediate retry should still be greylisted"
        );

        // 等待延迟期过后 — 应放行
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let result =
            ingest_inbound_message_with_context(&state, payload, context, vec![]).await;
        assert!(result.is_ok(), "should pass after greylist delay");
    }

    #[tokio::test]
    async fn trusted_http_skips_rate_limit_policy() {
        let mut state = build_test_state(&["kooixmail.local"]).await;
        // 设置极低的 rate limit，trusted 路径应完全绕过
        state.config.ingress_rate_limit_per_minute = 1;
        let _ = seed_mailbox(&state, "rate@kooixmail.local").await;

        let payload = InboundMessageRequest {
            to: "rate@kooixmail.local".to_string(),
            from_name: Some("Tester".to_string()),
            from_address: "sender@outer.net".to_string(),
            subject: Some("Rate test".to_string()),
            text: Some("body".to_string()),
            html: None,
        };

        for i in 0..5 {
            let result = ingest_inbound_message(&state, payload.clone()).await;
            assert!(result.is_ok(), "trusted delivery #{i} should succeed");
        }
    }

    #[tokio::test]
    async fn ingest_rejects_oversized_message() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let _ = seed_mailbox(&state, "big@kooixmail.local").await;

        // 超过默认 ingress_max_message_bytes (262144)
        let huge_text = "x".repeat(300_000);
        let payload = InboundMessageRequest {
            to: "big@kooixmail.local".to_string(),
            from_name: Some("Tester".to_string()),
            from_address: "sender@outer.net".to_string(),
            subject: Some("Huge".to_string()),
            text: Some(huge_text),
            html: None,
        };

        let result = ingest_inbound_message(&state, payload).await;
        assert!(
            matches!(result, Err(crate::models::AppError::BadRequest(_))),
            "oversized message should be rejected with BadRequest"
        );
    }
}
