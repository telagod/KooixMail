use sqlx::SqlitePool;

use crate::models::MessageRow;

pub async fn list_messages_by_mailbox(
    db: &SqlitePool,
    mailbox_id: &str,
    limit: i64,
    offset: i64,
) -> Result<Vec<MessageRow>, sqlx::Error> {
    sqlx::query_as::<_, MessageRow>(
        r#"
        SELECT
            id, mailbox_id, to_address, from_name, from_address,
            subject, text_body, html_body, seen, created_at, updated_at
        FROM messages
        WHERE mailbox_id = ?
        ORDER BY created_at DESC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(mailbox_id)
    .bind(limit)
    .bind(offset)
    .fetch_all(db)
    .await
}

pub async fn fetch_message(
    db: &SqlitePool,
    message_id: &str,
    mailbox_id: &str,
) -> Result<Option<MessageRow>, sqlx::Error> {
    sqlx::query_as::<_, MessageRow>(
        r#"
        SELECT
            id, mailbox_id, to_address, from_name, from_address,
            subject, text_body, html_body, seen, created_at, updated_at
        FROM messages
        WHERE id = ? AND mailbox_id = ?
        LIMIT 1
        "#,
    )
    .bind(message_id)
    .bind(mailbox_id)
    .fetch_optional(db)
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn insert_message(
    db: &SqlitePool,
    message_id: &str,
    mailbox_id: &str,
    to_address: &str,
    from_name: &str,
    from_address: &str,
    subject: &str,
    text_body: &str,
    html_body: Option<&str>,
    created_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO messages (
            id, mailbox_id, to_address, from_name, from_address,
            subject, text_body, html_body, seen, created_at, updated_at
        )
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, ?, ?)
        "#,
    )
    .bind(message_id)
    .bind(mailbox_id)
    .bind(to_address)
    .bind(from_name)
    .bind(from_address)
    .bind(subject)
    .bind(text_body)
    .bind(html_body)
    .bind(created_at)
    .bind(created_at)
    .execute(db)
    .await?;

    Ok(())
}

pub async fn update_message_seen(
    db: &SqlitePool,
    message_id: &str,
    mailbox_id: &str,
    seen: bool,
    updated_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        UPDATE messages
        SET seen = ?, updated_at = ?
        WHERE id = ? AND mailbox_id = ?
        "#,
    )
    .bind(if seen { 1 } else { 0 })
    .bind(updated_at)
    .bind(message_id)
    .bind(mailbox_id)
    .execute(db)
    .await?;

    Ok(())
}

pub async fn delete_message(
    db: &SqlitePool,
    message_id: &str,
    mailbox_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM messages WHERE id = ? AND mailbox_id = ?")
        .bind(message_id)
        .bind(mailbox_id)
        .execute(db)
        .await?;

    Ok(())
}
