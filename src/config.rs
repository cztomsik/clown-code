//! Application configuration.

pub struct Config {
    /// Base URL of the OpenAI-compatible LLM server.
    pub base_url: String,
    /// Request timeout (15 minutes).
    pub timeout_secs: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8080".into(),
            timeout_secs: 15 * 60,
        }
    }
}

impl Config {
    /// Apply environment variable overrides (`CLOWN_API` for the base URL).
    pub fn from_env() -> Self {
        let mut config = Self::default();
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
        let c = Config::default();
        assert_eq!(c.base_url, "http://127.0.0.1:8080");
        assert_eq!(c.timeout_secs, 900);
    }
}
