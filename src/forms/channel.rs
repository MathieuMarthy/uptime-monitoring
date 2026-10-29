use crate::forms::has_visible_char;
use crate::forms::monitor::{FieldErrors, validate_url};
use crate::notify::channel::{ChannelConfig, KINDS, NtfyConfig, WebhookConfig};
use serde::Deserialize;

const NAME_MAX_CHARS: usize = 100;
const TOPIC_MAX_CHARS: usize = 64;
const TOKEN_MAX_BYTES: usize = 256;
const DISCORD_PREFIXES: [&str; 2] = [
    "https://discord.com/api/webhooks/",
    "https://discordapp.com/api/webhooks/",
];
pub const NTFY_DEFAULT_SERVER: &str = "https://ntfy.sh";

/// Raw channel form, as submitted. The fields of every kind are sent; only those of the
/// selected kind are read.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ChannelForm {
    pub name: String,
    pub kind: String,
    /// Webhook or Discord URL.
    pub url: String,
    pub ntfy_server: String,
    pub ntfy_topic: String,
    /// Blank keeps the stored token; it is never sent back to the browser.
    pub ntfy_token: String,
    pub ntfy_clear_token: Option<String>,
    pub enabled: Option<String>,
}

/// A validated channel, ready to be stored.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelInput {
    pub name: String,
    pub config: ChannelConfig,
    pub enabled: bool,
}

impl ChannelForm {
    pub fn new_defaults() -> Self {
        Self {
            kind: "webhook".to_string(),
            ntfy_server: NTFY_DEFAULT_SERVER.to_string(),
            enabled: Some("on".to_string()),
            ..Self::default()
        }
    }

    /// The form of a stored channel. The ntfy token is left out.
    pub fn from_config(name: &str, config: &ChannelConfig, enabled: bool) -> Self {
        let mut form = Self {
            name: name.to_string(),
            kind: config.kind().to_string(),
            enabled: enabled.then(|| "on".to_string()),
            ..Self::new_defaults()
        };
        match config {
            ChannelConfig::Webhook(c) | ChannelConfig::Discord(c) => form.url = c.url.clone(),
            ChannelConfig::Ntfy(c) => {
                form.ntfy_server = c.server.clone();
                form.ntfy_topic = c.topic.clone();
            }
        }
        form
    }

    /// `existing` is the stored config when editing: its ntfy token is kept unless replaced or
    /// cleared.
    pub fn validate(&self, existing: Option<&ChannelConfig>) -> Result<ChannelInput, FieldErrors> {
        let mut errors = FieldErrors::new();

        let name = self.name.trim();
        if !has_visible_char(name) {
            errors.insert("name", "Name is required.".to_string());
        } else if name.chars().count() > NAME_MAX_CHARS {
            errors.insert(
                "name",
                format!("Name must be at most {NAME_MAX_CHARS} characters."),
            );
        }

        let config = match self.kind.as_str() {
            "webhook" => self.webhook_url(&mut errors).map(ChannelConfig::Webhook),
            "discord" => self.discord_url(&mut errors).map(ChannelConfig::Discord),
            "ntfy" => self.ntfy(existing, &mut errors).map(ChannelConfig::Ntfy),
            _ => {
                errors.insert("kind", format!("Choose one of: {}.", KINDS.join(", ")));
                None
            }
        };

        match config {
            Some(config) if errors.is_empty() => Ok(ChannelInput {
                name: name.to_string(),
                config,
                enabled: self.enabled.is_some(),
            }),
            _ => Err(errors),
        }
    }

    fn webhook_url(&self, errors: &mut FieldErrors) -> Option<WebhookConfig> {
        let url = self.url.trim();
        match validate_url(url) {
            Ok(()) => Some(WebhookConfig {
                url: url.to_string(),
            }),
            Err(message) => {
                errors.insert("url", message);
                None
            }
        }
    }

    fn discord_url(&self, errors: &mut FieldErrors) -> Option<WebhookConfig> {
        let config = self.webhook_url(errors)?;
        if DISCORD_PREFIXES.iter().any(|p| config.url.starts_with(p)) {
            Some(config)
        } else {
            errors.insert(
                "url",
                format!("A Discord webhook URL starts with {}.", DISCORD_PREFIXES[0]),
            );
            None
        }
    }

