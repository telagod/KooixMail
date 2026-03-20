use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};

use crate::{
    auth::{authorize, header_value, now_ts},
    db::{delete_message, list_messages_by_mailbox, update_message_seen},
    inbound::ingest_inbound_message,
    models::{
        AppError, AppState, InboundMessageRequest, ListMessagesQuery, MessageDetailResponse,
        MessageSummaryResponse, UpdateMessageRequest,
    },
    routes::{broadcast_event, map_message_detail, map_message_summary, require_message},
};

const DEFAULT_PAGE_LIMIT: i64 = 50;
const MAX_PAGE_LIMIT: i64 = 200;

pub(super) async fn list_messages(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListMessagesQuery>,
) -> Result<Json<Vec<MessageSummaryResponse>>, AppError> {
    let mailbox = authorize(&state, &headers, None).await?;
    let limit = query.limit.unwrap_or(DEFAULT_PAGE_LIMIT).clamp(1, MAX_PAGE_LIMIT);
    let offset = query.offset.unwrap_or(0).max(0);
    let rows = list_messages_by_mailbox(&state.db, &mailbox.id, limit, offset).await?;
    let messages = rows.iter().map(map_message_summary).collect::<Vec<_>>();

    Ok(Json(messages))
}

pub(super) async fn get_message(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(message_id): Path<String>,
) -> Result<Json<MessageDetailResponse>, AppError> {
    let mailbox = authorize(&state, &headers, None).await?;
    let row = require_message(&state, &message_id, &mailbox.id).await?;
    Ok(Json(map_message_detail(row)))
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

    // Patch in-memory row instead of re-fetching
    row.seen = if payload.seen { 1 } else { 0 };
    row.updated_at = now;
    Ok(Json(map_message_detail(row)))
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
    Ok((StatusCode::CREATED, Json(map_message_detail(row))))
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
