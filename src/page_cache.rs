//! Rendered public pages, kept a minute: whatever the traffic, a public page costs at most one
//! render per minute. Expired pages are dropped, so idle pages hold no memory.

use axum::body::Bytes;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a rendered page is served.
pub const TTL: Duration = Duration::from_secs(60);

type Slot = Arc<tokio::sync::Mutex<Option<(Instant, Bytes)>>>;

/// By page id, so it holds at most one entry per existing page.
#[derive(Clone)]
pub struct PageCache {
    slots: Arc<Mutex<HashMap<i64, Slot>>>,
    ttl: Duration,
}

impl Default for PageCache {
    fn default() -> Self {
        Self::with_ttl(TTL)
    }
}

impl PageCache {
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            slots: Arc::default(),
            ttl,
        }
    }

    /// The cached HTML of the page, or `render`'s, stored when it succeeds. Concurrent requests
    /// for the same page wait for a single render.
    pub async fn get_or_render<E, F, Fut>(&self, page_id: i64, render: F) -> Result<Bytes, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<String, E>>,
    {
        let slot = {
            let mut slots = self.slots.lock().unwrap();
            // A slot being rendered is busy and kept.
            slots.retain(|_, slot| {
                slot.try_lock().map_or(true, |c| {
                    c.as_ref().is_some_and(|(at, _)| at.elapsed() < self.ttl)
                })
            });
            slots.entry(page_id).or_default().clone()
        };
        let mut cached = slot.lock().await;
        if let Some((at, html)) = cached.as_ref()
            && at.elapsed() < self.ttl
        {
            return Ok(html.clone());
        }
        let html = Bytes::from(render().await?);
        *cached = Some((Instant::now(), html.clone()));
        Ok(html)
    }

    /// Drops the page, so its next request renders it again. A render already running stores
    /// its result in the dropped slot, where nobody reads it.
    pub fn invalidate(&self, page_id: i64) {
        self.slots.lock().unwrap().remove(&page_id);
    }

    /// Drops every page, for a change that may show on any of them.
    pub fn clear(&self) {
        self.slots.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn renders_once_until_invalidated() {
        let cache = PageCache::default();
        let renders = Arc::new(AtomicUsize::new(0));
        let render = || {
            let renders = renders.clone();
            async move {
                let n = renders.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(format!("render {n}"))
            }
        };
        let (a, b) = tokio::join!(
            cache.get_or_render(1, render),
            cache.get_or_render(1, render)
        );
        assert_eq!(a.unwrap(), "render 0");
        assert_eq!(b.unwrap(), "render 0");
        assert_eq!(cache.get_or_render(2, render).await.unwrap(), "render 1");

        cache.invalidate(1);
        assert_eq!(cache.get_or_render(1, render).await.unwrap(), "render 2");
        assert_eq!(cache.get_or_render(2, render).await.unwrap(), "render 1");
    }

    #[tokio::test]
    async fn expired_pages_are_dropped() {
        let cache = PageCache::with_ttl(Duration::ZERO);
        let render = || async { Ok::<_, ()>("page".to_string()) };
        cache.get_or_render(1, render).await.unwrap();
        cache.get_or_render(2, render).await.unwrap();
        assert_eq!(cache.slots.lock().unwrap().len(), 1, "page 1 expired");
    }

    #[tokio::test]
    async fn errors_are_not_cached() {
        let cache = PageCache::default();
        let failed = cache.get_or_render(1, || async { Err::<String, _>("boom") });
        assert_eq!(failed.await.unwrap_err(), "boom");
        let ok = cache.get_or_render(1, || async { Ok::<_, &str>("page".to_string()) });
        assert_eq!(ok.await.unwrap(), "page");
    }
}
