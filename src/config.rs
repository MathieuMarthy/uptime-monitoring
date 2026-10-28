use std::net::SocketAddr;

const DEFAULT_DATABASE_URL: &str = "sqlite://uptime.db";
const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";

#[derive(Debug)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    pub trust_proxy_headers: bool,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let database_url = get("DATABASE_URL").unwrap_or_else(|| DEFAULT_DATABASE_URL.to_string());
        let bind_addr = get("BIND_ADDR").unwrap_or_else(|| DEFAULT_BIND_ADDR.to_string());
        let bind_addr = bind_addr.parse().map_err(|_| {
            format!("BIND_ADDR must be an address like {DEFAULT_BIND_ADDR}, got {bind_addr:?}")
        })?;

        let trust_proxy_headers = match get("TRUST_PROXY_HEADERS").as_deref() {
            None | Some("false") | Some("0") => false,
            Some("true") | Some("1") => true,
            Some(other) => {
                return Err(format!(
                    "TRUST_PROXY_HEADERS must be true or false, got {other:?}"
                ));
            }
        };

        Ok(Self {
            database_url,
            bind_addr,
            trust_proxy_headers,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_when_unset() {
        let config = Config::from_lookup(|_| None).unwrap();
        assert_eq!(config.database_url, "sqlite://uptime.db");
        assert_eq!(config.bind_addr.to_string(), "0.0.0.0:8080");
        assert!(!config.trust_proxy_headers);
    }

    #[test]
    fn trust_proxy_headers_is_a_boolean() {
        let with = |value: &'static str| {
            Config::from_lookup(move |key| {
                (key == "TRUST_PROXY_HEADERS").then(|| value.to_string())
            })
        };
        assert!(with("true").unwrap().trust_proxy_headers);
        assert!(!with("0").unwrap().trust_proxy_headers);
        assert!(with("yes").unwrap_err().contains("TRUST_PROXY_HEADERS"));
    }

    #[test]
    fn invalid_bind_addr_is_error() {
        let err = Config::from_lookup(|key| (key == "BIND_ADDR").then(|| "nope".to_string()))
            .unwrap_err();
        assert!(err.contains("BIND_ADDR"));
    }
}
