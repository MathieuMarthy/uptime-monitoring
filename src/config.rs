use std::net::SocketAddr;
use url::Url;

const DEFAULT_DATABASE_URL: &str = "sqlite://uptime.db";
const DEFAULT_BIND_ADDR: &str = "0.0.0.0:8080";

#[derive(Debug)]
pub struct Config {
    pub database_url: String,
    pub bind_addr: SocketAddr,
    pub trust_proxy_headers: bool,
    /// Where users reach the instance, without a trailing `/`: links in alerts and the setup URL.
    pub public_url: Option<String>,
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

        let public_url = match get("PUBLIC_URL").as_deref().map(str::trim) {
            None | Some("") => None,
            Some(raw) => Some(public_url(raw).ok_or_else(|| {
                format!(
                    "PUBLIC_URL must be an http(s) URL such as https://status.example.com, without path, \
                     query, fragment or credentials, got {raw:?}"
                )
            })?),
        };

        Ok(Self {
            database_url,
            bind_addr,
            trust_proxy_headers,
            public_url,
        })
    }
}

fn public_url(raw: &str) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    let plain = matches!(url.scheme(), "http" | "https")
        && url.host().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    plain.then(|| url.as_str().trim_end_matches('/').to_string())
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
    fn public_url_is_an_optional_http_base() {
        let with = |value: &'static str| {
            Config::from_lookup(move |key| (key == "PUBLIC_URL").then(|| value.to_string()))
        };
        assert_eq!(Config::from_lookup(|_| None).unwrap().public_url, None);
        assert_eq!(with("").unwrap().public_url, None);
        assert_eq!(
            with("https://status.example.com/")
                .unwrap()
                .public_url
                .as_deref(),
            Some("https://status.example.com")
        );
        assert_eq!(
            with("http://10.0.0.2:8080").unwrap().public_url.as_deref(),
            Some("http://10.0.0.2:8080")
        );
        for bad in [
            "status.example.com",
            "ftp://example.com",
            "https://example.com/?a=1",
            "https://example.com/#x",
            "https://user:pw@example.com",
            // Routes and assets are root-absolute: no sub-path deployment.
            "https://example.com/uptime",
        ] {
            assert!(with(bad).unwrap_err().contains("PUBLIC_URL"), "{bad}");
        }
    }

    #[test]
    fn invalid_bind_addr_is_error() {
        let err = Config::from_lookup(|key| (key == "BIND_ADDR").then(|| "nope".to_string()))
            .unwrap_err();
        assert!(err.contains("BIND_ADDR"));
    }
}
