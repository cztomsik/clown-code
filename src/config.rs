//! Application configuration.
//!
//! Mirrors `Config` + `EnvOverrides` from `src/main.zig`.

pub struct Config {
    /// Base URL of the OpenAI-compatible LLM server.
    pub base_url: String,
    /// Request timeout (15 minutes, as in the Zig original).
    pub timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self::default_local()
    }
}

impl Config {
    pub fn default_local() -> Self {
        Self {
            base_url: "http://127.0.0.1:8080".into(),
            timeout_secs: 15 * 60,
        }
    }

    /// Apply environment variable overrides (`CLOWN_API` for the base URL).
    pub fn from_env() -> Self {
        let mut config = Self::default_local();
        if let Ok(url) = std::env::var("CLOWN_API") {
            config.base_url = url;
        }
        config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let c = Config::default_local();
        assert_eq!(c.base_url, "http://127.0.0.1:8080");
        assert_eq!(c.timeout_secs, 900);
    }
}
