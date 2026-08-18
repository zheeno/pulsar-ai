use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
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
pub const SECRET_BAMBOO_REFRESH_TOKEN: &str = "bamboo_refresh_token";
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
    SECRET_BAMBOO_REFRESH_TOKEN,
    SECRET_BAMBOO_USER_ID,
    SECRET_BAMBOO_NGN_WALLET_ID,
    SECRET_BAMBOO_TRANSACTION_PIN,
];

/// Refresh this many seconds before recorded expiry.
pub const TOKEN_EXPIRY_SKEW_SECS: i64 = 30;

static APP_DATA: OnceLock<PathBuf> = OnceLock::new();
static VAULT: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

#[cfg(test)]
static TEST_DISK: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);
#[cfg(test)]
static TEST_LOCK: Mutex<()> = Mutex::new(());

pub fn init(app_data_dir: &Path) {
    let _ = APP_DATA.set(app_data_dir.to_path_buf());
}

/// Load the vault from disk/keychain. Always reloads so a `get_secret` that ran
/// before `init()` cannot poison memory with an empty map and skip the file vault.
pub fn preload() {
    match load_unlocked() {
        Ok(map) => {
            log_vault_unlocked(&map);
            *VAULT.lock() = Some(map);
        }
        Err(e) => tracing::warn!(target: "secrets", error = %e, "failed to unlock secrets vault"),
    }
}

pub fn set_secret(key: &str, value: &str) -> Result<()> {
    let mut guard = VAULT.lock();
    if guard.is_none() {
        *guard = Some(load_unlocked().context("unlock vault before write")?);
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
        *guard = Some(load_unlocked().context("unlock vault before delete")?);
    }
    let map = guard.as_mut().expect("vault initialized");
    map.remove(key);
    persist(map)
}

/// Whether a non-empty secret is in the unlocked vault (never logs the value).
pub fn secret_present(key: &str) -> bool {
    get_secret(key).ok().flatten().is_some()
}

/// Compare settings `*_connected` flags with vault token presence after launch.
pub fn log_broker_restore_probe(wealth_connected: bool, bamboo_connected: bool) {
    let wealth_token = secret_present(SECRET_WEALTH_TOKEN);
    let bamboo_token = secret_present(SECRET_BAMBOO_TOKEN);
    tracing::info!(
        target: "secrets",
        wealth_connected,
        wealth_token,
        bamboo_connected,
        bamboo_token,
        "broker session restore check"
    );
    if wealth_connected && !wealth_token {
        tracing::warn!(
            target: "secrets",
            "wealth_connected is true but vault has no access token"
        );
    }
    if bamboo_connected && !bamboo_token {
        tracing::warn!(
            target: "secrets",
            "bamboo_connected is true but vault has no access token"
        );
    }
}

#[derive(Debug, Clone)]
pub struct StoredAccessToken {
    pub access_token: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRestoreKind {
    Valid,
    Expired,
    Missing,
}

pub fn token_unexpired(expires_at: DateTime<Utc>) -> bool {
    token_is_fresh(expires_at, TOKEN_EXPIRY_SKEW_SECS)
}

pub fn parse_expiry_datetime(raw: &str) -> Option<DateTime<Utc>> {
    parse_expiry(raw)
}

pub fn load_stored_session(token_key: &str, expires_key: &str) -> Option<StoredAccessToken> {
    hydrate_access_token(
        get_secret(token_key).ok().flatten(),
        get_secret(expires_key).ok().flatten(),
    )
}

pub fn stored_session_kind(token_key: &str, expires_key: &str) -> SessionRestoreKind {
    match load_stored_session(token_key, expires_key) {
        Some(s) if token_unexpired(s.expires_at) => SessionRestoreKind::Valid,
        Some(_) => SessionRestoreKind::Expired,
        None => SessionRestoreKind::Missing,
    }
}

/// Rebuild an access token from vault strings. Missing/odd expiry falls back to JWT `exp`, then +60m.
/// If the stored expiry parsed as a bogus past unix time (e.g. `"60"` TTL minutes) but the JWT
/// `exp` is still fresh, prefer the JWT so a restart inside the access-token TTL stays live.
pub fn hydrate_access_token(
    token: Option<String>,
    expires_raw: Option<String>,
) -> Option<StoredAccessToken> {
    let access_token = token.filter(|t| !t.is_empty())?;
    let parsed = expires_raw.as_deref().and_then(parse_expiry);
    let jwt = jwt_expiry(&access_token);
    let expires_at = match (parsed, jwt) {
        (Some(stored), Some(jwt_exp))
            if !token_unexpired(stored) && token_unexpired(jwt_exp) =>
        {
            jwt_exp
        }
        (Some(stored), _) => stored,
        (None, Some(jwt_exp)) => jwt_exp,
        (None, None) => Utc::now() + chrono::Duration::minutes(60),
    };
    Some(StoredAccessToken {
        access_token,
        expires_at,
    })
}

pub fn token_is_fresh(expires_at: DateTime<Utc>, skew_secs: i64) -> bool {
    token_is_fresh_at(expires_at, Utc::now(), skew_secs)
}

pub fn token_is_fresh_at(expires_at: DateTime<Utc>, now: DateTime<Utc>, skew_secs: i64) -> bool {
    expires_at > now + chrono::Duration::seconds(skew_secs)
}

/// Parse RFC3339, ISO-8601, or unix seconds/millis. Never panics.
pub fn parse_expiry(raw: &str) -> Option<DateTime<Utc>> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    const NAIVE_FMTS: &[&str] = &[
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%d %H:%M:%S",
    ];
    for fmt in NAIVE_FMTS {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(s, fmt) {
            return Some(DateTime::from_naive_utc_and_offset(naive, Utc));
        }
    }
    if let Ok(n) = s.parse::<i64>() {
        return unix_to_utc(n);
    }
    if let Ok(f) = s.parse::<f64>() {
        if f.is_finite() {
            return unix_to_utc(f as i64);
        }
    }
    None
}

