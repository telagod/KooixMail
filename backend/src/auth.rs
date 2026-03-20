use argon2::{
    Argon2,
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
};
use axum::http::{HeaderMap, header::AUTHORIZATION};
use chrono::{TimeZone, Utc};
use rand_core::OsRng;

use crate::{
    db::find_mailbox_by_session_token,
    models::{AppError, AppState, MailboxRow},
};

pub async fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> Result<MailboxRow, AppError> {
    let token = extract_token(headers, query_token)
        .ok_or_else(|| AppError::Unauthorized("missing bearer token".to_string()))?;
    let mailbox = find_mailbox_by_session_token(&state.db, &token)
        .await?
        .ok_or_else(|| AppError::Unauthorized("invalid bearer token".to_string()))?;

    ensure_not_expired(&mailbox)?;
    Ok(mailbox)
}

pub fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(ToOwned::to_owned)
}

pub fn hash_password(password: &str) -> Result<String, AppError> {
    let salt = SaltString::generate(&mut OsRng);
    let hash = Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map_err(|error| AppError::Internal(anyhow::Error::msg(error.to_string())))?;
    Ok(hash.to_string())
}

pub fn verify_password_hash(password: &str, hash: &str) -> Result<(), AppError> {
    let parsed_hash = PasswordHash::new(hash)
        .map_err(|error| AppError::Internal(anyhow::Error::msg(error.to_string())))?;
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed_hash)
        .map_err(|_| AppError::Unauthorized("invalid email or password".to_string()))
}

pub fn normalize_address(address: &str) -> Result<String, AppError> {
    let normalized = address.trim().to_lowercase();
    let mut parts = normalized.split('@');
    let local = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("mailbox local part is required".to_string()))?;
    let domain = parts
        .next()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::BadRequest("mailbox address must include a domain".to_string()))?;

    if parts.next().is_some() || !local.chars().all(valid_local_part_char) {
        return Err(AppError::BadRequest(
            "invalid mailbox address format".to_string(),
        ));
    }

    Ok(format!("{local}@{domain}"))
}

pub fn validate_mailbox_address(address: &str) -> Result<(), AppError> {
    let local = address
        .split('@')
        .next()
        .ok_or_else(|| AppError::BadRequest("invalid mailbox address".to_string()))?;

    if local.len() < 3 {
        Err(AppError::BadRequest(
            "mailbox local part must be at least 3 characters".to_string(),
        ))
    } else {
        Ok(())
    }
}

pub fn ensure_allowed_domain(domains: &[String], address: &str) -> Result<(), AppError> {
    let domain = address
        .split('@')
        .nth(1)
        .ok_or_else(|| AppError::BadRequest("invalid mailbox address".to_string()))?;
    if domains.iter().any(|candidate| candidate == domain) {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "domain {domain} is not enabled"
        )))
    }
}

pub fn validate_password(password: &str) -> Result<(), AppError> {
    if password.len() < 6 {
        Err(AppError::BadRequest(
            "password must be at least 6 characters".to_string(),
        ))
    } else {
        Ok(())
    }
}

pub fn calculate_expiry(expires_in: Option<i64>) -> Result<Option<i64>, AppError> {
    match expires_in.unwrap_or(24 * 60 * 60) {
        0 | -1 => Ok(None),
        value if value > 0 => Ok(Some(now_ts() + value)),
        other => Err(AppError::BadRequest(format!(
            "invalid expires_in value: {other}"
        ))),
    }
}

pub fn ensure_not_expired(mailbox: &MailboxRow) -> Result<(), AppError> {
    if mailbox
        .expires_at
        .is_some_and(|expires_at| expires_at <= now_ts())
    {
        Err(AppError::Unauthorized("mailbox has expired".to_string()))
    } else {
        Ok(())
    }
}

pub fn ts_to_rfc3339(timestamp: i64) -> String {
    Utc.timestamp_opt(timestamp, 0)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339()
}

pub fn now_ts() -> i64 {
    Utc::now().timestamp()
}

fn extract_token(headers: &HeaderMap, query_token: Option<&str>) -> Option<String> {
    if let Some(token) = query_token {
        return Some(token.to_string());
    }

    headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(ToOwned::to_owned)
}

