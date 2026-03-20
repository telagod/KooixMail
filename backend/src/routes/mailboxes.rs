use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use uuid::Uuid;

use crate::{
    auth::{
        authorize, calculate_expiry, ensure_allowed_domain, ensure_not_expired, hash_password,
        normalize_address, now_ts, ts_to_rfc3339, validate_mailbox_address, validate_password,
        verify_password_hash,
    },
    db::{delete_mailbox, find_mailbox_by_address, insert_mailbox, insert_session},
    models::{
        AppError, AppState, AuthResponse, CreateMailboxRequest, CreateSessionRequest,
        MailboxResponse,
    },
    routes::map_mailbox,
};

pub(super) async fn create_mailbox(
    State(state): State<AppState>,
    Json(payload): Json<CreateMailboxRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    let address = normalize_address(&payload.address)?;
    validate_mailbox_address(&address)?;
    validate_password(&payload.password)?;
    ensure_allowed_domain(&state.config.domains, &address)?;

    let expires_at = calculate_expiry(payload.expires_in)?;
    let password_hash = hash_password(&payload.password)?;
    let mailbox_id = Uuid::new_v4().to_string();
    let session_token = Uuid::new_v4().to_string();
    let now = now_ts();

    if let Err(error) = insert_mailbox(
        &state.db,
        &mailbox_id,
        &address,
        &password_hash,
        now,
        expires_at,
    )
    .await
    {
        if is_unique_violation(&error) {
            return Err(AppError::Conflict(
                "mailbox address already exists".to_string(),
            ));
        }
        return Err(error.into());
    }

    insert_session(&state.db, &session_token, &mailbox_id, now).await?;

    Ok(Json(AuthResponse {
        mailbox: MailboxResponse {
            id: mailbox_id,
            address,
            created_at: ts_to_rfc3339(now),
            expires_at: expires_at.map(ts_to_rfc3339),
        },
        token: session_token,
    }))
}

pub(super) async fn create_session(
    State(state): State<AppState>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<AuthResponse>, AppError> {
    let address = normalize_address(&payload.address)?;
    let mailbox = find_mailbox_by_address(&state.db, &address)
        .await?
        .ok_or_else(|| AppError::NotFound("mailbox not found".to_string()))?;
    ensure_not_expired(&mailbox)?;
    verify_password_hash(&payload.password, &mailbox.password_hash)?;

    let token = Uuid::new_v4().to_string();
    insert_session(&state.db, &token, &mailbox.id, now_ts()).await?;

    Ok(Json(AuthResponse {
        mailbox: map_mailbox(mailbox),
        token,
    }))
}

pub(super) async fn current_mailbox(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MailboxResponse>, AppError> {
    let mailbox = authorize(&state, &headers, None).await?;
    Ok(Json(map_mailbox(mailbox)))
}

pub(super) async fn delete_mailbox_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(mailbox_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mailbox = authorize(&state, &headers, None).await?;
    if mailbox.id != mailbox_id {
        return Err(AppError::Forbidden(
            "cannot delete another mailbox".to_string(),
        ));
    }

    delete_mailbox(&state.db, &mailbox.id).await?;
    {
        let mut events = state.events.write().await;
        events.remove(&mailbox.id);
    }
    Ok(StatusCode::NO_CONTENT)
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Database(db_error) => db_error
            .code()
            .is_some_and(|code| code == "2067" || code == "1555"),
        _ => false,
    }
}
