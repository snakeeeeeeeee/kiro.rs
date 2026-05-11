//! Runtime controls for single-node production use.

use std::collections::VecDeque;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;
use std::time::Instant;

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde_json::json;
use tokio::sync::Notify;
use tokio::time::timeout;

use crate::kiro::token_manager::MultiTokenManager;
use crate::metrics::duration_ms;
use crate::model::config::Config;

pub struct RuntimeLimiter {
    waiters: Mutex<VecDeque<Arc<Notify>>>,
    global_rpm_window: Mutex<Vec<DateTime<Utc>>>,
    closed: AtomicBool,
}

pub struct GlobalRequestPermit {
    token_manager: Arc<MultiTokenManager>,
    limiter: Weak<RuntimeLimiter>,
    queue_ms: u64,
}

impl GlobalRequestPermit {
    pub fn queue_ms(&self) -> u64 {
        self.queue_ms
    }
}

impl Drop for GlobalRequestPermit {
    fn drop(&mut self) {
        self.token_manager.decrement_global_in_flight();
        if let Some(limiter) = self.limiter.upgrade() {
            limiter.notify_capacity_available();
        }
    }
}

impl RuntimeLimiter {
    pub fn new(_config: &Config) -> Self {
        Self {
            waiters: Mutex::new(VecDeque::new()),
            global_rpm_window: Mutex::new(Vec::new()),
            closed: AtomicBool::new(false),
        }
    }

    pub async fn acquire(
        self: &Arc<Self>,
        token_manager: Arc<MultiTokenManager>,
    ) -> Result<GlobalRequestPermit, RuntimeLimitError> {
        let started_at = Instant::now();
        if self.closed.load(Ordering::Acquire) {
            return Err(RuntimeLimitError::Closed);
        }

        if self.try_enter_global(&token_manager) {
            if !self.try_record_global_rpm(&token_manager) {
                token_manager.decrement_global_in_flight();
                return Err(RuntimeLimitError::GlobalRpmExceeded);
            }
            return Ok(GlobalRequestPermit {
                token_manager,
                limiter: Arc::downgrade(self),
                queue_ms: duration_ms(started_at.elapsed()),
            });
        }

        if !token_manager.try_enter_queue() {
            return Err(RuntimeLimitError::QueueFull);
        }

        let notify = Arc::new(Notify::new());
        self.waiters.lock().push_back(notify.clone());

        let queue_timeout =
            Duration::from_millis(token_manager.runtime_settings().queue_timeout_ms);

        let result = loop {
            match timeout(queue_timeout, notify.notified()).await {
                Ok(_) => {
                    if self.closed.load(Ordering::Acquire) {
                        break Err(RuntimeLimitError::Closed);
                    }
                    if self.try_enter_global(&token_manager) {
                        if !self.try_record_global_rpm(&token_manager) {
                            token_manager.decrement_global_in_flight();
                            break Err(RuntimeLimitError::GlobalRpmExceeded);
                        }
                        break Ok(());
                    }
                    self.waiters.lock().push_back(notify.clone());
                }
                Err(_) => break Err(RuntimeLimitError::QueueTimeout),
            }
        };

        self.remove_waiter(&notify);
        token_manager.leave_queue();
        result.map(|_| GlobalRequestPermit {
            token_manager,
            limiter: Arc::downgrade(self),
            queue_ms: duration_ms(started_at.elapsed()),
        })
    }

    fn try_enter_global(&self, token_manager: &Arc<MultiTokenManager>) -> bool {
        let settings = token_manager.runtime_settings();
        token_manager.increment_global_in_flight();
        let current = token_manager.global_in_flight();
        if current > settings.global_max_concurrent {
            token_manager.decrement_global_in_flight();
            return false;
        }
        true
    }

    fn try_record_global_rpm(&self, token_manager: &Arc<MultiTokenManager>) -> bool {
        let global_rpm = token_manager.runtime_settings().global_rpm;
        if global_rpm == 0 {
            return true;
        }

        let now = Utc::now();
        let cutoff = now - chrono::Duration::seconds(60);
        let mut window = self.global_rpm_window.lock();
        window.retain(|ts| *ts > cutoff);

        if window.len() as u32 >= global_rpm {
            return false;
        }

        window.push(now);
        true
    }

    fn remove_waiter(&self, needle: &Arc<Notify>) {
        let mut waiters = self.waiters.lock();
        waiters.retain(|waiter| !Arc::ptr_eq(waiter, needle));
    }

    pub fn notify_capacity_available(&self) {
        if let Some(waiter) = self.waiters.lock().pop_front() {
            waiter.notify_one();
        }
    }
}

#[derive(Debug)]
pub enum RuntimeLimitError {
    QueueFull,
    QueueTimeout,
    Closed,
    GlobalRpmExceeded,
}

impl RuntimeLimitError {
    pub fn into_response(self) -> Response {
        let message = match self {
            RuntimeLimitError::QueueFull => "请求队列已满，请稍后重试",
            RuntimeLimitError::QueueTimeout => "请求等待超时，请稍后重试",
            RuntimeLimitError::Closed => "服务正在关闭，请稍后重试",
            RuntimeLimitError::GlobalRpmExceeded => "全局 RPM 限制已触发，请稍后重试",
        };

        (
            StatusCode::TOO_MANY_REQUESTS,
            Json(json!({
                "error": {
                    "type": "rate_limit_error",
                    "message": message
                }
            })),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::kiro::token_manager::MultiTokenManager;
    use crate::model::config::Config;

    use super::*;

    #[tokio::test]
    async fn global_rpm_is_not_consumed_by_queue_full_requests() {
        let mut config = Config::default();
        config.global_max_concurrent = 1;
        config.queue_max_size = 0;
        config.global_rpm = 2;

        let manager = Arc::new(
            MultiTokenManager::new(config.clone(), Vec::new(), None, None, false).unwrap(),
        );
        let limiter = Arc::new(RuntimeLimiter::new(&config));

        let permit = limiter.acquire(manager.clone()).await.unwrap();
        assert!(matches!(
            limiter.acquire(manager.clone()).await.err(),
            Some(RuntimeLimitError::QueueFull)
        ));
        drop(permit);

        let second = limiter.acquire(manager.clone()).await.unwrap();
        drop(second);

        let third = limiter.acquire(manager.clone()).await;
        assert!(
            matches!(third.err(), Some(RuntimeLimitError::GlobalRpmExceeded)),
            "queue-full attempts should not consume the second RPM slot"
        );
    }
}
