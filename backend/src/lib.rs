use std::{collections::HashMap, env, net::SocketAddr, sync::Arc};

use anyhow::Context;
use axum::extract::DefaultBodyLimit;
use mail_auth::MessageAuthenticator;
use tokio::{net::TcpListener, sync::RwLock};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tracing::{info, warn};

pub mod auth;
pub mod db;
pub mod inbound;
pub mod models;
pub mod routes;
pub mod smtp;
#[cfg(test)]
pub(crate) mod test_support;

use crate::{
    db::{connect_db, migrate, spawn_cleanup_worker},
    models::{AppConfig, AppState, SmtpTlsMode},
    routes::build_router,
};

pub async fn run() -> anyhow::Result<()> {
    let (address, database_url, config) = load_runtime_config()?;
    let db = connect_db(&database_url).await?;
    migrate(&db).await?;
    let mail_auth = build_mail_authenticator(&config)?;

    let state = AppState {
        db,
        config,
        events: Arc::new(RwLock::new(HashMap::new())),
        ingress_limits: Arc::new(RwLock::new(HashMap::new())),
        greylist: Arc::new(RwLock::new(HashMap::new())),
        mail_auth,
    };

    spawn_cleanup_worker(state.clone());

    let cors = build_cors_layer();
    let app = build_router(state.clone())
        .layer(cors)
        .layer(DefaultBodyLimit::max(4 * 1024 * 1024)); // 4MB global limit
    let http_server = async move {
        info!(%address, "kooixmail backend listening");
        let listener = TcpListener::bind(address).await?;
        axum::serve(listener, app).await?;
        Ok::<(), anyhow::Error>(())
    };

    if state.config.smtp_bind_addr.is_some() {
        tokio::try_join!(http_server, smtp::serve_smtp(state))?;
    } else {
        http_server.await?;
    }

    Ok(())
}

fn load_runtime_config() -> anyhow::Result<(SocketAddr, String, AppConfig)> {
    let port = env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse::<u16>()
        .context("invalid PORT")?;
    let database_url =
        env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://kooixmail.db".to_string());
    let domains = env::var("KOOIXMAIL_DOMAINS")
        .unwrap_or_else(|_| "kooixmail.local,quack.local".to_string())
        .split(',')
        .map(str::trim)
        .filter(|domain| !domain.is_empty())
        .map(|domain| domain.to_lowercase())
        .collect::<Vec<_>>();
    let ingress_token = env::var("INGRESS_TOKEN")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let smtp_bind_addr =
        parse_optional_bind_addr(env::var("SMTP_BIND_ADDR").ok().as_deref(), "127.0.0.1:2525");
    let smtp_hostname = env::var("SMTP_HOSTNAME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            domains
                .first()
                .cloned()
                .unwrap_or_else(|| "localhost".to_string())
        });
    let smtp_tls_mode = parse_smtp_tls_mode(env::var("SMTP_TLS_MODE").ok().as_deref())?;
    let smtp_tls_cert_path = parse_optional_path(env::var("SMTP_TLS_CERT_PATH").ok().as_deref());
    let smtp_tls_key_path = parse_optional_path(env::var("SMTP_TLS_KEY_PATH").ok().as_deref());
    let ingress_max_message_bytes = parse_usize_env(
        "INGRESS_MAX_MESSAGE_BYTES",
        env::var("INGRESS_MAX_MESSAGE_BYTES").ok().as_deref(),
        262_144,
    )?;
    let ingress_rate_limit_per_minute = parse_usize_env(
        "INGRESS_RATE_LIMIT_PER_MINUTE",
        env::var("INGRESS_RATE_LIMIT_PER_MINUTE").ok().as_deref(),
        30,
    )?;
    let ingress_require_spf = parse_bool_env(
        "INGRESS_REQUIRE_SPF",
        env::var("INGRESS_REQUIRE_SPF").ok().as_deref(),
        false,
    )?;
    let ingress_require_dkim = parse_bool_env(
        "INGRESS_REQUIRE_DKIM",
        env::var("INGRESS_REQUIRE_DKIM").ok().as_deref(),
        false,
    )?;
    let ingress_require_dmarc = parse_bool_env(
        "INGRESS_REQUIRE_DMARC",
        env::var("INGRESS_REQUIRE_DMARC").ok().as_deref(),
        false,
    )?;
    let ingress_protect_local_domains = parse_bool_env(
        "INGRESS_PROTECT_LOCAL_DOMAINS",
        env::var("INGRESS_PROTECT_LOCAL_DOMAINS").ok().as_deref(),
        true,
    )?;
    let ingress_greylist_enabled = parse_bool_env(
        "INGRESS_GREYLIST_ENABLED",
        env::var("INGRESS_GREYLIST_ENABLED").ok().as_deref(),
        false,
    )?;
    let ingress_greylist_delay_secs = parse_usize_env(
        "INGRESS_GREYLIST_DELAY_SECS",
        env::var("INGRESS_GREYLIST_DELAY_SECS").ok().as_deref(),
        60,
    )? as u64;
    let ingress_rbl_zones = env::var("INGRESS_RBL_ZONES")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|zone| !zone.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let address = SocketAddr::from(([127, 0, 0, 1], port));

    Ok((
        address,
        database_url,
        AppConfig {
            domains,
            ingress_token,
            smtp_bind_addr,
            smtp_hostname,
            smtp_tls_mode,
            smtp_tls_cert_path,
            smtp_tls_key_path,
            ingress_max_message_bytes,
            ingress_rate_limit_per_minute,
            ingress_require_spf,
            ingress_require_dkim,
            ingress_require_dmarc,
            ingress_protect_local_domains,
            ingress_greylist_enabled,
            ingress_greylist_delay_secs,
            ingress_rbl_zones,
        },
    ))
}