pub fn parse_expiry_json(value: &serde_json::Value) -> Option<DateTime<Utc>> {
    match value {
        serde_json::Value::String(s) => parse_expiry(s),
        serde_json::Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_u64().map(|u| u as i64))
            .or_else(|| n.as_f64().map(|f| f as i64))
            .and_then(unix_to_utc),
        _ => None,
    }
}

pub fn jwt_expiry(token: &str) -> Option<DateTime<Utc>> {
    let payload = decode_jwt_payload(token)?;
    payload
        .get("exp")
        .and_then(parse_expiry_json)
}

fn decode_jwt_payload(token: &str) -> Option<serde_json::Value> {
    let payload = token.split('.').nth(1)?;
    let mut padded = payload.replace('-', "+").replace('_', "/");
    while padded.len() % 4 != 0 {
        padded.push('=');
    }
    let bytes = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        padded.as_bytes(),
    )
    .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn unix_to_utc(n: i64) -> Option<DateTime<Utc>> {
    let secs = if n.abs() > 1_000_000_000_000 {
        n / 1000
    } else {
        n
    };
    // TTL minutes/seconds (e.g. "60", "3600") must not parse as 1970-era unix time.
    if secs < 1_000_000_000 {
        return None;
    }
    Utc.timestamp_opt(secs, 0).single()
}

fn vault_path() -> Option<PathBuf> {
    APP_DATA.get().map(|p| p.join("secrets.vault"))
}

/// Unsigned debug binaries change on every rebuild; macOS Keychain ACLs then
/// re-prompt (or fail closed). Persist to a 0600 file in debug/`tauri dev`.
/// Do **not** key this off `APP_ENV` — production API + debug binary is common.
fn use_file_vault() -> bool {
    vault_path().is_some() && (cfg!(debug_assertions) || cfg!(dev))
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
    #[cfg(test)]
    {
        *TEST_DISK.lock() = Some(map.clone());
        return Ok(());
    }
    #[allow(unreachable_code)]
    if use_file_vault() {
        persist_file(map)
    } else {
        persist_keychain(map)
    }
}

