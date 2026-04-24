use std::{any::Any, sync::Arc};

use async_trait::async_trait;
use futures::future::BoxFuture;
use serde_json::json;
use sysinfo::Disks;
use toni::{
    FxHashMap,
    traits_helpers::{Injectable, Provider, ProviderContext, ProviderFactory},
};

use crate::health_check_result::{HealthEntry, HealthIndicatorResult};

/// Checks disk space for a given path against a minimum free-space threshold.
///
/// # Example
///
/// ```ignore
/// self.health.check(vec![
///     // fail if the root filesystem has less than 10% free
///     self.disk.check_storage("disk", "/", 10.0),
/// ]).await
/// ```
#[derive(Clone)]
pub struct DiskHealthIndicator;

impl DiskHealthIndicator {
    pub fn new() -> Self {
        Self
    }

    /// Check that the disk containing `path` has at least `threshold_percent` free space.
    ///
    /// Picks the disk whose mount point is the longest prefix of `path`, matching
    /// how the OS resolves filesystem boundaries.
    pub fn check_storage(
        &self,
        key: impl Into<String>,
        path: impl Into<String>,
        threshold_percent: f64,
    ) -> BoxFuture<'static, HealthIndicatorResult> {
        let key = key.into();
        let path = path.into();

        Box::pin(async move {
            let disks = Disks::new_with_refreshed_list();

            // Find the disk whose mount point is the longest prefix of `path`.
            let best = disks
                .list()
                .iter()
                .filter_map(|d| {
                    let mount = d.mount_point().to_str()?;
                    if path.starts_with(mount) {
                        Some((mount.len(), d))
                    } else {
                        None
                    }
                })
                .max_by_key(|(len, _)| *len)
                .map(|(_, d)| d);

            match best {
                Some(disk) => {
                    let total = disk.total_space();
                    let available = disk.available_space();
                    let free_percent = if total > 0 {
                        (available as f64 / total as f64) * 100.0
                    } else {
                        0.0
                    };

                    if free_percent >= threshold_percent {
                        Ok(HealthEntry::up_with(
                            key,
                            json!({
                                "path": path,
                                "freePercent": free_percent,
                                "total": total,
                                "available": available,
                            }),
                        ))
                    } else {
                        Err(HealthEntry::down_with(
                            key,
                            json!({
                                "path": path,
                                "freePercent": free_percent,
                                "total": total,
                                "available": available,
                                "threshold": threshold_percent,
                                "message": "Free disk space below threshold",
                            }),
                        ))
                    }
                }
                None => Err(HealthEntry::down_with(
                    key,
                    json!({ "path": path, "message": "No disk found for path" }),
                )),
            }
        })
    }
}

impl Default for DiskHealthIndicator {
    fn default() -> Self {
        Self::new()
    }
}

// ── DI machinery ─────────────────────────────────────────────────────────────

pub(crate) struct DiskHealthIndicatorFactory;

#[async_trait]
impl ProviderFactory for DiskHealthIndicatorFactory {
    fn get_token(&self) -> String {
        std::any::type_name::<DiskHealthIndicator>().to_string()
    }

    async fn build(&self, _deps: FxHashMap<String, Injectable>) -> Injectable {
        Injectable::new(Arc::new(Box::new(DiskHealthIndicatorProvider)), vec![])
    }
}

struct DiskHealthIndicatorProvider;

#[async_trait]
impl Provider for DiskHealthIndicatorProvider {
    fn get_token(&self) -> String {
        std::any::type_name::<DiskHealthIndicator>().to_string()
    }

    fn get_token_factory(&self) -> String {
        std::any::type_name::<DiskHealthIndicator>().to_string()
    }

    async fn execute(
        &self,
        _params: Vec<Box<dyn Any + Send>>,
        _ctx: ProviderContext<'_>,
    ) -> Box<dyn Any + Send> {
        Box::new(DiskHealthIndicator)
    }
}
