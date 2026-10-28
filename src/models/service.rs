use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum ServiceStatus {
    Up,
    Down,
    Pending,
}

impl std::fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Up => write!(f, "Up"),
            Self::Down => write!(f, "Down"),
            Self::Pending => write!(f, "Pending"),
        }
    }
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct Service {
    pub id: String,
    pub name: String,
    pub url: String,
    pub method: String,
    pub interval_seconds: i64,
    pub timeout_seconds: i64,
    pub expected_status: i64,
    pub headers: Option<String>,
    pub body: Option<String>,
    pub keyword_match: Option<String>,
    pub max_retries: i64,
    pub is_enabled: bool,
    pub current_status: ServiceStatus,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateService {
    pub name: String,
    pub url: String,
    pub method: Option<String>,
    pub interval_seconds: Option<i64>,
    pub timeout_seconds: Option<i64>,
    pub expected_status: Option<i64>,
    pub headers: Option<String>,
    pub body: Option<String>,
    pub keyword_match: Option<String>,
    pub max_retries: Option<i64>,
    pub is_enabled: Option<bool>,
}

impl CreateService {
    #[allow(dead_code)]
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            method: None,
            interval_seconds: None,
            timeout_seconds: None,
            expected_status: None,
            headers: None,
            body: None,
            keyword_match: None,
            max_retries: None,
            is_enabled: Some(true),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditService {
    pub name: String,
    pub url: String,
    pub method: String,
    pub interval_seconds: i64,
    pub timeout_seconds: i64,
    pub expected_status: i64,
    pub headers: Option<String>,
    pub body: Option<String>,
    pub keyword_match: Option<String>,
    pub max_retries: i64,
    pub is_enabled: bool,
}

#[allow(dead_code)]
pub type UpdateService = EditService;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceFormInput {
    pub name: String,
    pub url: String,
    pub method: String,
    pub interval_seconds: i64,
    pub timeout_seconds: i64,
    pub expected_status: i64,
    pub headers: Option<String>,
    pub body: Option<String>,
    pub keyword_match: Option<String>,
    pub max_retries: i64,
    pub is_enabled: Option<String>,
}

impl ServiceFormInput {
    fn clean_string(val: Option<String>) -> Option<String> {
        val.and_then(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        let name = self.name.trim();
        if name.is_empty() {
            return Err("Service name cannot be empty.".to_string());
        }

        let url_str = self.url.trim();
        match url::Url::parse(url_str) {
            Ok(parsed) => {
                if parsed.scheme() != "http" && parsed.scheme() != "https" {
                    return Err("URL scheme must be http:// or https://".to_string());
                }
                match parsed.host_str() {
                    Some(host) if !host.trim().is_empty() => (),
                    _ => return Err(
                        "URL must include a valid host or domain name (e.g. https://example.com)."
                            .to_string(),
                    ),
                }
            }
            Err(_) => return Err(
                "Invalid URL format. Please enter a valid URL (e.g. https://example.com/health)."
                    .to_string(),
            ),
        }

        if self.interval_seconds < 5 {
            return Err("Interval must be at least 5 seconds.".to_string());
        }

        if self.timeout_seconds < 1 {
            return Err("Timeout must be at least 1 second.".to_string());
        }

        if self.timeout_seconds > self.interval_seconds {
            return Err("Timeout cannot be greater than the check interval.".to_string());
        }

        if !(100..=599).contains(&self.expected_status) {
            return Err("Expected status code must be a valid HTTP status (100-599).".to_string());
        }

        Ok(())
    }

    pub fn to_create_service(&self) -> CreateService {
        CreateService {
            name: self.name.trim().to_string(),
            url: self.url.trim().to_string(),
            method: Some(self.method.clone()),
            interval_seconds: Some(self.interval_seconds),
            timeout_seconds: Some(self.timeout_seconds),
            expected_status: Some(self.expected_status),
            headers: Self::clean_string(self.headers.clone()),
            body: Self::clean_string(self.body.clone()),
            keyword_match: Self::clean_string(self.keyword_match.clone()),
            max_retries: Some(self.max_retries),
            is_enabled: Some(self.is_enabled.is_some()),
        }
    }

    pub fn to_edit_service(&self) -> EditService {
        EditService {
            name: self.name.trim().to_string(),
            url: self.url.trim().to_string(),
            method: self.method.clone(),
            interval_seconds: self.interval_seconds,
            timeout_seconds: self.timeout_seconds,
            expected_status: self.expected_status,
            headers: Self::clean_string(self.headers.clone()),
            body: Self::clean_string(self.body.clone()),
            keyword_match: Self::clean_string(self.keyword_match.clone()),
            max_retries: self.max_retries,
            is_enabled: self.is_enabled.is_some(),
        }
    }

    pub fn to_dummy_service(&self, id: Option<String>) -> Service {
        Service {
            id: id.unwrap_or_default(),
            name: self.name.clone(),
            url: self.url.clone(),
            method: self.method.clone(),
            interval_seconds: self.interval_seconds,
            timeout_seconds: self.timeout_seconds,
            expected_status: self.expected_status,
            headers: Self::clean_string(self.headers.clone()),
            body: Self::clean_string(self.body.clone()),
            keyword_match: Self::clean_string(self.keyword_match.clone()),
            max_retries: self.max_retries,
            is_enabled: self.is_enabled.is_some(),
            current_status: ServiceStatus::Up,
            created_at: 0,
            updated_at: 0,
        }
    }
}

impl Service {
    pub async fn find_all(pool: &SqlitePool) -> Result<Vec<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT 
                id, name, url, method, interval_seconds, timeout_seconds, 
                expected_status, headers, body, keyword_match, max_retries, 
                is_enabled, current_status, created_at, updated_at
            FROM services 
            ORDER BY name ASC
            "#,
        )
        .fetch_all(pool)
        .await
    }

    pub async fn find_by_id(pool: &SqlitePool, id: &str) -> Result<Option<Self>, sqlx::Error> {
        sqlx::query_as::<_, Self>(
            r#"
            SELECT 
                id, name, url, method, interval_seconds, timeout_seconds, 
                expected_status, headers, body, keyword_match, max_retries, 
                is_enabled, current_status, created_at, updated_at
            FROM services 
            WHERE id = ?
            "#,
        )
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    pub async fn create(pool: &SqlitePool, data: CreateService) -> Result<Self, sqlx::Error> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let id = Uuid::new_v4().to_string();

        let method = data.method.unwrap_or_else(|| "GET".to_string());
        let interval_seconds = data.interval_seconds.unwrap_or(60);
        let timeout_seconds = data.timeout_seconds.unwrap_or(10);
        let expected_status = data.expected_status.unwrap_or(200);
        let max_retries = data.max_retries.unwrap_or(1);
        let is_enabled = data.is_enabled.unwrap_or(true);

        sqlx::query(
            r#"
            INSERT INTO services (
                id, name, url, method, interval_seconds, timeout_seconds,
                expected_status, headers, body, keyword_match, max_retries,
                is_enabled, current_status, created_at, updated_at
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'up', ?, ?)
            "#,
        )
        .bind(&id)
        .bind(&data.name)
        .bind(&data.url)
        .bind(&method)
        .bind(interval_seconds)
        .bind(timeout_seconds)
        .bind(expected_status)
        .bind(&data.headers)
        .bind(&data.body)
        .bind(&data.keyword_match)
        .bind(max_retries)
        .bind(is_enabled)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await?;

        Ok(Self {
            id,
            name: data.name,
            url: data.url,
            method,
            interval_seconds,
            timeout_seconds,
            expected_status,
            headers: data.headers,
            body: data.body,
            keyword_match: data.keyword_match,
            max_retries,
            is_enabled,
            current_status: ServiceStatus::Up,
            created_at: now,
            updated_at: now,
        })
    }

    pub async fn update(
        pool: &SqlitePool,
        id: &str,
        data: EditService,
    ) -> Result<Option<Self>, sqlx::Error> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        sqlx::query_as::<_, Self>(
            r#"
            UPDATE services
            SET 
                name = ?,
                url = ?,
                method = ?,
                interval_seconds = ?,
                timeout_seconds = ?,
                expected_status = ?,
                headers = ?,
                body = ?,
                keyword_match = ?,
                max_retries = ?,
                is_enabled = ?,
                updated_at = ?
            WHERE id = ?
            RETURNING 
                id, name, url, method, interval_seconds, timeout_seconds,
                expected_status, headers, body, keyword_match, max_retries,
                is_enabled, current_status, created_at, updated_at
            "#,
        )
        .bind(data.name)
        .bind(data.url)
        .bind(data.method)
        .bind(data.interval_seconds)
        .bind(data.timeout_seconds)
        .bind(data.expected_status)
        .bind(data.headers)
        .bind(data.body)
        .bind(data.keyword_match)
        .bind(data.max_retries)
        .bind(data.is_enabled)
        .bind(now)
        .bind(id)
        .fetch_optional(pool)
        .await
    }

    pub async fn delete(pool: &SqlitePool, id: &str) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM services WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn set_enabled(
        pool: &SqlitePool,
        id: &str,
        is_enabled: bool,
    ) -> Result<Option<Self>, sqlx::Error> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        sqlx::query_as::<_, Self>(
            r#"
            UPDATE services
            SET is_enabled = ?, updated_at = ?
            WHERE id = ?
            RETURNING 
                id, name, url, method, interval_seconds, timeout_seconds,
                expected_status, headers, body, keyword_match, max_retries,
                is_enabled, current_status, created_at, updated_at
            "#,
        )
        .bind(is_enabled)
        .bind(now)
        .bind(id)
        .fetch_optional(pool)
        .await
    }
}
