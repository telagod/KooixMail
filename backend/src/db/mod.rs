mod attachments;
mod mailboxes;
mod messages;
mod runtime;
mod sessions;

pub use attachments::{fetch_attachment, insert_attachment, list_attachments_meta};
pub use mailboxes::{delete_mailbox, find_mailbox_by_address, insert_mailbox};
pub use messages::{
    delete_message, fetch_message, insert_message, list_messages_by_mailbox, update_message_seen,
};
pub use runtime::{
    cleanup_old_events, connect_db, flush_greylist, flush_ingress_limits, insert_event,
    load_greylist, load_ingress_limits, migrate, poll_events_since, spawn_cleanup_worker,
};
pub use sessions::{find_mailbox_by_session_token, insert_session};

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::{
        auth::now_ts,
        db::{
            cleanup_old_events, delete_mailbox, delete_message, fetch_attachment, fetch_message,
            find_mailbox_by_address, find_mailbox_by_session_token, flush_greylist,
            flush_ingress_limits, insert_attachment, insert_mailbox, insert_message, insert_session,
            list_attachments_meta, list_messages_by_mailbox, load_greylist, load_ingress_limits,
            poll_events_since, update_message_seen,
        },
        db::runtime::{cleanup_expired_mailboxes, insert_event},
        test_support::build_test_state,
    };

    fn uid() -> String {
        Uuid::new_v4().to_string()
    }

    async fn seed_mailbox_raw(
        db: &sqlx::SqlitePool,
        address: &str,
    ) -> String {
        let id = uid();
        let now = now_ts();
        insert_mailbox(db, &id, address, "hash", now, None)
            .await
            .unwrap();
        id
    }

    async fn seed_message_raw(
        db: &sqlx::SqlitePool,
        mailbox_id: &str,
        created_at: i64,
    ) -> String {
        let id = uid();
        insert_message(
            db, &id, mailbox_id,
            "to@example.com", "Sender", "from@example.com",
            "Subject", "body text", None, created_at,
        )
        .await
        .unwrap();
        id
    }

    // 1. insert_and_find_mailbox
    #[tokio::test]
    async fn insert_and_find_mailbox() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let id = uid();
        let now = now_ts();
        let address = format!("user-{}@example.com", uid());

        insert_mailbox(db, &id, &address, "myhash", now, Some(now + 3600))
            .await
            .unwrap();

        let row = find_mailbox_by_address(db, &address)
            .await
            .unwrap()
            .expect("should find mailbox");

        assert_eq!(row.id, id);
        assert_eq!(row.address, address);
        assert_eq!(row.password_hash, "myhash");
        assert_eq!(row.created_at, now);
        assert_eq!(row.expires_at, Some(now + 3600));
    }

    // 2. find_mailbox_returns_none_for_unknown
    #[tokio::test]
    async fn find_mailbox_returns_none_for_unknown() {
        let state = build_test_state(&["example.com"]).await;
        let result = find_mailbox_by_address(&state.db, "nonexistent@x.com")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    // 3. delete_mailbox_removes_record
    #[tokio::test]
    async fn delete_mailbox_removes_record() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("del-{}@example.com", uid());
        let mailbox_id = seed_mailbox_raw(db, &address).await;

        delete_mailbox(db, &mailbox_id).await.unwrap();

        let result = find_mailbox_by_address(db, &address).await.unwrap();
        assert!(result.is_none());
    }

    // 4. delete_mailbox_cascades_sessions
    #[tokio::test]
    async fn delete_mailbox_cascades_sessions() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("cas-sess-{}@example.com", uid());
        let mailbox_id = seed_mailbox_raw(db, &address).await;
        let token = uid();
        insert_session(db, &token, &mailbox_id, now_ts()).await.unwrap();

        delete_mailbox(db, &mailbox_id).await.unwrap();

        let result = find_mailbox_by_session_token(db, &token).await.unwrap();
        assert!(result.is_none());
    }

    // 5. delete_mailbox_cascades_messages
    #[tokio::test]
    async fn delete_mailbox_cascades_messages() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("cas-msg-{}@example.com", uid());
        let mailbox_id = seed_mailbox_raw(db, &address).await;
        let msg_id = seed_message_raw(db, &mailbox_id, now_ts()).await;

        delete_mailbox(db, &mailbox_id).await.unwrap();

        let result = fetch_message(db, &msg_id, &mailbox_id).await.unwrap();
        assert!(result.is_none());
    }

    // 6. insert_mailbox_rejects_duplicate_address
    #[tokio::test]
    async fn insert_mailbox_rejects_duplicate_address() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("dup-{}@example.com", uid());
        let now = now_ts();

        insert_mailbox(db, &uid(), &address, "hash", now, None)
            .await
            .unwrap();

        let result = insert_mailbox(db, &uid(), &address, "hash2", now, None).await;
        assert!(result.is_err());
    }

    // 7. insert_and_find_session
    #[tokio::test]
    async fn insert_and_find_session() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("sess-{}@example.com", uid());
        let mailbox_id = seed_mailbox_raw(db, &address).await;
        let token = uid();

        insert_session(db, &token, &mailbox_id, now_ts()).await.unwrap();

        let row = find_mailbox_by_session_token(db, &token)
            .await
            .unwrap()
            .expect("should find mailbox via session");

        assert_eq!(row.id, mailbox_id);
        assert_eq!(row.address, address);
    }

    // 8. find_session_returns_none_for_unknown
    #[tokio::test]
    async fn find_session_returns_none_for_unknown() {
        let state = build_test_state(&["example.com"]).await;
        let result = find_mailbox_by_session_token(&state.db, "bad-token")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    // 9. insert_and_list_messages
    #[tokio::test]
    async fn insert_and_list_messages() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("list-{}@example.com", uid());
        let mailbox_id = seed_mailbox_raw(db, &address).await;
        let now = now_ts();

        seed_message_raw(db, &mailbox_id, now).await;
        seed_message_raw(db, &mailbox_id, now + 1).await;
        seed_message_raw(db, &mailbox_id, now + 2).await;

        let messages = list_messages_by_mailbox(db, &mailbox_id, 50, 0)
            .await
            .unwrap();
        assert_eq!(messages.len(), 3);
    }

    // 10. list_messages_respects_limit_and_offset
    #[tokio::test]
    async fn list_messages_respects_limit_and_offset() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("page-{}@example.com", uid());
        let mailbox_id = seed_mailbox_raw(db, &address).await;
        let now = now_ts();

        for i in 0..5 {
            seed_message_raw(db, &mailbox_id, now + i).await;
        }

        let page1 = list_messages_by_mailbox(db, &mailbox_id, 2, 0).await.unwrap();
        assert_eq!(page1.len(), 2);

        let page2 = list_messages_by_mailbox(db, &mailbox_id, 2, 2).await.unwrap();
        assert_eq!(page2.len(), 2);
    }

    // 11. list_messages_ordered_by_created_at_desc
    #[tokio::test]
    async fn list_messages_ordered_by_created_at_desc() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("order-{}@example.com", uid());
        let mailbox_id = seed_mailbox_raw(db, &address).await;
        let now = now_ts();

        seed_message_raw(db, &mailbox_id, now + 10).await;
        seed_message_raw(db, &mailbox_id, now + 30).await;
        seed_message_raw(db, &mailbox_id, now + 20).await;

        let messages = list_messages_by_mailbox(db, &mailbox_id, 50, 0)
            .await
            .unwrap();

        assert_eq!(messages.len(), 3);
        assert!(messages[0].created_at >= messages[1].created_at);
        assert!(messages[1].created_at >= messages[2].created_at);
    }

    // 12. fetch_message_success
    #[tokio::test]
    async fn fetch_message_success() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("fetch-{}@example.com", uid());
        let mailbox_id = seed_mailbox_raw(db, &address).await;
        let now = now_ts();
        let msg_id = uid();

        insert_message(
            db, &msg_id, &mailbox_id,
            "to@example.com", "Alice", "alice@example.com",
            "Hello", "text body", Some("<p>html</p>"), now,
        )
        .await
        .unwrap();

        let row = fetch_message(db, &msg_id, &mailbox_id)
            .await
            .unwrap()
            .expect("should fetch message");

        assert_eq!(row.id, msg_id);
        assert_eq!(row.mailbox_id, mailbox_id);
        assert_eq!(row.from_name, "Alice");
        assert_eq!(row.from_address, "alice@example.com");
        assert_eq!(row.subject, "Hello");
        assert_eq!(row.text_body, "text body");
        assert_eq!(row.html_body, Some("<p>html</p>".to_string()));
        assert_eq!(row.created_at, now);
    }

    // 13. fetch_message_wrong_mailbox
    #[tokio::test]
    async fn fetch_message_wrong_mailbox() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;

        let addr_a = format!("mba-{}@example.com", uid());
        let mailbox_a = seed_mailbox_raw(db, &addr_a).await;
        let addr_b = format!("mbb-{}@example.com", uid());
        let mailbox_b = seed_mailbox_raw(db, &addr_b).await;

        let msg_id = seed_message_raw(db, &mailbox_a, now_ts()).await;

        let result = fetch_message(db, &msg_id, &mailbox_b).await.unwrap();
        assert!(result.is_none());
    }

    // 14. update_message_seen
    #[tokio::test]
    async fn test_update_message_seen() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("seen-{}@example.com", uid());
        let mailbox_id = seed_mailbox_raw(db, &address).await;
        let now = now_ts();
        let msg_id = seed_message_raw(db, &mailbox_id, now).await;

        let row = fetch_message(db, &msg_id, &mailbox_id).await.unwrap().unwrap();
        assert_eq!(row.seen, 0);

        update_message_seen(db, &msg_id, &mailbox_id, true, now + 1)
            .await
            .unwrap();

        let updated = fetch_message(db, &msg_id, &mailbox_id).await.unwrap().unwrap();
        assert_eq!(updated.seen, 1);
    }

    // 15. delete_message_success
    #[tokio::test]
    async fn delete_message_success() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;
        let address = format!("delmsg-{}@example.com", uid());
        let mailbox_id = seed_mailbox_raw(db, &address).await;
        let msg_id = seed_message_raw(db, &mailbox_id, now_ts()).await;

        delete_message(db, &msg_id, &mailbox_id).await.unwrap();

        let result = fetch_message(db, &msg_id, &mailbox_id).await.unwrap();
        assert!(result.is_none());
    }

    // 16. delete_message_wrong_mailbox
    #[tokio::test]
    async fn delete_message_wrong_mailbox() {
        let state = build_test_state(&["example.com"]).await;
        let db = &state.db;

        let addr_a = format!("dwa-{}@example.com", uid());
        let mailbox_a = seed_mailbox_raw(db, &addr_a).await;
        let addr_b = format!("dwb-{}@example.com", uid());
        let mailbox_b = seed_mailbox_raw(db, &addr_b).await;

        let msg_id = seed_message_raw(db, &mailbox_a, now_ts()).await;

        delete_message(db, &msg_id, &mailbox_b).await.unwrap();

        let result = fetch_message(db, &msg_id, &mailbox_a).await.unwrap();
        assert!(result.is_some(), "message should still exist in mailbox_a");
    }

    #[tokio::test]
    async fn cleanup_removes_expired_mailboxes() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let now = crate::auth::now_ts();

        // 创建一个已过期的邮箱
        let expired_id = Uuid::new_v4().to_string();
        insert_mailbox(&state.db, &expired_id, "expired@kooixmail.local", "hash", now - 1000, Some(now - 500)).await.unwrap();

        // 创建一个未过期的邮箱
        let valid_id = Uuid::new_v4().to_string();
        insert_mailbox(&state.db, &valid_id, "valid@kooixmail.local", "hash", now, Some(now + 86400)).await.unwrap();

        // 创建一个永不过期的邮箱
        let forever_id = Uuid::new_v4().to_string();
        insert_mailbox(&state.db, &forever_id, "forever@kooixmail.local", "hash", now, None).await.unwrap();

        // 执行清理
        cleanup_expired_mailboxes(&state.db).await.unwrap();

        // 已过期的应该被删除
        assert!(find_mailbox_by_address(&state.db, "expired@kooixmail.local").await.unwrap().is_none());
        // 未过期的应该保留
        assert!(find_mailbox_by_address(&state.db, "valid@kooixmail.local").await.unwrap().is_some());
        // 永不过期的应该保留
        assert!(find_mailbox_by_address(&state.db, "forever@kooixmail.local").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn cleanup_cascades_sessions_and_messages() {
        let state = build_test_state(&["kooixmail.local"]).await;
        let now = crate::auth::now_ts();

        // 创建已过期邮箱
        let mailbox_id = Uuid::new_v4().to_string();
        insert_mailbox(&state.db, &mailbox_id, "cascade@kooixmail.local", "hash", now - 1000, Some(now - 500)).await.unwrap();

        // 给它加 session 和 message
        insert_session(&state.db, "cleanup-token", &mailbox_id, now).await.unwrap();
        let msg_id = Uuid::new_v4().to_string();
        insert_message(&state.db, &msg_id, &mailbox_id, "cascade@kooixmail.local", "Sender", "s@x.com", "Test", "body", None, now).await.unwrap();

        // 执行清理
        cleanup_expired_mailboxes(&state.db).await.unwrap();

        // 邮箱、session、message 都应该被级联删除
        assert!(find_mailbox_by_address(&state.db, "cascade@kooixmail.local").await.unwrap().is_none());
        assert!(find_mailbox_by_session_token(&state.db, "cleanup-token").await.unwrap().is_none());
        assert!(fetch_message(&state.db, &msg_id, &mailbox_id).await.unwrap().is_none());
    }

    // ── Phase 1: ingress_limits persistence ─────────────

    #[tokio::test]
    async fn load_ingress_limits_from_db() {
        let state = build_test_state(&["example.com"]).await;
        // Seed DB directly
        sqlx::query("INSERT INTO ingress_limits (key, hit_at) VALUES (?, ?)")
            .bind("ip:1.2.3.4")
            .bind(1000i64)
            .execute(&state.db)
            .await
            .unwrap();
        sqlx::query("INSERT INTO ingress_limits (key, hit_at) VALUES (?, ?)")
            .bind("ip:1.2.3.4")
            .bind(2000i64)
            .execute(&state.db)
            .await
            .unwrap();

        let loaded = load_ingress_limits(&state.db).await.unwrap();
        assert_eq!(loaded.get("ip:1.2.3.4").unwrap().len(), 2);
    }

    #[tokio::test]
    async fn flush_and_load_ingress_limits_roundtrip() {
        let state = build_test_state(&["example.com"]).await;
        {
            let mut limits = state.ingress_limits.write().await;
            limits.insert("ip:10.0.0.1".to_string(), vec![100, 200, 300]);
            limits.insert("sender:a@b.com".to_string(), vec![400]);
        }

        flush_ingress_limits(&state).await.unwrap();
        let loaded = load_ingress_limits(&state.db).await.unwrap();
        assert_eq!(loaded.get("ip:10.0.0.1").unwrap().len(), 3);
        assert_eq!(loaded.get("sender:a@b.com").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn flush_and_load_greylist_roundtrip() {
        let state = build_test_state(&["example.com"]).await;
        {
            let mut greylist = state.greylist.write().await;
            greylist.insert("grey:1.2.3.4:a@b:c@d".to_string(), 12345);
            greylist.insert("grey:5.6.7.8:x@y:z@w".to_string(), 67890);
        }

        flush_greylist(&state).await.unwrap();
        let loaded = load_greylist(&state.db).await.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(*loaded.get("grey:1.2.3.4:a@b:c@d").unwrap(), 12345);
    }

    // ── Phase 2: attachments ────────────────────────────

    #[tokio::test]
    async fn insert_and_list_and_fetch_attachment() {
        let state = build_test_state(&["example.com"]).await;
        let mailbox_id = seed_mailbox_raw(&state.db, "att@example.com").await;
        let msg_id = seed_message_raw(&state.db, &mailbox_id, now_ts()).await;

        insert_attachment(
            &state.db, "att-1", &msg_id, "file.txt", "text/plain", "attachment", 5, b"hello",
        )
        .await
        .unwrap();

        let meta = list_attachments_meta(&state.db, &msg_id).await.unwrap();
        assert_eq!(meta.len(), 1);
        assert_eq!(meta[0].filename, "file.txt");
        assert_eq!(meta[0].size, 5);

        let full = fetch_attachment(&state.db, "att-1", &msg_id).await.unwrap().unwrap();
        assert_eq!(full.data, b"hello");
        assert_eq!(full.content_type, "text/plain");
    }

    #[tokio::test]
    async fn attachment_cascade_on_message_delete() {
        let state = build_test_state(&["example.com"]).await;
        let mailbox_id = seed_mailbox_raw(&state.db, "cas-att@example.com").await;
        let msg_id = seed_message_raw(&state.db, &mailbox_id, now_ts()).await;

        insert_attachment(
            &state.db, "att-cas", &msg_id, "doc.pdf", "application/pdf", "attachment", 3, b"pdf",
        )
        .await
        .unwrap();

        delete_message(&state.db, &msg_id, &mailbox_id).await.unwrap();

        let meta = list_attachments_meta(&state.db, &msg_id).await.unwrap();
        assert!(meta.is_empty());
    }

    // ── Phase 3: events ─────────────────────────────────

    #[tokio::test]
    async fn insert_and_poll_events() {
        let state = build_test_state(&["example.com"]).await;
        let now = now_ts();

        let id1 = insert_event(&state.db, "mb-1", "message.created", "msg-1", now).await.unwrap();
        let id2 = insert_event(&state.db, "mb-1", "message.updated", "msg-1", now).await.unwrap();
        let _id3 = insert_event(&state.db, "mb-2", "message.created", "msg-2", now).await.unwrap();

        // Poll from 0 for mb-1 should get 2 events
        let events = poll_events_since(&state.db, "mb-1", 0).await.unwrap();
        assert_eq!(events.len(), 2);

        // Poll from id1 should get only id2
        let events = poll_events_since(&state.db, "mb-1", id1).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].id, id2);
    }

    #[tokio::test]
    async fn cleanup_old_events_removes_expired() {
        let state = build_test_state(&["example.com"]).await;
        let now = now_ts();

        // Old event (6 minutes ago)
        insert_event(&state.db, "mb-1", "message.created", "old-msg", now - 360).await.unwrap();
        // Recent event
        insert_event(&state.db, "mb-1", "message.created", "new-msg", now).await.unwrap();

        cleanup_old_events(&state.db).await.unwrap();

        let events = poll_events_since(&state.db, "mb-1", 0).await.unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].message_id, "new-msg");
    }
}
