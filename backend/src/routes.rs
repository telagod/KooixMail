mod events;
mod mailboxes;
mod messages;
mod system;

use axum::{
    Router,
    routing::{delete, get, post},
};

use crate::{
    auth::{now_ts, ts_to_rfc3339},
    db::fetch_message,
    models::{
        AppError, AppState, ContactResponse, MailboxEvent, MailboxResponse, MailboxRow,
        MessageDetailResponse, MessageRow, MessageSummaryResponse,
    },
};

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(system::healthz))
        .route("/api/v1/domains", get(system::list_domains))
        .route("/api/v1/mailboxes", post(mailboxes::create_mailbox))
        .route(
            "/api/v1/mailboxes/{id}",
            delete(mailboxes::delete_mailbox_handler),
        )
        .route("/api/v1/sessions", post(mailboxes::create_session))
        .route("/api/v1/me", get(mailboxes::current_mailbox))
        .route("/api/v1/messages", get(messages::list_messages))
        .route(
            "/api/v1/messages/{id}",
            get(messages::get_message)
                .patch(messages::update_message)
                .delete(messages::delete_message_handler),
        )
        .route("/api/v1/events", get(events::stream_events))
        .route("/api/v1/inbound/messages", post(messages::deliver_message))
        .with_state(state)
}

pub(super) async fn require_message(
    state: &AppState,
    message_id: &str,
    mailbox_id: &str,
) -> Result<MessageRow, AppError> {
    fetch_message(&state.db, message_id, mailbox_id)
        .await?
        .ok_or_else(|| AppError::NotFound("message not found".to_string()))
}

pub(crate) async fn broadcast_event(
    state: &AppState,
    mailbox_id: &str,
    message_id: &str,
    kind: &str,
) {
    let sender = event_sender(state, mailbox_id).await;
    let _ = sender.send(MailboxEvent {
        kind: kind.to_string(),
        mailbox_id: mailbox_id.to_string(),
        message_id: message_id.to_string(),
        created_at: ts_to_rfc3339(now_ts()),
    });
}

pub(super) async fn event_sender(
    state: &AppState,
    mailbox_id: &str,
) -> tokio::sync::broadcast::Sender<MailboxEvent> {
    // Fast path: read lock
    {
        let events = state.events.read().await;
        if let Some(sender) = events.get(mailbox_id) {
            return sender.clone();
        }
    }
    // Slow path: write lock, insert if still missing
    let mut events = state.events.write().await;
    events
        .entry(mailbox_id.to_string())
        .or_insert_with(|| {
            let (sender, _) = tokio::sync::broadcast::channel(32);
            sender
        })
        .clone()
}

pub(super) fn map_mailbox(row: MailboxRow) -> MailboxResponse {
    MailboxResponse {
        id: row.id,
        address: row.address,
        created_at: ts_to_rfc3339(row.created_at),
        expires_at: row.expires_at.map(ts_to_rfc3339),
    }
}

pub(super) fn map_message_summary(row: &MessageRow) -> MessageSummaryResponse {
    let size = row.text_body.len() + row.html_body.as_deref().unwrap_or_default().len();
    MessageSummaryResponse {
        id: row.id.clone(),
        mailbox_id: row.mailbox_id.clone(),
        from: ContactResponse {
            name: row.from_name.clone(),
            address: row.from_address.clone(),
        },
        to: vec![ContactResponse {
            name: row.to_address.clone(),
            address: row.to_address.clone(),
        }],
        subject: row.subject.clone(),
        intro: intro(&row.text_body),
        seen: row.seen == 1,
        has_attachments: false,
        size,
        created_at: ts_to_rfc3339(row.created_at),
        updated_at: ts_to_rfc3339(row.updated_at),
    }
}

pub(super) fn map_message_detail(row: MessageRow) -> MessageDetailResponse {
    MessageDetailResponse {
        summary: map_message_summary(&row),
        text: row.text_body,
        html: row.html_body.into_iter().collect(),
        attachments: Vec::new(),
    }
}

