use sqlx::SqlitePool;

use crate::models::{AttachmentMetaRow, AttachmentRow};

#[allow(clippy::too_many_arguments)]
pub async fn insert_attachment(
    db: &SqlitePool,
    id: &str,
    message_id: &str,
    filename: &str,
    content_type: &str,
    disposition: &str,
    size: i64,
    data: &[u8],
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO attachments (id, message_id, filename, content_type, disposition, size, data) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(message_id)
    .bind(filename)
    .bind(content_type)
    .bind(disposition)
    .bind(size)
    .bind(data)
    .execute(db)
    .await?;

    Ok(())
}

pub async fn list_attachments_meta(
    db: &SqlitePool,
    message_id: &str,
) -> anyhow::Result<Vec<AttachmentMetaRow>> {
    let rows = sqlx::query_as::<_, AttachmentMetaRow>(
        "SELECT id, message_id, filename, content_type, disposition, size FROM attachments WHERE message_id = ?",
    )
    .bind(message_id)
    .fetch_all(db)
    .await?;

    Ok(rows)
}

pub async fn fetch_attachment(
    db: &SqlitePool,
    attachment_id: &str,
    message_id: &str,
) -> anyhow::Result<Option<AttachmentRow>> {
    let row = sqlx::query_as::<_, AttachmentRow>(
        "SELECT id, message_id, filename, content_type, disposition, size, data FROM attachments WHERE id = ? AND message_id = ?",
    )
    .bind(attachment_id)
    .bind(message_id)
    .fetch_optional(db)
    .await?;

    Ok(row)
}
