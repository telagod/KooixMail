mod mailboxes;
mod messages;
mod runtime;
mod sessions;

pub use mailboxes::{delete_mailbox, find_mailbox_by_address, insert_mailbox};
pub use messages::{
    delete_message, fetch_message, insert_message, list_messages_by_mailbox, update_message_seen,
};
pub use runtime::{connect_db, migrate, spawn_cleanup_worker};
pub use sessions::{find_mailbox_by_session_token, insert_session};

#[cfg(test)]
mod tests {
    use uuid::Uuid;

    use crate::{
        auth::now_ts,
        db::{
            delete_mailbox, delete_message, fetch_message, find_mailbox_by_address,
            find_mailbox_by_session_token, insert_mailbox, insert_message, insert_session,
            list_messages_by_mailbox, update_message_seen,
        },
        db::runtime::cleanup_expired_mailboxes,
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
}
