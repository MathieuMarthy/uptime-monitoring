use crate::auth::random_token;
use crate::auth::rate_limit::LoginLimiter;
use crate::engine::EngineHandle;
use crate::models::Admin;
use sqlx::SqlitePool;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    /// Set while no admin exists: the one-time token of the `/setup` URL printed in the logs.
    pub setup_token: Arc<Mutex<Option<String>>>,
    pub login_limiter: Arc<Mutex<LoginLimiter>>,
    /// One password hash at a time: each Argon2 run takes 19 MiB.
    pub password_hashing: Arc<Semaphore>,
    pub trust_proxy_headers: bool,
    /// Told about every monitor change; disconnected unless the engine was started.
    pub engine: EngineHandle,
}

impl AppState {
    pub fn new(db: SqlitePool) -> Self {
        Self {
            db,
            setup_token: Arc::default(),
            login_limiter: Arc::default(),
            password_hashing: Arc::new(Semaphore::new(1)),
            trust_proxy_headers: false,
            engine: EngineHandle::default(),
        }
    }

    pub fn with_trust_proxy_headers(mut self, trust: bool) -> Self {
        self.trust_proxy_headers = trust;
        self
    }

    pub fn with_engine(mut self, engine: EngineHandle) -> Self {
        self.engine = engine;
        self
    }

    /// When no admin exists, generates the setup token and returns it.
    pub async fn start_setup_if_needed(&self) -> Result<Option<String>, sqlx::Error> {
        if Admin::exists(&self.db).await? {
            return Ok(None);
        }
        let token = random_token();
        *self.setup_token.lock().unwrap() = Some(token.clone());
        Ok(Some(token))
    }

    pub fn setup_pending(&self) -> bool {
        self.setup_token.lock().unwrap().is_some()
    }
}