fn valid_local_part_char(ch: char) -> bool {
    ch.is_ascii_lowercase() || ch.is_ascii_digit() || matches!(ch, '.' | '_' | '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::{HeaderMap, HeaderValue, header::AUTHORIZATION};
    use crate::models::MailboxRow;

    // ── normalize_address ────────────────────────────────────────────────────

    #[test]
    fn normalize_address_lowercases_and_trims() {
        assert_eq!(
            normalize_address("User@Example.COM").unwrap(),
            "user@example.com"
        );
    }

    #[test]
    fn normalize_address_strips_surrounding_spaces() {
        assert_eq!(
            normalize_address("  user@domain.com  ").unwrap(),
            "user@domain.com"
        );
    }

    #[test]
    fn normalize_address_no_domain_is_err() {
        assert!(normalize_address("user").is_err());
    }

    #[test]
    fn normalize_address_empty_local_is_err() {
        assert!(normalize_address("@domain.com").is_err());
    }

    #[test]
    fn normalize_address_multiple_at_is_err() {
        assert!(normalize_address("a@b@c").is_err());
    }

    #[test]
    fn normalize_address_space_in_local_is_err() {
        assert!(normalize_address("us er@domain.com").is_err());
    }

    #[test]
    fn normalize_address_valid_special_chars_ok() {
        assert!(normalize_address("user.name_test-1@domain.com").is_ok());
    }

    // ── validate_mailbox_address ─────────────────────────────────────────────

    #[test]
    fn validate_mailbox_address_local_3_ok() {
        assert!(validate_mailbox_address("abc@d.com").is_ok());
    }

    #[test]
    fn validate_mailbox_address_local_2_err() {
        assert!(validate_mailbox_address("ab@d.com").is_err());
    }

    #[test]
    fn validate_mailbox_address_local_1_err() {
        assert!(validate_mailbox_address("a@d.com").is_err());
    }

    // ── ensure_allowed_domain ────────────────────────────────────────────────

    #[test]
    fn ensure_allowed_domain_in_list_ok() {
        let domains = vec!["example.com".to_string(), "test.org".to_string()];
        assert!(ensure_allowed_domain(&domains, "user@example.com").is_ok());
    }

    #[test]
    fn ensure_allowed_domain_not_in_list_err() {
        let domains = vec!["example.com".to_string()];
        assert!(ensure_allowed_domain(&domains, "user@other.com").is_err());
    }

    // ── validate_password ────────────────────────────────────────────────────

    #[test]
    fn validate_password_len_6_ok() {
        assert!(validate_password("123456").is_ok());
    }

    #[test]
    fn validate_password_len_5_err() {
        assert!(validate_password("12345").is_err());
    }

    #[test]
    fn validate_password_empty_err() {
        assert!(validate_password("").is_err());
    }

    // ── calculate_expiry ─────────────────────────────────────────────────────

    #[test]
    fn calculate_expiry_none_defaults_to_86400() {
        let before = now_ts();
        let result = calculate_expiry(None).unwrap();
        let after = now_ts();
        let expiry = result.expect("should be Some");
        assert!(expiry >= before + 86400 && expiry <= after + 86400);
    }

    #[test]
    fn calculate_expiry_zero_returns_none() {
        assert_eq!(calculate_expiry(Some(0)).unwrap(), None);
    }

    #[test]
    fn calculate_expiry_minus_one_returns_none() {
        assert_eq!(calculate_expiry(Some(-1)).unwrap(), None);
    }

    #[test]
    fn calculate_expiry_positive_adds_to_now() {
        let before = now_ts();
        let result = calculate_expiry(Some(3600)).unwrap();
        let after = now_ts();
        let expiry = result.expect("should be Some");
        assert!(expiry >= before + 3600 && expiry <= after + 3600);
    }

    #[test]
    fn calculate_expiry_minus_two_is_err() {
        assert!(calculate_expiry(Some(-2)).is_err());
    }

    // ── ensure_not_expired ───────────────────────────────────────────────────

    fn make_mailbox(expires_at: Option<i64>) -> MailboxRow {
        MailboxRow {
            id: "1".to_string(),
            address: "test@example.com".to_string(),
            password_hash: "hash".to_string(),
            expires_at,
            created_at: 0,
        }
    }

    #[test]
    fn ensure_not_expired_no_expiry_ok() {
        assert!(ensure_not_expired(&make_mailbox(None)).is_ok());
    }

    #[test]
    fn ensure_not_expired_far_future_ok() {
        let future = now_ts() + 99999;
        assert!(ensure_not_expired(&make_mailbox(Some(future))).is_ok());
    }

    #[test]
    fn ensure_not_expired_past_err() {
        let past = now_ts() - 1;
        assert!(ensure_not_expired(&make_mailbox(Some(past))).is_err());
    }

    // ── hash_password + verify_password_hash ─────────────────────────────────

    #[test]
    fn hash_and_verify_roundtrip_ok() {
        let hash = hash_password("mypassword").unwrap();
        assert!(verify_password_hash("mypassword", &hash).is_ok());
    }

    #[test]
    fn verify_wrong_password_err() {
        let hash = hash_password("mypassword").unwrap();
        assert!(verify_password_hash("wrong", &hash).is_err());
    }

    // ── extract_token ────────────────────────────────────────────────────────

    #[test]
    fn extract_token_from_bearer_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer mytoken123"),
        );
        assert_eq!(
            extract_token(&headers, None),
            Some("mytoken123".to_string())
        );
    }

    #[test]
    fn extract_token_query_takes_priority_over_header() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Bearer headertoken"),
        );
        assert_eq!(
            extract_token(&headers, Some("querytoken")),
            Some("querytoken".to_string())
        );
    }

    #[test]
    fn extract_token_no_header_no_query_none() {
        let headers = HeaderMap::new();
        assert_eq!(extract_token(&headers, None), None);
    }

    #[test]
    fn extract_token_malformed_header_none() {
        let mut headers = HeaderMap::new();
        headers.insert(
            AUTHORIZATION,
            HeaderValue::from_static("Token mytoken123"),
        );
        assert_eq!(extract_token(&headers, None), None);
    }

    // ── ts_to_rfc3339 ────────────────────────────────────────────────────────

    #[test]
    fn ts_to_rfc3339_epoch_zero() {
        assert_eq!(ts_to_rfc3339(0), "1970-01-01T00:00:00+00:00");
    }

    // ── valid_local_part_char ────────────────────────────────────────────────

    #[test]
    fn valid_local_part_char_allowed() {
        assert!(valid_local_part_char('a'));
        assert!(valid_local_part_char('0'));
        assert!(valid_local_part_char('.'));
        assert!(valid_local_part_char('_'));
        assert!(valid_local_part_char('-'));
    }

    #[test]
    fn valid_local_part_char_rejected() {
        assert!(!valid_local_part_char('A'));
        assert!(!valid_local_part_char(' '));
        assert!(!valid_local_part_char('@'));
    }
}
