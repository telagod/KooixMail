use std::{collections::HashMap, sync::Arc};

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use mail_auth::MessageAuthenticator;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use tokio::sync::{RwLock, broadcast};

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub config: AppConfig,
    pub events: Arc<RwLock<HashMap<String, broadcast::Sender<MailboxEvent>>>>,
    pub ingress_limits: Arc<RwLock<HashMap<String, Vec<i64>>>>,
    pub greylist: Arc<RwLock<HashMap<String, i64>>>,
    pub mail_auth: Option<Arc<MessageAuthenticator>>,
}

#[derive(Clone)]
pub struct AppConfig {
    pub domains: Vec<String>,
    pub ingress_token: Option<String>,
    pub smtp_bind_addr: Option<String>,
    pub smtp_hostname: String,
    pub smtp_tls_mode: SmtpTlsMode,
    pub smtp_tls_cert_path: Option<String>,
    pub smtp_tls_key_path: Option<String>,
    pub ingress_max_message_bytes: usize,
    pub ingress_rate_limit_per_minute: usize,
    pub ingress_require_spf: bool,
    pub ingress_require_dkim: bool,
    pub ingress_require_dmarc: bool,
    pub ingress_protect_local_domains: bool,
    pub ingress_greylist_enabled: bool,
    pub ingress_greylist_delay_secs: u64,
    pub ingress_rbl_zones: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SmtpTlsMode {
    #[default]
    Disabled,
    StartTls,
    RequireStartTls,
    ImplicitTls,
}

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Unauthorized(String),
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    TooManyRequests(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorBody {
    error: &'static str,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error) = match &self {
            Self::BadRequest(message) => (
                StatusCode::BAD_REQUEST,
                ErrorBody {
                    error: "bad_request",
                    message: message.clone(),
                },
            ),
            Self::Unauthorized(message) => (
                StatusCode::UNAUTHORIZED,
                ErrorBody {
                    error: "unauthorized",
                    message: message.clone(),
                },
            ),
            Self::Forbidden(message) => (
                StatusCode::FORBIDDEN,
                ErrorBody {
                    error: "forbidden",
                    message: message.clone(),
                },
            ),
            Self::NotFound(message) => (
                StatusCode::NOT_FOUND,
                ErrorBody {
                    error: "not_found",
                    message: message.clone(),
                },
            ),
            Self::Conflict(message) => (
                StatusCode::CONFLICT,
                ErrorBody {
                    error: "conflict",
                    message: message.clone(),
                },
            ),
            Self::TooManyRequests(message) => (
                StatusCode::TOO_MANY_REQUESTS,
                ErrorBody {
                    error: "too_many_requests",
                    message: message.clone(),
                },
            ),
            Self::Internal(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                ErrorBody {
                    error: "internal_error",
                    message: error.to_string(),
                },
            ),
        };

        (status, Json(error)).into_response()
    }
}

impl From<sqlx::Error> for AppError {
    fn from(value: sqlx::Error) -> Self {
        Self::Internal(anyhow::Error::new(value))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: &'static str,
    pub domains: Vec<String>,
    pub service: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DomainResponse {
    pub id: String,
    pub domain: String,
    pub is_verified: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxResponse {
    pub id: String,
    pub address: String,
    pub created_at: String,
    pub expires_at: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponse {
    pub mailbox: MailboxResponse,
    pub token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactResponse {
    pub name: String,
    pub address: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentResponse {
    pub id: String,
    pub filename: String,
    pub content_type: String,
    pub disposition: String,
    pub size: usize,
    pub download_url: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSummaryResponse {
    pub id: String,
    pub mailbox_id: String,
    pub from: ContactResponse,
    pub to: Vec<ContactResponse>,
    pub subject: String,
    pub intro: String,
    pub seen: bool,
    pub has_attachments: bool,
    pub size: usize,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDetailResponse {
    #[serde(flatten)]
    pub summary: MessageSummaryResponse,
    pub text: String,
    pub html: Vec<String>,
    pub attachments: Vec<AttachmentResponse>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxEvent {
    pub kind: String,
    pub mailbox_id: String,
    pub message_id: String,
    pub created_at: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateMailboxRequest {
    pub address: String,
    pub password: String,
    pub expires_in: Option<i64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    pub address: String,
    pub password: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMessageRequest {
    pub seen: bool,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InboundMessageRequest {
    pub to: String,
    pub from_name: Option<String>,
    pub from_address: String,
    pub subject: Option<String>,
    pub text: Option<String>,
    pub html: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventsQuery {
    pub mailbox_id: String,
    pub token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListMessagesQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

#[derive(FromRow)]
pub struct MailboxRow {
    pub id: String,
    pub address: String,
    pub password_hash: String,
    pub created_at: i64,
    pub expires_at: Option<i64>,
}

#[derive(FromRow)]
pub struct MessageRow {
    pub id: String,
    pub mailbox_id: String,
    pub to_address: String,
    pub from_name: String,
    pub from_address: String,
    pub subject: String,
    pub text_body: String,
    pub html_body: Option<String>,
    pub seen: i64,
    pub created_at: i64,
    pub updated_at: i64,
}
