use sqlx::SqlitePool;

use crate::models::MailboxRow;

pub async fn insert_mailbox(
    db: &SqlitePool,
    mailbox_id: &str,
    address: &str,
    password_hash: &str,
    created_at: i64,
    expires_at: Option<i64>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO mailboxes (id, address, password_hash, created_at, expires_at)
        VALUES (?, ?, ?, ?, ?)
        "#,
    )
    .bind(mailbox_id)
    .bind(address)
    .bind(password_hash)
    .bind(created_at)
    .bind(expires_at)
    .execute(db)
    .await?;

    Ok(())
}

pub async fn delete_mailbox(db: &SqlitePool, mailbox_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM mailboxes WHERE id = ?")
        .bind(mailbox_id)
        .execute(db)
        .await?;

    Ok(())
}

pub async fn find_mailbox_by_address(
    db: &SqlitePool,
    address: &str,
) -> Result<Option<MailboxRow>, sqlx::Error> {
    sqlx::query_as::<_, MailboxRow>(
        r#"
        SELECT id, address, password_hash, created_at, expires_at
        FROM mailboxes
        WHERE address = ?
        LIMIT 1
        "#,
    )
    .bind(address)
    .fetch_optional(db)
    .await
}
