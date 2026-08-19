use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::capture::bearer_token;
use super::config::BrokerAuthConfig;
use super::store::{self, StoredCredential};

pub const EXPIRED_EVENT: &str = "auth-bridge:expired";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatusKind {
    Disconnected,
    Connecting,
    Connected,
    Expired,
    Cancelled,
    TimedOut,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthSessionStatus {
    pub broker_id: String,
    pub display_name: String,
    pub status: SessionStatusKind,
    pub expires_at: Option<DateTime<Utc>>,
    pub account_hint: Option<String>,
}

pub fn status_for(config: &BrokerAuthConfig) -> AuthSessionStatus {
    match store::get(&config.id).ok().flatten() {
        Some(cred) if crate::secrets::token_is_fresh(cred.expires_at, crate::secrets::TOKEN_EXPIRY_SKEW_SECS) => {
            AuthSessionStatus {
                broker_id: config.id.clone(),
                display_name: config.display_name.clone(),
                status: SessionStatusKind::Connected,
                expires_at: Some(cred.expires_at),
                account_hint: cred.account_hint,
            }
        }
        Some(cred) => AuthSessionStatus {
            broker_id: config.id.clone(),
            display_name: config.display_name.clone(),
            status: SessionStatusKind::Expired,
            expires_at: Some(cred.expires_at),
            account_hint: cred.account_hint,
        },
        None => AuthSessionStatus {
            broker_id: config.id.clone(),
            display_name: config.display_name.clone(),
            status: SessionStatusKind::Disconnected,
            expires_at: None,
            account_hint: None,
        },
    }
}

pub fn expiry_for(config: &BrokerAuthConfig, token_value: &str) -> DateTime<Utc> {
    let raw = bearer_token(token_value);
    crate::secrets::jwt_expiry(raw)
        .filter(|exp| *exp > Utc::now())
        .unwrap_or_else(|| Utc::now() + chrono::Duration::seconds(config.fallback_ttl_secs as i64))
}

pub fn persist_valid(
    config: &BrokerAuthConfig,
    header_name: &str,
    value: &str,
    account_hint: Option<String>,
) -> anyhow::Result<AuthSessionStatus> {
    let expires_at = expiry_for(config, value);
    let cred = StoredCredential {
        kind: "header".into(),
        header_name: header_name.to_string(),
        token: value.to_string(),
        expires_at,
        account_hint,
    };
    store::put(&config.id, &cred)?;
    tracing::info!(
        target: "auth_bridge",
        broker = %config.id,
        expires_at = %expires_at,
        hint_present = cred.account_hint.is_some(),
        "session stored"
    );
    Ok(status_for(config))
}

#[allow(dead_code)]
pub fn get_token(broker_id: &str) -> anyhow::Result<Option<String>> {
    let Some(cred) = store::get(broker_id)? else {
        return Ok(None);
    };
    if crate::secrets::token_is_fresh(cred.expires_at, crate::secrets::TOKEN_EXPIRY_SKEW_SECS) {
        Ok(Some(cred.token))
    } else {
        Ok(None)
    }
}

pub fn revoke(broker_id: &str) -> anyhow::Result<()> {
    store::delete(broker_id)?;
    tracing::info!(target: "auth_bridge", broker = %broker_id, "session revoked");
    Ok(())
}

pub fn revoke_all() {
    if let Ok(cfgs) = super::config::builtin_configs() {
        let ids: Vec<&str> = cfgs.iter().map(|c| c.id.as_str()).collect();
        store::delete_all(&ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_jwt(exp: i64) -> String {
        let header = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            br#"{"alg":"none"}"#,
        );
        let payload = base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            format!(r#"{{"exp":{exp}}}"#).as_bytes(),
        );
        format!("Bearer {header}.{payload}.x")
    }

    #[test]
    fn jwt_expiry_preferred() {
        let cfg = crate::auth_bridge::config::get("busha").unwrap();
        let exp = Utc::now() + chrono::Duration::hours(2);
        let token = fake_jwt(exp.timestamp());
        let got = expiry_for(&cfg, &token);
        assert!((got - exp).num_seconds().abs() < 3);
    }
}
