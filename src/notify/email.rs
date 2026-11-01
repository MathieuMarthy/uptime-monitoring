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
use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
use std::time::Duration;

/// A whole delivery, from the connection to `QUIT`.
const MAIL_TIMEOUT: Duration = Duration::from_secs(30);

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

/// A subject never spans lines.
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub(crate) async fn send(
    guard: &Guard,
    config: &EmailConfig,
    message: &Message,
) -> Result<(), SendError> {
    let mail = build(config, message).map_err(|message| SendError {
        message,
        retry: false,
    })?;
    let addr = guard
        .resolve_host(&config.host)
        .await
        .map_err(|e| SendError {
            retry: matches!(e, GuardError::Dns(_)),
            message: e.to_string(),
        })?[0];
    let tls = match config.security {
        SmtpSecurity::None => Tls::None,
        security => {
            let parameters = TlsParameters::new(config.host.clone()).map_err(|e| SendError {
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
    let mut transport =
        AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(addr.ip().to_string())
            .port(config.port)
            .tls(tls)
            .timeout(Some(SEND_TIMEOUT));
    if let (Some(username), Some(password)) = (&config.username, &config.password) {
        transport = transport.credentials(Credentials::new(username.clone(), password.clone()));
    }
    let transport = transport.build();
    match tokio::time::timeout(MAIL_TIMEOUT, transport.send(mail)).await {
        Ok(Ok(_)) => Ok(()),
        Ok(Err(e)) => Err(SendError {
            message: truncate(&error_chain(&e), ERROR_MAX_CHARS),
            retry: !e.is_permanent(),
        }),
        Err(_) => Err(SendError {
            message: "timed out".to_string(),
            retry: true,
        }),
    }
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