fn parse_optional_bind_addr(raw: Option<&str>, default: &str) -> Option<String> {
    match raw.map(str::trim) {
        Some("") => None,
        Some(value) => Some(value.to_string()),
        None => Some(default.to_string()),
    }
}

fn parse_optional_path(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn parse_bool_env(name: &str, raw: Option<&str>, default: bool) -> anyhow::Result<bool> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        Some(value)
            if value.eq_ignore_ascii_case("true")
                || value.eq_ignore_ascii_case("1")
                || value.eq_ignore_ascii_case("yes")
                || value.eq_ignore_ascii_case("on") =>
        {
            Ok(true)
        }
        Some(value)
            if value.eq_ignore_ascii_case("false")
                || value.eq_ignore_ascii_case("0")
                || value.eq_ignore_ascii_case("no")
                || value.eq_ignore_ascii_case("off") =>
        {
            Ok(false)
        }
        Some(_) => Err(anyhow::anyhow!("invalid {name}")),
        None => Ok(default),
    }
}

fn parse_usize_env(name: &str, raw: Option<&str>, default: usize) -> anyhow::Result<usize> {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .parse::<usize>()
                .with_context(|| format!("invalid {name}"))
        })
        .transpose()
        .map(|value| value.unwrap_or(default))
}

fn parse_smtp_tls_mode(raw: Option<&str>) -> anyhow::Result<SmtpTlsMode> {
    match raw.map(str::trim).filter(|value| !value.is_empty()) {
        None => Ok(SmtpTlsMode::Disabled),
        Some(value) if value.eq_ignore_ascii_case("disabled") => Ok(SmtpTlsMode::Disabled),
        Some(value)
            if value.eq_ignore_ascii_case("starttls") || value.eq_ignore_ascii_case("explicit") =>
        {
            Ok(SmtpTlsMode::StartTls)
        }
        Some(value)
            if value.eq_ignore_ascii_case("require-starttls")
                || value.eq_ignore_ascii_case("required") =>
        {
            Ok(SmtpTlsMode::RequireStartTls)
        }
        Some(value)
            if value.eq_ignore_ascii_case("implicit") || value.eq_ignore_ascii_case("smtps") =>
        {
            Ok(SmtpTlsMode::ImplicitTls)
        }
        Some(_) => Err(anyhow::anyhow!("invalid SMTP_TLS_MODE")),
    }
}

fn build_mail_authenticator(
    config: &AppConfig,
) -> anyhow::Result<Option<Arc<MessageAuthenticator>>> {
    if !config.ingress_require_spf
        && !config.ingress_require_dkim
        && !config.ingress_require_dmarc
        && !config.ingress_protect_local_domains
    {
        return Ok(None);
    }

    match MessageAuthenticator::new_system_conf() {
        Ok(authenticator) => Ok(Some(Arc::new(authenticator))),
        Err(error)
            if !config.ingress_require_spf
                && !config.ingress_require_dkim
                && !config.ingress_require_dmarc =>
        {
            warn!(
                ?error,
                "mail authentication disabled because resolver initialization failed"
            );
            Ok(None)
        }
        Err(error) => {
            Err(anyhow::Error::new(error).context("failed to initialize mail authentication"))
        }
    }
}

fn build_cors_layer() -> CorsLayer {
    let raw = env::var("CORS_ALLOWED_ORIGINS").unwrap_or_default();
    let origins: Vec<&str> = raw.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();

    if origins.is_empty() {
        CorsLayer::permissive()
    } else {
        let parsed: Vec<_> = origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(AllowOrigin::list(parsed))
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any)
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_bool_env, parse_optional_bind_addr, parse_smtp_tls_mode, parse_usize_env};
    use crate::models::SmtpTlsMode;

    #[test]
    fn parse_optional_bind_addr_defaults_when_unset() {
        assert_eq!(
            parse_optional_bind_addr(None, "127.0.0.1:2525"),
            Some("127.0.0.1:2525".to_string())
        );
    }

    #[test]
    fn parse_optional_bind_addr_disables_listener_when_empty() {
        assert_eq!(parse_optional_bind_addr(Some(""), "127.0.0.1:2525"), None);
        assert_eq!(
            parse_optional_bind_addr(Some("   "), "127.0.0.1:2525"),
            None
        );
    }

    #[test]
    fn parse_optional_bind_addr_preserves_explicit_value() {
        assert_eq!(
            parse_optional_bind_addr(Some("0.0.0.0:25"), "127.0.0.1:2525"),
            Some("0.0.0.0:25".to_string())
        );
    }

    #[test]
    fn parse_smtp_tls_mode_supports_starttls_values() {
        assert_eq!(
            parse_smtp_tls_mode(Some("starttls")).unwrap(),
            SmtpTlsMode::StartTls
        );
        assert_eq!(
            parse_smtp_tls_mode(Some("require-starttls")).unwrap(),
            SmtpTlsMode::RequireStartTls
        );
        assert_eq!(
            parse_smtp_tls_mode(Some("implicit")).unwrap(),
            SmtpTlsMode::ImplicitTls
        );
    }

    #[test]
    fn parse_bool_and_usize_env_support_expected_values() {
        assert!(parse_bool_env("FLAG", Some("true"), false).unwrap());
        assert!(!parse_bool_env("FLAG", Some("0"), true).unwrap());
        assert_eq!(parse_usize_env("LIMIT", Some("42"), 1).unwrap(), 42);
    }
}
