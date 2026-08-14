use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use keyring::Entry;
use parking_lot::Mutex;

const SERVICE: &str = "com.pulsar.ai";
/// Single Keychain account so macOS prompts once instead of per-secret.
const VAULT_ACCOUNT: &str = "vault";

pub const SECRET_PULSE_PASSWORD: &str = "pulse_password";
pub const SECRET_PULSE_API_KEY: &str = "pulse_api_key";
pub const SECRET_LLM_API_KEY: &str = "llm_api_key";
pub const SECRET_WEALTH_PASSWORD: &str = "wealth_password";
pub const SECRET_WEALTH_TOKEN: &str = "wealth_token";
pub const SECRET_WEALTH_TOKEN_EXPIRES: &str = "wealth_token_expires";
pub const SECRET_WEALTH_REFRESH_TOKEN: &str = "wealth_refresh_token";
pub const SECRET_BAMBOO_PASSWORD: &str = "bamboo_password";
pub const SECRET_BAMBOO_TOKEN: &str = "bamboo_token";
pub const SECRET_BAMBOO_TOKEN_EXPIRES: &str = "bamboo_token_expires";
pub const SECRET_BAMBOO_USER_ID: &str = "bamboo_user_id";
pub const SECRET_BAMBOO_NGN_WALLET_ID: &str = "bamboo_ngn_wallet_id";
pub const SECRET_BAMBOO_TRANSACTION_PIN: &str = "bamboo_transaction_pin";

const LEGACY_KEYS: &[&str] = &[
    SECRET_PULSE_PASSWORD,
    SECRET_PULSE_API_KEY,
    SECRET_LLM_API_KEY,
    SECRET_WEALTH_PASSWORD,
    SECRET_WEALTH_TOKEN,
    SECRET_WEALTH_TOKEN_EXPIRES,
    SECRET_WEALTH_REFRESH_TOKEN,
    SECRET_BAMBOO_PASSWORD,
    SECRET_BAMBOO_TOKEN,
    SECRET_BAMBOO_TOKEN_EXPIRES,
    SECRET_BAMBOO_USER_ID,
    SECRET_BAMBOO_NGN_WALLET_ID,
    SECRET_BAMBOO_TRANSACTION_PIN,
];

static APP_DATA: OnceLock<PathBuf> = OnceLock::new();
static VAULT: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub fn init(app_data_dir: &Path) {
    let _ = APP_DATA.set(app_data_dir.to_path_buf());
}

/// Unlock the vault once at launch so later reads hit memory, not Keychain.
pub fn preload() {
    let mut guard = VAULT.lock();
    if guard.is_some() {
        return;
    }
    match load_unlocked() {
        Ok(map) => *guard = Some(map),
        Err(e) => tracing::warn!(target: "secrets", error = %e, "failed to unlock secrets vault"),
    }
}

pub fn set_secret(key: &str, value: &str) -> Result<()> {
    let mut guard = VAULT.lock();
    if guard.is_none() {
        *guard = Some(load_unlocked().unwrap_or_default());
    }
    let map = guard.as_mut().expect("vault initialized");
    map.insert(key.to_string(), value.to_string());
    persist(map)
}

pub fn get_secret(key: &str) -> Result<Option<String>> {
    let mut guard = VAULT.lock();
    if guard.is_none() {
        *guard = Some(load_unlocked()?);
    }
    Ok(guard
        .as_ref()
        .expect("vault initialized")
        .get(key)
        .cloned()
        .filter(|s| !s.is_empty()))
}

pub fn delete_secret(key: &str) -> Result<()> {
    let mut guard = VAULT.lock();
    if guard.is_none() {
        *guard = Some(load_unlocked().unwrap_or_default());
    }
    let map = guard.as_mut().expect("vault initialized");
    map.remove(key);
    persist(map)
}

fn vault_path() -> Option<PathBuf> {
    APP_DATA.get().map(|p| p.join("secrets.vault"))
}

fn keychain_entry(account: &str) -> Result<Entry> {
    Entry::new(SERVICE, account).context("keyring entry")
}

fn read_keychain(account: &str) -> Result<Option<String>> {
    match keychain_entry(account)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_keychain(account: &str, value: &str) -> Result<()> {
    keychain_entry(account)?
        .set_password(value)
        .context("set keyring password")
}

fn parse_vault(raw: &str) -> HashMap<String, String> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return HashMap::new();
    };
    let Some(obj) = value.as_object() else {
        return HashMap::new();
    };
    obj.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .filter(|(_, v)| !v.is_empty())
        .collect()
}

fn read_vault_file(path: &Path) -> Result<HashMap<String, String>> {
    let raw = std::fs::read_to_string(path).context("read secrets vault")?;
    Ok(parse_vault(&raw))
}

fn persist_file(map: &HashMap<String, String>) -> Result<()> {
    let Some(path) = vault_path() else {
        return persist_keychain(map);
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create app data dir")?;
    }
    let raw = serde_json::to_string(map).context("serialize secrets vault")?;
    std::fs::write(&path, raw).context("write secrets vault")?;
    harden_file_permissions(&path);
    Ok(())
}

fn persist_keychain(map: &HashMap<String, String>) -> Result<()> {
    let raw = serde_json::to_string(map).context("serialize secrets vault")?;
    write_keychain(VAULT_ACCOUNT, &raw)
}

fn persist(map: &HashMap<String, String>) -> Result<()> {
    // Unsigned debug binaries change on every rebuild; macOS Keychain ACLs then
    // re-prompt for every item. Keep the vault on disk in dev (0600).
    if crate::runtime_util::is_dev() && vault_path().is_some() {
        persist_file(map)
    } else {
        persist_keychain(map)
    }
}

fn load_unlocked() -> Result<HashMap<String, String>> {
    if let Some(path) = vault_path() {
        if path.is_file() {
            return read_vault_file(&path);
        }
    }

    if let Some(raw) = read_keychain(VAULT_ACCOUNT).ok().flatten() {
        let map = parse_vault(&raw);
        if crate::runtime_util::is_dev() && vault_path().is_some() {
            let _ = persist_file(&map);
        }
        return Ok(map);
    }

    let mut map = HashMap::new();
    for key in LEGACY_KEYS {
        if let Ok(Some(v)) = read_keychain(key) {
            if !v.is_empty() {
                map.insert((*key).to_string(), v);
            }
        }
    }
    if !map.is_empty() {
        let _ = persist(&map);
    }
    Ok(map)
}

fn harden_file_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct PulseSecrets {
    pub password: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct LlmSecrets {
    pub api_key: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_vault_reads_string_fields() {
        let map = parse_vault(r#"{"pulse_password":"p","wealth_token":"t","empty":""}"#);
        assert_eq!(map.get("pulse_password").map(String::as_str), Some("p"));
        assert_eq!(map.get("wealth_token").map(String::as_str), Some("t"));
        assert!(!map.contains_key("empty"));
    }

    #[test]
    fn parse_vault_ignores_invalid_json() {
        assert!(parse_vault("not-json").is_empty());
        assert!(parse_vault("[]").is_empty());
    }
}
