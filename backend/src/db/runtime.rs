use std::str::FromStr;

use sqlx::{
    SqlitePool,
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
