use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};

use crate::{
    auth::{authorize, header_value, now_ts},
    db::{
        delete_message, fetch_attachment, list_attachments_meta, list_messages_by_mailbox,
        update_message_seen,
    },
    inbound::ingest_inbound_message,
    models::{
        AppError, AppState, AttachmentResponse, InboundMessageRequest, ListMessagesQuery,
        MessageDetailResponse, MessageSummaryResponse, UpdateMessageRequest,
    },
    routes::{broadcast_event, map_message_detail, map_message_summary, require_message},
};

const DEFAULT_PAGE_LIMIT: i64 = 50;
const MAX_PAGE_LIMIT: i64 = 200;

#[derive(serde::Deserialize)]
pub(super) struct OptionalTokenQuery {
    pub token: Option<String>,
}

pub(super) async fn list_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListMessagesQuery>,
) -> Result<Json<Vec<MessageSummaryResponse>>, AppError> {
    let mailbox = authorize(&state, &headers, None).await?;
    let limit = query.limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT);
    let offset = query.offset.unwrap_or(0).max(0);
    let rows = list_messages_by_mailbox(&state.db, &mailbox.id, limit, offset).await?;

    let mut messages = Vec::with_capacity(rows.len());
    for row in &rows {
        let att_meta = list_attachments_meta(&state.db, &row.id)
            .await
            .unwrap_or_default();
        messages.push(map_message_summary(row, !att_meta.is_empty()));
    }

    Ok(Json(messages))
}

pub(super) async fn get_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<String>,
) -> Result<Json<MessageDetailResponse>, AppError> {
    let mailbox = authorize(&state, &headers, None).await?;
    let row = require_message(&state, &message_id, &mailbox.id).await?;
    let attachments = build_attachment_responses(&state, &message_id).await;
    Ok(Json(map_message_detail(row, attachments)))
}

pub(super) async fn update_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<String>,
    Json(payload): Json<UpdateMessageRequest>,
) -> Result<Json<MessageDetailResponse>, AppError> {
    let mailbox = authorize(&state, &headers, None).await?;
    let mut row = require_message(&state, &message_id, &mailbox.id).await?;

    let now = now_ts();
    update_message_seen(&state.db, &message_id, &mailbox.id, payload.seen, now).await?;
    broadcast_event(&state, &mailbox.id, &message_id, "message.updated").await;

    row.seen = if payload.seen { 1 } else { 0 };
    row.updated_at = now;
    let attachments = build_attachment_responses(&state, &message_id).await;
    Ok(Json(map_message_detail(row, attachments)))
}

pub(super) async fn delete_message_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let mailbox = authorize(&state, &headers, None).await?;
    require_message(&state, &message_id, &mailbox.id).await?;

    delete_message(&state.db, &message_id, &mailbox.id).await?;
    broadcast_event(&state, &mailbox.id, &message_id, "message.deleted").await;
    Ok(StatusCode::NO_CONTENT)
}

pub(super) async fn deliver_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(payload): Json<InboundMessageRequest>,
) -> Result<(StatusCode, Json<MessageDetailResponse>), AppError> {
    if let Some(required_token) = &state.config.ingress_token {
        let provided = header_value(&headers, "x-ingress-token");
        let matches = provided
            .as_deref()
            .map(|p| constant_time_eq(p.as_bytes(), required_token.as_bytes()))
            .unwrap_or(false);
        if !matches {
            return Err(AppError::Unauthorized("missing ingress token".to_string()));
        }
    }

    let row = ingest_inbound_message(&state, payload).await?;
    let attachments = build_attachment_responses(&state, &row.id).await;
    Ok((StatusCode::CREATED, Json(map_message_detail(row, attachments))))
}

pub(super) async fn download_attachment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((message_id, attachment_id)): Path<(String, String)>,
    Query(query): Query<OptionalTokenQuery>,
) -> Result<Response, AppError> {
    let mailbox = authorize(&state, &headers, query.token.as_deref()).await?;
    require_message(&state, &message_id, &mailbox.id).await?;

    let row = fetch_attachment(&state.db, &attachment_id, &message_id)
        .await
        .map_err(AppError::Internal)?
        .ok_or_else(|| AppError::NotFound("attachment not found".to_string()))?;

    let disposition = format!(
        "{}; filename=\"{}\"",
        row.disposition,
        row.filename.replace('"', "\\\"")
    );

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &row.content_type)
        .header(header::CONTENT_DISPOSITION, disposition)
        .header(header::CONTENT_LENGTH, row.data.len())
        .body(Body::from(row.data))
        .unwrap()
        .into_response())
}

async fn build_attachment_responses(state: &AppState, message_id: &str) -> Vec<AttachmentResponse> {
    let meta = list_attachments_meta(&state.db, message_id)
        .await
        .unwrap_or_default();
    meta.into_iter()
        .map(|m| AttachmentResponse {
            id: m.id.clone(),
            filename: m.filename,
            content_type: m.content_type,
            disposition: m.disposition,
            size: m.size as usize,
            download_url: format!(
                "/api/v1/messages/{}/attachments/{}",
                message_id, m.id
            ),
        })
        .collect()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter()
        .zip(b.iter())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}