fn load_unlocked() -> Result<HashMap<String, String>> {
    #[cfg(test)]
    {
        return Ok(TEST_DISK.lock().clone().unwrap_or_default());
    }

    #[allow(unreachable_code)]
    let mut map = HashMap::new();
    let mut sources: Vec<&str> = Vec::new();
    let mut keychain_err: Option<String> = None;
    let mut file_present = false;

    match read_keychain(VAULT_ACCOUNT) {
        Ok(Some(raw)) => {
            let parsed = parse_vault(&raw);
            if !parsed.is_empty() {
                map.extend(parsed);
                sources.push("keychain");
            }
        }
        Ok(None) => {}
        Err(e) => {
            keychain_err = Some(e.to_string());
            tracing::warn!(target: "secrets", error = %e, "keychain vault unread");
        }
    }

    if let Some(path) = vault_path() {
        if path.is_file() {
            file_present = true;
            match read_vault_file(&path) {
                Ok(file_map) => {
                    if !file_map.is_empty() {
                        if use_file_vault() {
                            // Dev: file is the persist target and wins.
                            map.extend(file_map);
                        } else {
                            // Production: keychain is the persist target. A leftover
                            // secrets.vault from `tauri dev` must not overwrite live tokens.
                            for (k, v) in file_map {
                                map.entry(k).or_insert(v);
                            }
                        }
                        sources.push("file");
                    } else {
                        sources.push("file-empty");
                    }
                }
                Err(e) => tracing::warn!(target: "secrets", error = %e, "file vault unread"),
            }
        }
    }

    for key in LEGACY_KEYS {
        if map.contains_key(*key) {
            continue;
        }
        if let Ok(Some(v)) = read_keychain(key) {
            if !v.is_empty() {
                map.insert((*key).to_string(), v);
                if !sources.iter().any(|s| *s == "legacy") {
                    sources.push("legacy");
                }
            }
        }
    }

    if map.is_empty() {
        if let Some(err) = keychain_err {
            if !file_present {
                return Err(anyhow::anyhow!(err)).context("unlock secrets vault");
            }
        }
    } else if use_file_vault() {
        let _ = persist_file(&map);
    }

    tracing::debug!(
        target: "secrets",
        sources = %sources.join("+"),
        "secrets vault sources"
    );
    Ok(map)
}

