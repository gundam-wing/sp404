//! Process configuration. The API key is required; the limits have defaults.

use crate::error::err;

/// Defaults and the required API key.
#[derive(Debug, Clone)]
pub struct Config {
    /// Bearer token clients must send. Never logged.
    pub api_key: String,
    /// `SP404_MAX_BARS`. Default 64.
    pub max_bars: u32,
    /// `SP404_MAX_UPLOAD_BYTES`. Default 20 MiB.
    pub max_upload_bytes: usize,
    /// `SP404_MAX_CONCURRENT_JOBS`. Default 2. Zero accepts no jobs.
    pub max_concurrent_jobs: usize,
    /// `SP404_BIND`. Default `127.0.0.1:4040`.
    pub bind: String,
}

/// Read configuration from the environment.
///
/// `SP404_API_KEY` must be set and non-empty. A missing limit uses the default.
/// A limit that is present but not an integer is a startup error.
pub fn config_from_env() -> Result<Config, String> {
    let api_key = std::env::var("SP404_API_KEY").unwrap_or_default();
    if api_key.is_empty() {
        return Err("SP404_API_KEY is required".into());
    }
    let max_bars = env_u64("SP404_MAX_BARS", 64)?;
    if max_bars == 0 || max_bars > u64::from(u32::MAX) {
        return Err("SP404_MAX_BARS must be from 1 to 4294967295".into());
    }
    let max_upload_bytes = env_u64("SP404_MAX_UPLOAD_BYTES", 20 * 1024 * 1024)?;
    if max_upload_bytes == 0 {
        return Err("SP404_MAX_UPLOAD_BYTES must be at least 1".into());
    }
    let max_concurrent_jobs = env_u64("SP404_MAX_CONCURRENT_JOBS", 2)?;
    let bind = match std::env::var("SP404_BIND") {
        Ok(value) if value.is_empty() => {
            return Err("SP404_BIND is empty".into());
        }
        Ok(value) => value,
        Err(_) => "127.0.0.1:4040".to_string(),
    };
    Ok(Config {
        api_key,
        max_bars: max_bars as u32,
        max_upload_bytes: max_upload_bytes as usize,
        max_concurrent_jobs: max_concurrent_jobs as usize,
        bind,
    })
}

fn env_u64(name: &str, default: u64) -> Result<u64, String> {
    match std::env::var(name) {
        Ok(value) => value
            .parse()
            .map_err(|_| err(format!("{name} must be an integer"))),
        Err(_) => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::config_from_env;

    #[test]
    fn missing_api_key_refuses_to_boot() {
        let previous = std::env::var("SP404_API_KEY").ok();
        std::env::remove_var("SP404_API_KEY");
        let result = config_from_env();
        if let Some(value) = previous {
            std::env::set_var("SP404_API_KEY", value);
        }
        assert!(result.is_err(), "expected a missing key to fail");
        assert!(result.unwrap_err().contains("SP404_API_KEY"));
    }
}
