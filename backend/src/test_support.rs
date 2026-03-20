use std::{collections::HashMap, fs, net::TcpListener, path::PathBuf, sync::Arc, time::Duration};

use tokio::net::TcpStream;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::{
    auth::now_ts,
    db::{connect_db, find_mailbox_by_address, insert_mailbox, migrate},
    models::{AppConfig, AppState, MailboxRow, SmtpTlsMode},
};

const TEST_TLS_CERT_CHAIN: &str = concat!(
    "-----BEGIN CERTIFICATE-----\n",
    "MIIBszCCAVmgAwIBAgIUUg3keFcU1xXWK8BNVb1KynPulV8wCgYIKoZIzj0EAwIw\n",
    "JjEkMCIGA1UEAwwbUnVzdGxzIFJvYnVzdCBSb290IC0gUnVuZyAyMCAXDTc1MDEw\n",
    "MTAwMDAwMFoYDzQwOTYwMTAxMDAwMDAwWjAhMR8wHQYDVQQDDBZyY2dlbiBzZWxm\n",
    "IHNpZ25lZCBjZXJ0MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEud6w4gtZ0xbw\n",
    "J3E69SSMy5TZfdIifl9L5ZY+hgEe4UiUsBWS32f6Y5NR5Jo8FO1f6o13b3+FvVHR\n",
    "EHCGdvppL6NoMGYwFQYDVR0RBA4wDIIKZm9vYmFyLmNvbTAdBgNVHSUEFjAUBggr\n",
    "BgEFBQcDAQYIKwYBBQUHAwIwHQYDVR0OBBYEFELvxbj5tD75n4pYFvJyr+c8qVEi\n",
    "MA8GA1UdEwEB/wQFMAMBAQAwCgYIKoZIzj0EAwIDSAAwRQIhALxSSdUsrRFnwNMu\n",
    "/doBqI8i8u5HdohVAheFTDwObkOMAiASSjULUtkWSD15u/7Sr01Wm9J1MpqW1pob\n",
    "BVqU3CNRlA==\n",
    "-----END CERTIFICATE-----\n",
    "-----BEGIN CERTIFICATE-----\n",
    "MIIBiTCCATCgAwIBAgIUHWiVYIvMMWoZEFYvSz46COf2FqowCgYIKoZIzj0EAwIw\n",
    "HTEbMBkGA1UEAwwSUnVzdGxzIFJvYnVzdCBSb290MCAXDTc1MDEwMTAwMDAwMFoY\n",
    "DzQwOTYwMTAxMDAwMDAwWjAmMSQwIgYDVQQDDBtSdXN0bHMgUm9idXN0IFJvb3Qg\n",
    "LSBSdW5nIDIwWTATBgcqhkjOPQIBBggqhkjOPQMBBwNCAATAOCcBD7dXjmAZ3te5\n",
    "D47cCJ9ec93PWv7BKYIL826CJsKfXQOGrBTthLm77hXLhHu6uv8E5QXNLZpfowLQ\n",
    "Do1ao0MwQTAPBgNVHQ8BAf8EBQMDB4QAMB0GA1UdDgQWBBRdza76r11Ok9vRmlg6\n",
    "Nn/wL/N+jTAPBgNVHRMBAf8EBTADAQH/MAoGCCqGSM49BAMCA0cAMEQCIFmZrXeK\n",
    "hnfkahocvkhhNT3cDv1LWf6WBoFaCiBwZXFPAiARaKRiSCMG7PCHmSqFe82TBVmL\n",
    "odHGogAVax1Dh/aYAA==\n",
    "-----END CERTIFICATE-----\n"
);

const TEST_TLS_PRIVATE_KEY: &str = concat!(
    "-----BEGIN PRIVATE KEY-----\n",
    "MIGHAgEAMBMGByqGSM49AgEGCCqGSM49AwEHBG0wawIBAQQgTbAQpfjAT46fgF4B\n",
    "mP15n37woNG5ZNJmwcqsred/7tmhRANCAAS53rDiC1nTFvAncTr1JIzLlNl90iJ+\n",
    "X0vllj6GAR7hSJSwFZLfZ/pjk1HkmjwU7V/qjXdvf4W9UdEQcIZ2+mkv\n",
    "-----END PRIVATE KEY-----\n"
);

pub(crate) async fn build_test_state(domains: &[&str]) -> AppState {
    let database_url = format!("sqlite:///tmp/kooixmail-test-{}.db", Uuid::new_v4());
    let db = connect_db(&database_url).await.unwrap();
    migrate(&db).await.unwrap();

    AppState {
        db,
        config: AppConfig {
            domains: domains.iter().map(|domain| domain.to_string()).collect(),
            ingress_token: None,
            smtp_bind_addr: None,
            smtp_hostname: "mx.kooixmail.local".to_string(),
            smtp_tls_mode: SmtpTlsMode::Disabled,
            smtp_tls_cert_path: None,
            smtp_tls_key_path: None,
            ingress_max_message_bytes: 262_144,
            ingress_rate_limit_per_minute: 30,
            ingress_require_spf: false,
            ingress_require_dkim: false,
            ingress_require_dmarc: false,
            ingress_protect_local_domains: false,
            ingress_greylist_enabled: false,
            ingress_greylist_delay_secs: 60,
            ingress_rbl_zones: vec![],
        },
        events: Arc::new(RwLock::new(HashMap::new())),
        ingress_limits: Arc::new(RwLock::new(HashMap::new())),
        greylist: Arc::new(RwLock::new(HashMap::new())),
        mail_auth: None,
    }
}

pub(crate) async fn seed_mailbox(state: &AppState, address: &str) -> MailboxRow {
    let mailbox_id = Uuid::new_v4().to_string();
    let created_at = now_ts();

    insert_mailbox(
        state.db(),
        &mailbox_id,
        address,
        "test-password-hash",
        created_at,
        None,
    )
    .await
    .unwrap();

    find_mailbox_by_address(state.db(), address)
        .await
        .unwrap()
        .unwrap()
}

trait AppStateExt {
    fn db(&self) -> &sqlx::SqlitePool;
}

impl AppStateExt for AppState {
    fn db(&self) -> &sqlx::SqlitePool {
        &self.db
    }
}

pub(crate) fn reserve_local_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    addr.to_string()
}

pub(crate) async fn wait_for_tcp(addr: &str) {
    for _ in 0..50 {
        if TcpStream::connect(addr).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for TCP listener at {addr}");
}

pub(crate) fn write_tls_fixture() -> (PathBuf, PathBuf) {
    let fixture_dir = std::env::temp_dir().join(format!("kooixmail-tls-{}", Uuid::new_v4()));
    fs::create_dir_all(&fixture_dir).unwrap();

    let cert_path = fixture_dir.join("smtp-chain.pem");
    let key_path = fixture_dir.join("smtp.key");
    fs::write(&cert_path, TEST_TLS_CERT_CHAIN).unwrap();
    fs::write(&key_path, TEST_TLS_PRIVATE_KEY).unwrap();

    (cert_path, key_path)
}
