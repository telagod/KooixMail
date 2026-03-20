use std::{collections::HashMap, str::FromStr};

use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use tokio::time::{Duration as TokioDuration, sleep};
use tracing::{error, info};

use crate::{auth::now_ts, models::AppState};

pub async fn connect_db(database_url: &str) -> anyhow::Result<SqlitePool> {
    let connect_options = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .busy_timeout(std::time::Duration::from_secs(5));

    SqlitePoolOptions::new()
        .max_connections(16)
        .min_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(10))
        .connect_with(connect_options)
        .await
        .map_err(Into::into)
}

pub async fn migrate(db: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS mailboxes (
            id TEXT PRIMARY KEY,
            address TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            expires_at INTEGER
        );

        CREATE TABLE IF NOT EXISTS sessions (
            token TEXT PRIMARY KEY,
            mailbox_id TEXT NOT NULL,
            created_at INTEGER NOT NULL,
            FOREIGN KEY (mailbox_id) REFERENCES mailboxes(id) ON DELETE CASCADE
        );

        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            mailbox_id TEXT NOT NULL,
            to_address TEXT NOT NULL,
            from_name TEXT NOT NULL,
            from_address TEXT NOT NULL,
            subject TEXT NOT NULL,
            text_body TEXT NOT NULL,
            html_body TEXT,
            seen INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            FOREIGN KEY (mailbox_id) REFERENCES mailboxes(id) ON DELETE CASCADE
        );

        CREATE INDEX IF NOT EXISTS idx_messages_mailbox_created_at
        ON messages (mailbox_id, created_at DESC);

        CREATE INDEX IF NOT EXISTS idx_sessions_mailbox_id
        ON sessions (mailbox_id);

        CREATE INDEX IF NOT EXISTS idx_mailboxes_expires_at
        ON mailboxes (expires_at) WHERE expires_at IS NOT NULL;

        CREATE TABLE IF NOT EXISTS ingress_limits (
            key TEXT NOT NULL,
            hit_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_ingress_limits_key ON ingress_limits (key);

        CREATE TABLE IF NOT EXISTS greylist (
            key TEXT PRIMARY KEY,
            first_seen INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS attachments (
            id TEXT PRIMARY KEY,
            message_id TEXT NOT NULL,
            filename TEXT NOT NULL DEFAULT 'attachment',
            content_type TEXT NOT NULL DEFAULT 'application/octet-stream',
            disposition TEXT NOT NULL DEFAULT 'attachment',
            size INTEGER NOT NULL,
            data BLOB NOT NULL,
            FOREIGN KEY (message_id) REFERENCES messages(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_attachments_message_id ON attachments (message_id);

        CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            mailbox_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            message_id TEXT NOT NULL,
            created_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_events_mailbox_id ON events (mailbox_id, id);
        "#,
    )
    .execute(db)
    .await?;

    Ok(())
}

pub fn spawn_cleanup_worker(state: AppState) {
    tokio::spawn(async move {
        loop {
            sleep(TokioDuration::from_secs(60)).await;
            if let Err(error) = cleanup_expired_mailboxes(&state.db).await {
                error!(?error, "failed to cleanup expired mailboxes");
            }
            prune_ingress_limits(&state).await;
            prune_greylist(&state).await;
            prune_event_channels(&state).await;
            if let Err(error) = flush_ingress_limits(&state).await {
                error!(?error, "failed to flush ingress limits to DB");
            }
            if let Err(error) = flush_greylist(&state).await {
                error!(?error, "failed to flush greylist to DB");
            }
            if let Err(error) = cleanup_old_events(&state.db).await {
                error!(?error, "failed to cleanup old events");
            }
        }
    });
}

pub(crate) async fn cleanup_expired_mailboxes(db: &SqlitePool) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM mailboxes WHERE expires_at IS NOT NULL AND expires_at <= ?")
        .bind(now_ts())
        .execute(db)
        .await?;

    Ok(())
}

async fn prune_ingress_limits(state: &AppState) {
    let window_start = now_ts() - 60;
    let mut limits = state.ingress_limits.write().await;
    let before = limits.len();
    limits.retain(|_, hits| {
        hits.retain(|ts| *ts > window_start);
        !hits.is_empty()
    });
    let pruned = before.saturating_sub(limits.len());
    if pruned > 0 {
        info!(pruned, remaining = limits.len(), "pruned ingress rate-limit entries");
    }
}

async fn prune_greylist(state: &AppState) {
    let max_age = (state.config.ingress_greylist_delay_secs as i64) * 10;
    let cutoff = now_ts() - max_age.max(600);
    let mut greylist = state.greylist.write().await;
    let before = greylist.len();
    greylist.retain(|_, first_seen| *first_seen > cutoff);
    let pruned = before.saturating_sub(greylist.len());
    if pruned > 0 {
        info!(pruned, remaining = greylist.len(), "pruned greylist entries");
    }
}

async fn prune_event_channels(state: &AppState) {
    let mut events = state.events.write().await;
    let before = events.len();
    events.retain(|_, sender| sender.receiver_count() > 0);
    let pruned = before.saturating_sub(events.len());
    if pruned > 0 {
        info!(pruned, remaining = events.len(), "pruned orphaned SSE channels");
    }
}

// ── Phase 1: ingress_limits persistence ─────────────────

pub async fn load_ingress_limits(db: &SqlitePool) -> anyhow::Result<HashMap<String, Vec<i64>>> {
    let rows = sqlx::query("SELECT key, hit_at FROM ingress_limits")
        .fetch_all(db)
        .await?;

    let mut map: HashMap<String, Vec<i64>> = HashMap::new();
    for row in rows {
        let key: String = row.get("key");
        let hit_at: i64 = row.get("hit_at");
        map.entry(key).or_default().push(hit_at);
    }
    Ok(map)
}

pub async fn flush_ingress_limits(state: &AppState) -> anyhow::Result<()> {
    let snapshot = {
        let limits = state.ingress_limits.read().await;
        limits.clone()
    };

    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM ingress_limits")
        .execute(&mut *tx)
        .await?;
    for (key, hits) in &snapshot {
        for hit_at in hits {
            sqlx::query("INSERT INTO ingress_limits (key, hit_at) VALUES (?, ?)")
                .bind(key)
                .bind(hit_at)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;

    Ok(())
}

// ── Phase 1: greylist persistence ───────────────────────

pub async fn load_greylist(db: &SqlitePool) -> anyhow::Result<HashMap<String, i64>> {
    let rows = sqlx::query("SELECT key, first_seen FROM greylist")
        .fetch_all(db)
        .await?;

    let mut map = HashMap::new();
    for row in rows {
        let key: String = row.get("key");
        let first_seen: i64 = row.get("first_seen");
        map.insert(key, first_seen);
    }
    Ok(map)
}

pub async fn flush_greylist(state: &AppState) -> anyhow::Result<()> {
    let snapshot = {
        let greylist = state.greylist.read().await;
        greylist.clone()
    };

    let mut tx = state.db.begin().await?;
    sqlx::query("DELETE FROM greylist")
        .execute(&mut *tx)
        .await?;
    for (key, first_seen) in &snapshot {
        sqlx::query("INSERT INTO greylist (key, first_seen) VALUES (?, ?)")
            .bind(key)
            .bind(first_seen)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;

    Ok(())
}

// ── Phase 3: events persistence ─────────────────────────

pub async fn insert_event(
    db: &SqlitePool,
    mailbox_id: &str,
    kind: &str,
    message_id: &str,
    created_at: i64,
) -> anyhow::Result<i64> {
    let result = sqlx::query(
        "INSERT INTO events (mailbox_id, kind, message_id, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(mailbox_id)
    .bind(kind)
    .bind(message_id)
    .bind(created_at)
    .execute(db)
    .await?;

    Ok(result.last_insert_rowid())
}

pub async fn poll_events_since(
    db: &SqlitePool,
    mailbox_id: &str,
    last_event_id: i64,
) -> anyhow::Result<Vec<crate::models::EventRow>> {
    let rows = sqlx::query_as::<_, crate::models::EventRow>(
        "SELECT id, mailbox_id, kind, message_id, created_at FROM events WHERE mailbox_id = ? AND id > ? ORDER BY id ASC",
    )
    .bind(mailbox_id)
    .bind(last_event_id)
    .fetch_all(db)
    .await?;

    Ok(rows)
}

pub async fn cleanup_old_events(db: &SqlitePool) -> anyhow::Result<()> {
    let cutoff = now_ts() - 300; // 5 minutes
    sqlx::query("DELETE FROM events WHERE created_at < ?")
        .bind(cutoff)
        .execute(db)
        .await?;
    Ok(())
}
