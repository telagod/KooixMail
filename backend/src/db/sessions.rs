use sqlx::SqlitePool;

use crate::models::MailboxRow;

pub async fn insert_session(
    db: &SqlitePool,
    token: &str,
    mailbox_id: &str,
    created_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO sessions (token, mailbox_id, created_at)
        VALUES (?, ?, ?)
        "#,
    )
    .bind(token)
    .bind(mailbox_id)
    .bind(created_at)
    .execute(db)
    .await?;

    Ok(())
}

pub async fn find_mailbox_by_session_token(
    db: &SqlitePool,
    token: &str,
) -> Result<Option<MailboxRow>, sqlx::Error> {
    sqlx::query_as::<_, MailboxRow>(
        r#"
        SELECT m.id, m.address, m.password_hash, m.created_at, m.expires_at
        FROM sessions s
        JOIN mailboxes m ON m.id = s.mailbox_id
        WHERE s.token = ?
        LIMIT 1
        "#,
    )
    .bind(token)
    .fetch_optional(db)
    .await
}