fn intro(text: &str) -> String {
    let trimmed = text.replace('\n', " ").trim().to_string();
    if trimmed.chars().count() <= 120 {
        trimmed
    } else {
        format!("{}...", trimmed.chars().take(117).collect::<String>())
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;
    use serde_json::{json, Value};
    use uuid::Uuid;

    use crate::auth::{hash_password, now_ts};
    use crate::db::{insert_mailbox, insert_message, insert_session};
    use crate::test_support::build_test_state;
    use super::build_router;

    async fn body_json(response: axum::http::Response<Body>) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// Inserts a mailbox + session into the DB and returns (mailbox_id, token).
    async fn setup_authed_mailbox(
        state: &crate::models::AppState,
        address: &str,
        password: &str,
    ) -> (String, String) {
        let mailbox_id = Uuid::new_v4().to_string();
        let token = Uuid::new_v4().to_string();
        let hash = hash_password(password).unwrap();
        let now = now_ts();
        insert_mailbox(&state.db, &mailbox_id, address, &hash, now, None)
            .await
            .unwrap();
        insert_session(&state.db, &token, &mailbox_id, now)
            .await
            .unwrap();
        (mailbox_id, token)
    }

    // ── 1. healthz ──────────────────────────────────────────────

    #[tokio::test]
    async fn healthz_returns_ok() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let app = build_router(state);
        let resp = app
            .oneshot(Request::get("/healthz").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["status"], "ok");
    }

    // ── 2. list_domains ─────────────────────────────────────────

    #[tokio::test]
    async fn list_domains_returns_configured_domains() {
        let state = build_test_state(&["kooixmail.local", "quack.local"]).await;
        let app = build_router(state);
        let resp = app
            .oneshot(Request::get("/api/v1/domains").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        let domains: Vec<String> = json
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["domain"].as_str().unwrap().to_string())
            .collect();
        assert!(domains.contains(&"kooixmail.local".to_string()));
        assert!(domains.contains(&"quack.local".to_string()));
    }

    // ── 3. create_mailbox_success ───────────────────────────────

    #[tokio::test]
    async fn create_mailbox_success() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/v1/mailboxes")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "address": "test@kooixmail.local",
                            "password": "secret123",
                            "expiresIn": 3600
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["mailbox"]["address"], "test@kooixmail.local");
        assert!(json["token"].as_str().is_some_and(|t| !t.is_empty()));
    }

    // ── 4. create_mailbox_rejects_short_password ────────────────

    #[tokio::test]
    async fn create_mailbox_rejects_short_password() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/v1/mailboxes")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "address": "test@kooixmail.local",
                            "password": "12345"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── 5. create_mailbox_rejects_invalid_domain ────────────────

    #[tokio::test]
    async fn create_mailbox_rejects_invalid_domain() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/v1/mailboxes")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "address": "test@evil.com",
                            "password": "secret123"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── 6. create_mailbox_rejects_duplicate_address ─────────────

    #[tokio::test]
    async fn create_mailbox_rejects_duplicate_address() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let payload = serde_json::to_string(&json!({
            "address": "dup@kooixmail.local",
            "password": "secret123"
        }))
        .unwrap();

        // First creation succeeds
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::post("/api/v1/mailboxes")
                    .header("content-type", "application/json")
                    .body(Body::from(payload.clone()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // Second creation conflicts
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/v1/mailboxes")
                    .header("content-type", "application/json")
                    .body(Body::from(payload))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    // ── 7. create_session_success ───────────────────────────────

    #[tokio::test]
    async fn create_session_success() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let password = "secret123";
        let hash = hash_password(password).unwrap();
        let mailbox_id = Uuid::new_v4().to_string();
        insert_mailbox(&state.db, &mailbox_id, "sess@kooixmail.local", &hash, now_ts(), None)
            .await
            .unwrap();

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "address": "sess@kooixmail.local",
                            "password": "secret123"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert!(json["token"].as_str().is_some_and(|t| !t.is_empty()));
    }

    // ── 8. create_session_wrong_password ────────────────────────

    #[tokio::test]
    async fn create_session_wrong_password() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let hash = hash_password("correct-password").unwrap();
        let mailbox_id = Uuid::new_v4().to_string();
        insert_mailbox(&state.db, &mailbox_id, "wrong@kooixmail.local", &hash, now_ts(), None)
            .await
            .unwrap();

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "address": "wrong@kooixmail.local",
                            "password": "bad-password"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── 9. create_session_nonexistent_mailbox ───────────────────

    #[tokio::test]
    async fn create_session_nonexistent_mailbox() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/v1/sessions")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "address": "ghost@kooixmail.local",
                            "password": "secret123"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── 10. get_current_mailbox_success ─────────────────────────

    #[tokio::test]
    async fn get_current_mailbox_success() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let (_, token) = setup_authed_mailbox(&state, "me@kooixmail.local", "secret123").await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/v1/me")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["address"], "me@kooixmail.local");
    }

    // ── 11. get_current_mailbox_no_auth ─────────────────────────

    #[tokio::test]
    async fn get_current_mailbox_no_auth() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let app = build_router(state);
        let resp = app
            .oneshot(Request::get("/api/v1/me").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    // ── 12. delete_mailbox_success ──────────────────────────────

    #[tokio::test]
    async fn delete_mailbox_success() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let (mailbox_id, token) =
            setup_authed_mailbox(&state, "del@kooixmail.local", "secret123").await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::delete(format!("/api/v1/mailboxes/{mailbox_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // ── 13. delete_mailbox_wrong_id ─────────────────────────────

    #[tokio::test]
    async fn delete_mailbox_wrong_id() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let (_, token) =
            setup_authed_mailbox(&state, "own@kooixmail.local", "secret123").await;
        let other_id = Uuid::new_v4().to_string();

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::delete(format!("/api/v1/mailboxes/{other_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    // ── 14. list_messages_empty ─────────────────────────────────

    #[tokio::test]
    async fn list_messages_empty() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let (_, token) =
            setup_authed_mailbox(&state, "empty@kooixmail.local", "secret123").await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/v1/messages")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json.as_array().unwrap().len(), 0);
    }

    // ── 15. list_messages_with_data ─────────────────────────────

    #[tokio::test]
    async fn list_messages_with_data() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let (mailbox_id, token) =
            setup_authed_mailbox(&state, "msgs@kooixmail.local", "secret123").await;

        let msg_id = Uuid::new_v4().to_string();
        insert_message(
            &state.db, &msg_id, &mailbox_id, "msgs@kooixmail.local",
            "Sender", "sender@test.com", "Hello", "body text", None, now_ts(),
        )
        .await
        .unwrap();

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/v1/messages")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        let arr = json.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["subject"], "Hello");
    }

    // ── 16. get_message_success ─────────────────────────────────

    #[tokio::test]
    async fn get_message_success() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let (mailbox_id, token) =
            setup_authed_mailbox(&state, "getm@kooixmail.local", "secret123").await;

        let msg_id = Uuid::new_v4().to_string();
        insert_message(
            &state.db, &msg_id, &mailbox_id, "getm@kooixmail.local",
            "Sender", "sender@test.com", "Detail", "detail body", None, now_ts(),
        )
        .await
        .unwrap();

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get(format!("/api/v1/messages/{msg_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["subject"], "Detail");
        assert_eq!(json["text"], "detail body");
    }

    // ── 17. get_message_not_found ───────────────────────────────

    #[tokio::test]
    async fn get_message_not_found() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let (_, token) =
            setup_authed_mailbox(&state, "nfm@kooixmail.local", "secret123").await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/v1/messages/nonexistent")
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── 18. update_message_seen ─────────────────────────────────

    #[tokio::test]
    async fn update_message_seen() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let (mailbox_id, token) =
            setup_authed_mailbox(&state, "seen@kooixmail.local", "secret123").await;

        let msg_id = Uuid::new_v4().to_string();
        insert_message(
            &state.db, &msg_id, &mailbox_id, "seen@kooixmail.local",
            "Sender", "sender@test.com", "Mark", "mark body", None, now_ts(),
        )
        .await
        .unwrap();

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::builder()
                    .method("PATCH")
                    .uri(format!("/api/v1/messages/{msg_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({"seen": true})).unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let json = body_json(resp).await;
        assert_eq!(json["seen"], true);
    }

    // ── 19. delete_message_success ──────────────────────────────

    #[tokio::test]
    async fn delete_message_success() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let (mailbox_id, token) =
            setup_authed_mailbox(&state, "delmsg@kooixmail.local", "secret123").await;

        let msg_id = Uuid::new_v4().to_string();
        insert_message(
            &state.db, &msg_id, &mailbox_id, "delmsg@kooixmail.local",
            "Sender", "sender@test.com", "Gone", "gone body", None, now_ts(),
        )
        .await
        .unwrap();

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::delete(format!("/api/v1/messages/{msg_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // ── 20. deliver_message_success ─────────────────────────────

    #[tokio::test]
    async fn deliver_message_success() {
        let state = build_test_state(&["kooixmail.local"]).await;
        // Need a mailbox for the recipient to exist
        setup_authed_mailbox(&state, "inbox@kooixmail.local", "secret123").await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/v1/inbound/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "to": "inbox@kooixmail.local",
                            "fromAddress": "external@sender.test",
                            "subject": "Inbound",
                            "text": "inbound body"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // ── 21. deliver_message_requires_ingress_token ──────────────

    #[tokio::test]
    async fn deliver_message_requires_ingress_token() {
        let mut state = build_test_state(&["kooixmail.local"]).await;
        state.config.ingress_token = Some("super-secret".to_string());
        setup_authed_mailbox(&state, "inbox@kooixmail.local", "secret123").await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/v1/inbound/messages")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::to_string(&json!({
                            "to": "inbox@kooixmail.local",
                            "fromAddress": "external@sender.test",
                            "subject": "Blocked",
                            "text": "should fail"
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
