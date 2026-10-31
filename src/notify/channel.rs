//! The configuration of each kind of channel, and the HTTP request each one sends.

use crate::notify::Notification;
use crate::notify::message::{Message, Severity};
use reqwest::{Client, RequestBuilder};
use serde::{Deserialize, Serialize};
use serde_json::json;

pub const KINDS: [&str; 4] = ["webhook", "discord", "ntfy", "telegram"];

/// The public Bot API server.
pub const TELEGRAM_API: &str = "https://api.telegram.org";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WebhookConfig {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NtfyConfig {
    /// Base URL of the ntfy server, e.g. `https://ntfy.sh`.
    pub server: String,
    pub topic: String,
    /// Access token of a protected topic.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TelegramConfig {
    /// Bot API server. Only the public one is offered; tests point it elsewhere.
    #[serde(default = "telegram_api", skip_serializing_if = "is_telegram_api")]
    pub server: String,
    /// Bot token from @BotFather, e.g. `123456:ABC-…`.
    pub token: String,
    /// Numeric id of a chat, group or channel, or `@channelname`.
    pub chat_id: String,
}

fn telegram_api() -> String {
    TELEGRAM_API.to_string()
}

fn is_telegram_api(server: &str) -> bool {
    server == TELEGRAM_API
}

/// Stored as `notification_channels.kind` plus the JSON of the inner config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelConfig {
    /// A POST of a JSON document to any URL.
    Webhook(WebhookConfig),
    /// A Discord webhook URL.
    Discord(WebhookConfig),
    Ntfy(NtfyConfig),
    /// A message sent by a bot to a chat.
    Telegram(TelegramConfig),
}

impl ChannelConfig {
    pub fn parse(kind: &str, config: &str) -> Result<Self, serde_json::Error> {
        use serde::de::Error;
        match kind {
            "webhook" => serde_json::from_str(config).map(Self::Webhook),
            "discord" => serde_json::from_str(config).map(Self::Discord),
            "ntfy" => serde_json::from_str(config).map(Self::Ntfy),
            "telegram" => serde_json::from_str(config).map(Self::Telegram),
            other => Err(serde_json::Error::custom(format!(
                "unknown channel kind {other:?}"
            ))),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Webhook(_) => "webhook",
            Self::Discord(_) => "discord",
            Self::Ntfy(_) => "ntfy",
            Self::Telegram(_) => "telegram",
        }
    }

    /// Webhooks are read by programs, which expect one event per monitor.
    pub fn can_group(&self) -> bool {
        !matches!(self, Self::Webhook(_))
    }

    pub fn to_json(&self) -> String {
        match self {
            Self::Webhook(c) | Self::Discord(c) => json!(c),
            Self::Ntfy(c) => json!(c),
            Self::Telegram(c) => json!(c),
        }
        .to_string()
    }

    /// Where the request goes. Telegram's URL carries the bot token.
    fn url(&self) -> String {
        match self {
            Self::Webhook(c) | Self::Discord(c) => c.url.clone(),
            Self::Ntfy(c) => c.server.clone(),
            Self::Telegram(c) => format!(
                "{}/bot{}/sendMessage",
                c.server.trim_end_matches('/'),
                c.token
            ),
        }
    }

    pub fn request(
        &self,
        client: &Client,
        notification: &Notification,
        message: &Message,
    ) -> RequestBuilder {
        let mut body = match self {
            Self::Webhook(_) => json!({
                "event": notification.event.as_str(),
                "monitor": notification.monitor.as_ref().map(|m| json!({
                    "id": m.id,
                    "name": m.name,
                    "url": m.url,
                    "link": message.link,
                })),
                "title": message.title,
                "message": message.text,
                "at": notification.at,
            }),
            Self::Discord(_) => json!({
                "username": "Uptime Monitoring",
                "embeds": [{
                    "title": message.title,
                    "description": message.text,
                    "color": discord_color(message.severity),
                }],
            }),
            Self::Ntfy(c) => json!({
                "topic": c.topic,
                "title": message.title,
                "message": message.text,
                "priority": if message.severity == Severity::Critical { 4 } else { 3 },
                "tags": [ntfy_tag(message.severity)],
            }),
            Self::Telegram(c) => json!({
                "chat_id": c.chat_id,
                "text": telegram_text(message),
                "link_preview_options": { "is_disabled": true },
            }),
        };
        if let Some(link) = &message.link {
            match self {
                Self::Webhook(_) | Self::Telegram(_) => {}
                Self::Discord(_) => body["embeds"][0]["url"] = json!(link),
                Self::Ntfy(_) => body["click"] = json!(link),
            }
        }
        let mut request = client
            .post(self.url())
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(body.to_string());
        if let Self::Ntfy(NtfyConfig {
            token: Some(token), ..
        }) = self
        {
            request = request.bearer_auth(token);
        }
        request
    }
}

