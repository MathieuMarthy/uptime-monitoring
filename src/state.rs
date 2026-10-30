use crate::auth::random_token;
use crate::auth::rate_limit::LoginLimiter;
use crate::engine::EngineHandle;
use crate::engine::guard::Guard;
use crate::engine::http::HttpChecker;
use crate::models::Admin;
use crate::notify::Notifier;
use crate::page_cache::PageCache;
use sqlx::SqlitePool;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// Form tests running at once; more wait for none and are told to retry.
const MONITOR_TESTS: usize = 2;

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    /// Set while no admin exists: the one-time token of the `/setup` URL printed in the logs.
    pub setup_token: Arc<Mutex<Option<String>>>,
    pub login_limiter: Arc<Mutex<LoginLimiter>>,
    /// One password hash at a time: each Argon2 run takes 19 MiB.
    pub password_hashing: Arc<Semaphore>,
    pub trust_proxy_headers: bool,
    /// `PUBLIC_URL`: completes the setup URL printed if the admin row disappears.
    pub public_url: Option<String>,
    /// Told about every monitor change; disconnected unless the engine was started.
    pub engine: EngineHandle,
    /// Private-target blocking shared with the engine and the notifier; settings change it live.
    pub guard: Guard,
    /// Sends channel tests; alerts themselves come from the engine.
    pub notifier: Notifier,
    /// Rendered public pages; every change to a page invalidates it.
    pub public_pages: PageCache,
    /// Checks run from the monitor form's Test button at the same time.
    pub monitor_tests: Arc<Semaphore>,
    /// Runs those checks; its clients are built once. Rebuilt by `with_guard`.
    pub form_checker: Arc<HttpChecker>,
}

impl AppState {
    pub fn new(db: SqlitePool) -> Self {
        Self {
            db,
            setup_token: Arc::default(),
            login_limiter: Arc::default(),
            password_hashing: Arc::new(Semaphore::new(1)),
            trust_proxy_headers: false,
            public_url: None,
            engine: EngineHandle::default(),
            guard: Guard::default(),
            notifier: Notifier::default(),
            public_pages: PageCache::default(),
            monitor_tests: Arc::new(Semaphore::new(MONITOR_TESTS)),
            form_checker: Arc::new(HttpChecker::new(Guard::default())),
        }
    }

    pub fn with_trust_proxy_headers(mut self, trust: bool) -> Self {
        self.trust_proxy_headers = trust;
        self
    }

    pub fn with_public_url(mut self, public_url: Option<String>) -> Self {
        self.public_url = public_url;
        self
    }

    pub fn with_engine(mut self, engine: EngineHandle) -> Self {
        self.engine = engine;
        self
    }

    pub fn with_guard(mut self, guard: Guard) -> Self {
        self.form_checker = Arc::new(HttpChecker::new(guard.clone()));
        self.guard = guard;
        self
    }

    pub fn with_notifier(mut self, notifier: Notifier) -> Self {
        self.notifier = notifier;
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