    fn ntfy(
        &self,
        existing: Option<&ChannelConfig>,
        errors: &mut FieldErrors,
    ) -> Option<NtfyConfig> {
        let server = self.ntfy_server.trim().trim_end_matches('/');
        let server = if server.is_empty() {
            NTFY_DEFAULT_SERVER
        } else {
            server
        };
        if let Err(message) = validate_url(server) {
            errors.insert("ntfy_server", message);
        }

        let topic = self.ntfy_topic.trim();
        let topic_ok = !topic.is_empty()
            && topic.chars().count() <= TOPIC_MAX_CHARS
            && topic
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        if !topic_ok {
            errors.insert(
                "ntfy_topic",
                format!("Topic is required: up to {TOPIC_MAX_CHARS} letters, digits, - or _."),
            );
        }

        let typed = self.ntfy_token.trim();
        let token = if !typed.is_empty() {
            let valid =
                typed.len() <= TOKEN_MAX_BYTES && typed.bytes().all(|b| b.is_ascii_graphic());
            if !valid {
                errors.insert(
                    "ntfy_token",
                    format!("Token must be at most {TOKEN_MAX_BYTES} characters, without spaces."),
                );
            }
            Some(typed.to_string())
        } else if self.ntfy_clear_token.is_some() {
            None
        } else {
            match existing {
                Some(ChannelConfig::Ntfy(c)) => c.token.clone(),
                _ => None,
            }
        };

        errors.is_empty().then(|| NtfyConfig {
            server: server.to_string(),
            topic: topic.to_string(),
            token,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(kind: &str) -> ChannelForm {
        ChannelForm {
            name: "Ops".to_string(),
            kind: kind.to_string(),
            ..ChannelForm::new_defaults()
        }
    }

    fn ntfy_config(token: Option<&str>) -> ChannelConfig {
        ChannelConfig::Ntfy(NtfyConfig {
            server: "https://ntfy.sh".to_string(),
            topic: "alerts".to_string(),
            token: token.map(str::to_string),
        })
    }

    #[test]
    fn webhook_needs_an_http_url() {
        let mut f = form("webhook");
        f.url = "ftp://example.com".to_string();
        assert!(f.validate(None).unwrap_err().contains_key("url"));
        f.url = " https://hooks.example.com/x ".to_string();
        let input = f.validate(None).unwrap();
        assert_eq!(
            input.config,
            ChannelConfig::Webhook(WebhookConfig {
                url: "https://hooks.example.com/x".to_string()
            })
        );
        assert!(input.enabled);
    }

    #[test]
    fn discord_needs_a_discord_webhook_url() {
        let mut f = form("discord");
        f.url = "https://example.com/api/webhooks/1/x".to_string();
        assert!(f.validate(None).unwrap_err().contains_key("url"));
        f.url = "https://discord.com/api/webhooks/1/x".to_string();
        assert!(f.validate(None).is_ok());
    }

    #[test]
    fn ntfy_checks_server_and_topic() {
        let mut f = form("ntfy");
        f.ntfy_server = String::new();
        f.ntfy_topic = "bad topic".to_string();
        let errors = f.validate(None).unwrap_err();
        assert!(errors.contains_key("ntfy_topic"));
        assert!(!errors.contains_key("ntfy_server"), "blank means ntfy.sh");

        f.ntfy_topic = "alerts".to_string();
        f.ntfy_server = "https://ntfy.example.com/".to_string();
        let input = f.validate(None).unwrap();
        assert_eq!(
            input.config,
            ChannelConfig::Ntfy(NtfyConfig {
                server: "https://ntfy.example.com".to_string(),
                topic: "alerts".to_string(),
                token: None,
            })
        );
    }

    #[test]
    fn blank_ntfy_token_keeps_the_stored_one_unless_cleared() {
        let mut f = form("ntfy");
        f.ntfy_topic = "alerts".to_string();
        let stored = ntfy_config(Some("tk_old"));
        assert_eq!(f.validate(Some(&stored)).unwrap().config, stored);

        f.ntfy_token = "tk_new".to_string();
        assert_eq!(
            f.validate(Some(&stored)).unwrap().config,
            ntfy_config(Some("tk_new"))
        );

        f.ntfy_token = String::new();
        f.ntfy_clear_token = Some("on".to_string());
        assert_eq!(f.validate(Some(&stored)).unwrap().config, ntfy_config(None));

        f.ntfy_clear_token = None;
        f.ntfy_token = "two words".to_string();
        assert!(f.validate(None).unwrap_err().contains_key("ntfy_token"));
    }

    #[test]
    fn name_and_kind_are_required() {
        let mut f = form("email");
        f.name = " ".to_string();
        let errors = f.validate(None).unwrap_err();
        assert!(errors.contains_key("name"));
        assert!(errors.contains_key("kind"));
    }

    #[test]
    fn stored_form_omits_the_token() {
        let f = ChannelForm::from_config("Ops", &ntfy_config(Some("tk")), false);
        assert_eq!(f.ntfy_token, "");
        assert_eq!(f.ntfy_topic, "alerts");
        assert_eq!(f.enabled, None);
    }
}
