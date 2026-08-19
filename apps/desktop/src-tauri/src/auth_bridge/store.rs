use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::OnceLock;

const SERVICE: &str = "com.pulsar.ai.auth-bridge";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredCredential {
    pub kind: String,
    pub header_name: String,
    pub token: String,
    pub expires_at: DateTime<Utc>,
    pub account_hint: Option<String>,
}

static TEST_STORE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

fn test_map() -> &'static Mutex<HashMap<String, String>> {
    TEST_STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn account(broker_id: &str) -> String {
    format!("{broker_id}:record")
}

pub fn put(broker_id: &str, cred: &StoredCredential) -> Result<()> {
    let raw = serde_json::to_string(cred).context("serialize auth-bridge record")?;
    write_item(&account(broker_id), &raw)
}

pub fn get(broker_id: &str) -> Result<Option<StoredCredential>> {
    let Some(raw) = read_item(&account(broker_id))? else {
        return Ok(None);
    };
    let cred: StoredCredential = serde_json::from_str(&raw).context("parse auth-bridge record")?;
    Ok(Some(cred))
}

pub fn delete(broker_id: &str) -> Result<()> {
    delete_item(&account(broker_id))
}

#[allow(dead_code)]
pub fn present(broker_id: &str) -> bool {
    get(broker_id).ok().flatten().is_some()
}

pub fn delete_all(broker_ids: &[&str]) {
    for id in broker_ids {
        let _ = delete(id);
    }
}

fn write_item(account: &str, value: &str) -> Result<()> {
    if cfg!(test) {
        test_map().lock().insert(account.to_string(), value.to_string());
        return Ok(());
    }
    let entry = keyring::Entry::new(SERVICE, account).context("auth-bridge keyring entry")?;
    entry.set_password(value).context("auth-bridge keyring write")
}

fn read_item(account: &str) -> Result<Option<String>> {
    if cfg!(test) {
        return Ok(test_map().lock().get(account).cloned());
    }
    let entry = keyring::Entry::new(SERVICE, account).context("auth-bridge keyring entry")?;
    match entry.get_password() {
        Ok(v) if !v.is_empty() => Ok(Some(v)),
        Ok(_) => Ok(None),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn delete_item(account: &str) -> Result<()> {
    if cfg!(test) {
        test_map().lock().remove(account);
        return Ok(());
    }
    let entry = keyring::Entry::new(SERVICE, account).context("auth-bridge keyring entry")?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn round_trip_and_delete() {
        let cred = StoredCredential {
            kind: "header".into(),
            header_name: "authorization".into(),
            token: "Bearer test-token-value".into(),
            expires_at: Utc::now() + Duration::hours(1),
            account_hint: Some("user@example.com".into()),
        };
        put("test-broker", &cred).unwrap();
        let loaded = get("test-broker").unwrap().unwrap();
        assert_eq!(loaded.token, cred.token);
        delete("test-broker").unwrap();
        assert!(get("test-broker").unwrap().is_none());
    }
}
