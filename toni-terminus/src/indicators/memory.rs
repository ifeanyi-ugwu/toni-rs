use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde_json::json;
use sysinfo::{Pid, ProcessesToUpdate, System};
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};

use crate::health_check_result::{HealthEntry, HealthIndicatorResult};

/// Checks process and system memory usage against configurable thresholds.
///
/// # Example
///
/// ```ignore
/// self.health.check(vec![
///     // fail if the process RSS exceeds 512 MiB
///     self.memory.check_rss("memory_rss", 512 * 1024 * 1024),
///     // fail if total system memory used exceeds 1 GiB
///     self.memory.check_heap("memory_heap", 1024 * 1024 * 1024),
/// ]).await
/// ```
#[derive(Clone)]
pub struct MemoryHealthIndicator;

impl MemoryHealthIndicator {
    pub fn new() -> Self {
        Self
    }

    /// Check process RSS (resident set size). Fails when RSS ≥ `threshold_bytes`.
    pub fn check_rss(
        &self,
        key: impl Into<String>,
        threshold_bytes: u64,
    ) -> BoxFuture<'static, HealthIndicatorResult> {
        let key = key.into();
        Box::pin(async move {
            let pid = Pid::from(std::process::id() as usize);
            let mut sys = System::new();
            sys.refresh_processes(ProcessesToUpdate::Some(&[pid]), false);

            match sys.process(pid) {
                Some(proc) => {
                    let rss = proc.memory();
                    if rss < threshold_bytes {
                        Ok(HealthEntry::up_with(
                            key,
                            json!({ "rss": rss, "threshold": threshold_bytes }),
                        ))
                    } else {
                        Err(HealthEntry::down_with(
                            key,
                            json!({
                                "rss": rss,
                                "threshold": threshold_bytes,
                                "message": "Process RSS exceeds threshold",
                            }),
                        ))
                    }
                }
                None => Err(HealthEntry::down_with(
                    key,
                    json!({ "message": "Could not read process memory" }),
                )),
            }
        })
    }

    /// Check total system memory used. Fails when used memory ≥ `threshold_bytes`.
    pub fn check_heap(
        &self,
        key: impl Into<String>,
        threshold_bytes: u64,
    ) -> BoxFuture<'static, HealthIndicatorResult> {
        let key = key.into();
        Box::pin(async move {
            let mut sys = System::new();
            sys.refresh_memory();

            let used = sys.used_memory();
            let total = sys.total_memory();

            if used < threshold_bytes {
                Ok(HealthEntry::up_with(
                    key,
                    json!({ "used": used, "total": total, "threshold": threshold_bytes }),
                ))
            } else {
                Err(HealthEntry::down_with(
                    key,
                    json!({
                        "used": used,
                        "total": total,
                        "threshold": threshold_bytes,
                        "message": "System memory usage exceeds threshold",
                    }),
                ))
            }
        })
    }
}

impl Default for MemoryHealthIndicator {
    fn default() -> Self {
        Self::new()
    }
}

// ── DI machinery ─────────────────────────────────────────────────────────────

pub(crate) struct MemoryHealthIndicatorFactory;

#[async_trait]
impl ProviderFactory for MemoryHealthIndicatorFactory {
    fn get_token(&self) -> String {
        toni::di::token_of::<MemoryHealthIndicator>()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        Injectable::new(Arc::new(Box::new(MemoryHealthIndicatorProvider)), vec![])
    }
}

struct MemoryHealthIndicatorProvider;

#[async_trait]
impl Provider for MemoryHealthIndicatorProvider {
    fn get_token(&self) -> String {
        toni::di::token_of::<MemoryHealthIndicator>()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext,
    ) -> Box<dyn Any + Send> {
        Box::new(MemoryHealthIndicator)
    }
}
