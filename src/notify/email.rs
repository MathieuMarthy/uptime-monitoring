//! E-mail channels: a plain-text mail sent through the user's SMTP relay.
//!
//! The relay's name is resolved through the guard and the connection goes to the address it
//! allows, so a private relay is refused like any other private target; TLS still checks the
//! certificate against the name.

use crate::engine::guard::{Guard, GuardError};
use crate::notify::channel::{EmailConfig, SmtpSecurity};
use crate::notify::message::Message;
use crate::notify::{ERROR_MAX_CHARS, SEND_TIMEOUT, SendError, error_chain, truncate};
use lettre::message::header::ContentType;
use lettre::message::{Mailbox, Message as Mail};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::transport::smtp::extension::ClientId;
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

/// A whole delivery, from the connection to `QUIT`.
const MAIL_TIMEOUT: Duration = Duration::from_secs(30);
/// Relays whose TLS settings are kept; beyond, the cache starts over.
const TLS_CACHE_MAX: usize = 32;

/// The mail of `message`: its title as subject, its text and link as body.
pub fn build(config: &EmailConfig, message: &Message) -> Result<Mail, String> {
    let mailbox = |text: &str| {
        text.parse::<Mailbox>()
            .map_err(|e| format!("invalid address {text:?}: {e}"))
    };
    let from = mailbox(&config.from)?;
    // Relays usually add one, but a mail without it looks like spam.
    let message_id = format!(
        "<{}@{}>",
        &crate::auth::random_token()[..32],
        from.email.domain()
    );
    let mut builder = Mail::builder()
        .from(from)
        .message_id(Some(message_id))
        .subject(one_line(&message.title))
        .header(ContentType::TEXT_PLAIN);
    for to in &config.to {
        builder = builder.to(mailbox(to)?);
    }
    let mut body = message.text.clone();
    if let Some(link) = &message.link {
        body.push_str("\n\n");
        body.push_str(link);
    }
    body.push('\n');
    builder.body(body).map_err(|e| e.to_string())
}

/// A list of addresses separated by commas, trimmed, empty ones dropped. A comma inside a quoted
/// display name (`"Doe, John" <john@example.com>`) or angle brackets does not split.
pub fn split_addresses(text: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let (mut quoted, mut escaped, mut bracket) = (false, false, false);
    for c in text.chars() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            '<' if !quoted => bracket = true,
            '>' if !quoted => bracket = false,
            ',' if !quoted && !bracket => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(c);
    }
    parts.push(current);
    parts
        .iter()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// A subject never spans lines.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// The name given with EHLO: the host of `PUBLIC_URL`, else lettre's default, the `[127.0.0.1]`
/// address literal (RFC 5321 allows a literal when the client has no name).
fn hello_name(public_url: Option<&str>) -> ClientId {
    match public_url
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| u.host().map(|h| h.to_owned()))
    {
        Some(url::Host::Domain(name)) => ClientId::Domain(name),
        Some(url::Host::Ipv4(ip)) => ClientId::Ipv4(ip),
        Some(url::Host::Ipv6(ip)) => ClientId::Ipv6(ip),
        None => ClientId::default(),
    }
}

/// Sends mails; keeps the TLS settings of each relay, whose certificate verifier is costly to
/// build.
#[derive(Default)]
pub(crate) struct Mailer {
    tls: Mutex<HashMap<String, TlsParameters>>,
}

impl Mailer {
    fn tls(&self, host: &str) -> Result<TlsParameters, lettre::transport::smtp::Error> {
        let mut cache = self.tls.lock().expect("TLS cache lock poisoned");
        if let Some(parameters) = cache.get(host) {
            return Ok(parameters.clone());
        }
        let parameters = TlsParameters::new(host.to_string())?;
        if cache.len() >= TLS_CACHE_MAX {
            cache.clear();
        }
        cache.insert(host.to_string(), parameters.clone());
        Ok(parameters)
    }

    pub(crate) async fn send(
        &self,
        guard: &Guard,
        public_url: Option<&str>,
        config: &EmailConfig,
        message: &Message,
    ) -> Result<(), SendError> {
        send(self, guard, hello_name(public_url), config, message).await
    }
}