fn log_vault_unlocked(map: &HashMap<String, String>) {
    let mut keys: Vec<&str> = map.keys().map(String::as_str).collect();
    keys.sort_unstable();
    tracing::info!(
        target: "secrets",
        key_count = map.len(),
        keys = %keys.join(","),
        wealth_token = map.contains_key(SECRET_WEALTH_TOKEN),
        bamboo_token = map.contains_key(SECRET_BAMBOO_TOKEN),
        "secrets vault unlocked"
    );
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
pub struct VaultTestGuard {
    _lock: parking_lot::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for VaultTestGuard {
    fn drop(&mut self) {
        *VAULT.lock() = None;
        *TEST_DISK.lock() = None;
    }
}

#[cfg(test)]
pub fn vault_test_guard() -> VaultTestGuard {
    let lock = TEST_LOCK.lock();
    *VAULT.lock() = None;
    *TEST_DISK.lock() = None;
    VaultTestGuard { _lock: lock }
}

#[cfg(test)]
pub fn seed_vault(pairs: &[(&str, &str)]) {
    *TEST_DISK.lock() = None;
    *VAULT.lock() = Some(HashMap::new());
    for (k, v) in pairs {
        set_secret(k, v).expect("seed vault");
    }
}

#[cfg(test)]
pub fn with_isolated_vault<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    let _guard = vault_test_guard();
    f()
}

#[cfg(test)]
pub fn simulate_restart() {
    *VAULT.lock() = None;
    preload();
}

/// In-memory vault looks empty while TEST_DISK still has secrets (pre-init get_secret poison).
#[cfg(test)]
pub fn poison_empty_memory_keep_disk() {
    *VAULT.lock() = Some(HashMap::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn future_rfc3339() -> String {
        (Utc::now() + chrono::Duration::hours(2)).to_rfc3339()
    }

    fn past_rfc3339() -> String {
        (Utc::now() - chrono::Duration::hours(2)).to_rfc3339()
    }

    fn test_jwt_with_exp(exp: i64) -> String {
        let payload = serde_json::json!({ "exp": exp, "sub": "1" });
        let raw = serde_json::to_vec(&payload).unwrap();
        let b64 = base64::Engine::encode(&base64::engine::general_purpose::URL_SAFE_NO_PAD, &raw);
        format!("eyJhbGciOiJub25lIn0.{b64}.sig")
    }

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

    #[test]
    fn parse_expiry_rfc3339_and_unix_and_millis() {
        let rfc = parse_expiry("2026-08-17T21:00:00Z").unwrap();
        assert_eq!(rfc.timestamp(), 1_787_000_400);
        let naive = parse_expiry("2026-08-17T21:00:00").unwrap();
        assert_eq!(naive.timestamp(), 1_787_000_400);
        let unix = parse_expiry("1787000400").unwrap();
        assert_eq!(unix.timestamp(), 1_787_000_400);
        let millis = parse_expiry("1787000400000").unwrap();
        assert_eq!(millis.timestamp(), 1_787_000_400);
        assert!(parse_expiry("").is_none());
        assert!(parse_expiry("not-a-date").is_none());
        // TTL minutes/seconds must not become 1970-01-01.
        assert!(parse_expiry("60").is_none());
        assert!(parse_expiry("3600").is_none());
    }

    #[test]
    fn clock_skew_treats_near_expiry_as_stale() {
        let now = Utc::now();
        assert!(!token_is_fresh_at(
            now + chrono::Duration::seconds(10),
            now,
            TOKEN_EXPIRY_SKEW_SECS
        ));
        assert!(token_is_fresh_at(
            now + chrono::Duration::minutes(5),
            now,
            TOKEN_EXPIRY_SKEW_SECS
        ));
        assert!(!token_is_fresh_at(
            now - chrono::Duration::seconds(1),
            now,
            TOKEN_EXPIRY_SKEW_SECS
        ));
    }

    #[test]
    fn hydrate_uses_stored_expiry_when_valid() {
        let exp = future_rfc3339();
        let session = hydrate_access_token(Some("tok".into()), Some(exp.clone())).unwrap();
        assert_eq!(session.access_token, "tok");
        assert!(token_is_fresh(session.expires_at, TOKEN_EXPIRY_SKEW_SECS));
    }

    #[test]
    fn hydrate_falls_back_to_jwt_exp_when_stored_expiry_unparseable() {
        let exp = Utc::now().timestamp() + 3600;
        let jwt = test_jwt_with_exp(exp);
        let session = hydrate_access_token(Some(jwt), Some("nope".into())).unwrap();
        assert!((session.expires_at.timestamp() - exp).abs() <= 1);
        assert!(token_is_fresh(session.expires_at, TOKEN_EXPIRY_SKEW_SECS));
    }

    #[test]
    fn hydrate_skips_empty_token_even_if_connected_flag_would_be_true() {
        assert!(hydrate_access_token(None, Some(future_rfc3339())).is_none());
        assert!(hydrate_access_token(Some("".into()), Some(future_rfc3339())).is_none());
    }

    #[test]
    fn preload_then_get_secret_restores_after_restart() {
        with_isolated_vault(|| {
            set_secret(SECRET_WEALTH_TOKEN, "restart-token").unwrap();
            set_secret(SECRET_WEALTH_TOKEN_EXPIRES, &future_rfc3339()).unwrap();
            simulate_restart();
            assert_eq!(
                get_secret(SECRET_WEALTH_TOKEN).unwrap().as_deref(),
                Some("restart-token")
            );
            assert!(secret_present(SECRET_WEALTH_TOKEN));
        });
    }

    #[test]
    fn missing_vault_has_no_tokens() {
        with_isolated_vault(|| {
            simulate_restart();
            assert!(!secret_present(SECRET_WEALTH_TOKEN));
            assert!(!secret_present(SECRET_BAMBOO_TOKEN));
            assert!(hydrate_access_token(
                get_secret(SECRET_WEALTH_TOKEN).ok().flatten(),
                get_secret(SECRET_WEALTH_TOKEN_EXPIRES).ok().flatten(),
            )
            .is_none());
        });
    }

    #[test]
    fn expired_hydrate_is_not_fresh() {
        let session = hydrate_access_token(Some("old".into()), Some(past_rfc3339())).unwrap();
        assert!(!token_is_fresh(session.expires_at, TOKEN_EXPIRY_SKEW_SECS));
    }

    #[test]
    fn hydrate_prefers_fresh_jwt_when_stored_expiry_is_ttl_minutes() {
        let exp = Utc::now().timestamp() + 3600;
        let jwt = test_jwt_with_exp(exp);
        let session = hydrate_access_token(Some(jwt), Some("60".into())).unwrap();
        assert!(token_unexpired(session.expires_at));
        assert!((session.expires_at.timestamp() - exp).abs() <= 1);
    }

    #[test]
    fn preload_reloads_after_empty_in_memory_poison() {
        with_isolated_vault(|| {
            set_secret(SECRET_WEALTH_TOKEN, "still-here").unwrap();
            set_secret(SECRET_WEALTH_TOKEN_EXPIRES, &future_rfc3339()).unwrap();
            poison_empty_memory_keep_disk();
            assert!(!secret_present(SECRET_WEALTH_TOKEN));
            preload();
            assert_eq!(
                get_secret(SECRET_WEALTH_TOKEN).unwrap().as_deref(),
                Some("still-here")
            );
            let session = load_stored_session(SECRET_WEALTH_TOKEN, SECRET_WEALTH_TOKEN_EXPIRES);
            assert!(session.is_some());
            assert!(token_unexpired(session.unwrap().expires_at));
        });
    }
}
