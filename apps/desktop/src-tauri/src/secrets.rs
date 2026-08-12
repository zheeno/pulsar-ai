use anyhow::{Context, Result};
use keyring::Entry;
use serde::{Deserialize, Serialize};

const SERVICE: &str = "com.pulsar.ai";

pub fn set_secret(key: &str, value: &str) -> Result<()> {
    let entry = Entry::new(SERVICE, key).context("keyring entry")?;
    entry.set_password(value).context("set keyring password")
}

pub fn get_secret(key: &str) -> Result<Option<String>> {
    let entry = Entry::new(SERVICE, key).context("keyring entry")?;
    match entry.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn delete_secret(key: &str) -> Result<()> {
    let entry = Entry::new(SERVICE, key).context("keyring entry")?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PulseSecrets {
    pub password: Option<String>,
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LlmSecrets {
    pub api_key: Option<String>,
}

pub const SECRET_PULSE_PASSWORD: &str = "pulse_password";
pub const SECRET_PULSE_API_KEY: &str = "pulse_api_key";
pub const SECRET_LLM_API_KEY: &str = "llm_api_key";
pub const SECRET_WEALTH_PASSWORD: &str = "wealth_password";
pub const SECRET_WEALTH_TOKEN: &str = "wealth_token";
pub const SECRET_WEALTH_TOKEN_EXPIRES: &str = "wealth_token_expires";
pub const SECRET_WEALTH_REFRESH_TOKEN: &str = "wealth_refresh_token";