async fn send(
    mailer: &Mailer,
    guard: &Guard,
    hello: ClientId,
    config: &EmailConfig,
    message: &Message,
) -> Result<(), SendError> {
    let mail = build(config, message).map_err(|message| SendError {
        message,
        retry: false,
    })?;
    let addrs = guard
        .resolve_host(&config.host)
        .await
        .map_err(|e| SendError {
            retry: matches!(e, GuardError::Dns(_)),
            message: e.to_string(),
        })?;
    let tls = match config.security {
        SmtpSecurity::None => Tls::None,
        security => {
            let parameters = mailer.tls(&config.host).map_err(|e| SendError {
                message: truncate(&error_chain(&e), ERROR_MAX_CHARS),
                retry: false,
            })?;
            if security == SmtpSecurity::Tls {
                Tls::Wrapper(parameters)
            } else {
                Tls::Required(parameters)
            }
        }
    };
    // Like a resolver's own loop: the next address only when this one never answered.
    let attempts = async {
        let mut last = None;
        for addr in &addrs {
            let mut transport =
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(addr.ip().to_string())
                    .port(config.port)
                    .tls(tls.clone())
                    .hello_name(hello.clone())
                    .timeout(Some(SEND_TIMEOUT));
            if let (Some(username), Some(password)) = (&config.username, &config.password) {
                transport =
                    transport.credentials(Credentials::new(username.clone(), password.clone()));
            }
            match transport.build().send(mail.clone()).await {
                Ok(_) => return Ok(()),
                Err(e) if never_answered(&e) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.expect("the guard returns at least one address"))
    };
    match tokio::time::timeout(MAIL_TIMEOUT, attempts).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(SendError {
            message: truncate(&error_chain(&e), ERROR_MAX_CHARS),
            // A refusal (5xx), a certificate or a missing STARTTLS or AUTH will not change.
            retry: !(e.is_permanent() || e.is_tls() || e.is_client()),
        }),
        Err(_) => Err(SendError {
            message: "timed out".to_string(),
            retry: true,
        }),
    }
}

/// The server never answered: refused, unreachable or silent.
fn never_answered(e: &lettre::transport::smtp::Error) -> bool {
    !(e.is_response() || e.is_tls() || e.is_client())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::message::Severity;

    fn config() -> EmailConfig {
        EmailConfig {
            host: "smtp.example.com".to_string(),
            port: 587,
            security: SmtpSecurity::Starttls,
            username: None,
            password: None,
            from: "Uptime <up@example.com>".to_string(),
            to: vec![
                "ops@example.com".to_string(),
                "Dev <dev@example.com>".to_string(),
            ],
        }
    }

    #[test]
    fn mail_has_the_title_as_subject_and_the_link_last() {
        let message = Message {
            title: "API\nis down".to_string(),
            text: "HTTP 503".to_string(),
            severity: Severity::Critical,
            link: Some("https://up.example.com/monitors/1".to_string()),
        };
        let mail = String::from_utf8(build(&config(), &message).unwrap().formatted()).unwrap();
        assert!(mail.contains("Subject: API is down\r\n"), "{mail}");
        let id = mail.split("Message-ID: <").nth(1).expect("a Message-ID");
        assert!(
            id.split('>').next().unwrap().ends_with("@example.com"),
            "{mail}"
        );
        assert!(
            mail.contains("To: ops@example.com, Dev <dev@example.com>"),
            "{mail}"
        );
        assert!(
            mail.ends_with("HTTP 503\r\n\r\nhttps://up.example.com/monitors/1\r\n"),
            "{mail}"
        );
    }

    #[test]
    fn addresses_split_on_commas_outside_quotes_and_brackets() {
        assert_eq!(
            split_addresses(r#" "Doe, John" <john@example.com>, ops@example.com ,, <a,b@x> "#),
            [
                r#""Doe, John" <john@example.com>"#,
                "ops@example.com",
                "<a,b@x>"
            ]
        );
        assert!(split_addresses(" , ").is_empty());
        assert_eq!(split_addresses(r#""a\", b" <c@d>"#), [r#""a\", b" <c@d>"#]);
    }

    #[test]
    fn tls_settings_are_built_once_per_host() {
        let mailer = Mailer::default();
        mailer.tls("smtp.example.com").unwrap();
        mailer.tls("smtp.example.com").unwrap();
        mailer.tls("mail.example.com").unwrap();
        assert_eq!(mailer.tls.lock().unwrap().len(), 2);
        for n in 0..TLS_CACHE_MAX {
            mailer.tls(&format!("h{n}.example.com")).unwrap();
        }
        assert!(mailer.tls.lock().unwrap().len() <= TLS_CACHE_MAX);
    }

    #[test]
    fn hello_name_comes_from_the_public_url() {
        assert_eq!(
            hello_name(Some("https://Up.Example.com:8443/x")).to_string(),
            "up.example.com"
        );
        assert_eq!(
            hello_name(Some("http://192.0.2.1")).to_string(),
            "[192.0.2.1]"
        );
        assert_eq!(hello_name(None).to_string(), "[127.0.0.1]");
    }

    #[test]
    fn a_bad_address_fails_without_retry() {
        let mut config = config();
        config.to = vec!["not an address".to_string()];
        let message = Message {
            title: "t".to_string(),
            text: "x".to_string(),
            severity: Severity::Info,
            link: None,
        };
        assert!(
            build(&config, &message)
                .unwrap_err()
                .contains("not an address")
        );
    }
}