fn discord_color(severity: Severity) -> u32 {
    match severity {
        Severity::Critical => 0xE5484D,
        Severity::Resolved => 0x30A46C,
        Severity::Warning => 0xF5A524,
        Severity::Info => 0x3E63DD,
    }
}

/// An emoji shortcode, shown by ntfy before the title.
fn ntfy_tag(severity: Severity) -> &'static str {
    match severity {
        Severity::Critical => "rotating_light",
        Severity::Resolved => "white_check_mark",
        Severity::Warning => "warning",
        Severity::Info => "bell",
    }
}

/// Telegram refuses a text over 4,096 characters; this leaves room for how it counts emoji.
const TELEGRAM_TEXT_MAX_CHARS: usize = 4_000;

/// Plain text, so that nothing in a monitor name or cause needs escaping. A long message is cut
/// so that the link stays.
fn telegram_text(message: &Message) -> String {
    let emoji = match message.severity {
        Severity::Critical => "🚨",
        Severity::Resolved => "✅",
        Severity::Warning => "⚠️",
        Severity::Info => "🔔",
    };
    let head = format!("{emoji} {}\n\n", message.title);
    let tail = message
        .link
        .as_ref()
        .map_or(String::new(), |link| format!("\n\n{link}"));
    let room = TELEGRAM_TEXT_MAX_CHARS.saturating_sub(head.chars().count() + tail.chars().count());
    let body = if message.text.chars().count() > room {
        let mut cut: String = message.text.chars().take(room.saturating_sub(1)).collect();
        cut.push('…');
        cut
    } else {
        message.text.clone()
    };
    format!("{head}{body}{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_round_trips_through_its_kind_and_json() {
        let configs = [
            ChannelConfig::Webhook(WebhookConfig {
                url: "https://hooks.example.com/x".to_string(),
            }),
            ChannelConfig::Discord(WebhookConfig {
                url: "https://discord.com/api/webhooks/1/abc".to_string(),
            }),
            ChannelConfig::Ntfy(NtfyConfig {
                server: "https://ntfy.sh".to_string(),
                topic: "alerts".to_string(),
                token: Some("tk_secret".to_string()),
            }),
            ChannelConfig::Telegram(TelegramConfig {
                server: TELEGRAM_API.to_string(),
                token: "123:ABC".to_string(),
                chat_id: "-1001".to_string(),
            }),
        ];
        for config in configs {
            assert_eq!(
                ChannelConfig::parse(config.kind(), &config.to_json()).unwrap(),
                config
            );
        }
        assert!(ChannelConfig::parse("email", "{}").is_err());
    }

    #[test]
    fn telegram_text_fits_in_one_message() {
        let message = Message {
            title: "API is down".to_string(),
            text: "é".repeat(5_000),
            severity: Severity::Critical,
            link: Some("https://up.example.com/monitors/1".to_string()),
        };
        let text = telegram_text(&message);
        assert_eq!(text.chars().count(), TELEGRAM_TEXT_MAX_CHARS);
        assert!(
            text.ends_with("…\n\nhttps://up.example.com/monitors/1"),
            "the link is kept"
        );
    }

    #[test]
    fn telegram_server_defaults_to_the_bot_api_and_is_not_stored() {
        let config =
            ChannelConfig::parse("telegram", r#"{"token":"1:A","chat_id":"@ops"}"#).unwrap();
        assert!(matches!(&config, ChannelConfig::Telegram(c) if c.server == TELEGRAM_API));
        assert!(!config.to_json().contains("server"));
    }
}
