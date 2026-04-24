use serde_json::{Value, json};
use toni::{Body, HttpResponse, IntoResponse};

/// A single indicator's result, carried by both the healthy (`Ok`) and
/// unhealthy (`Err`) arms of [`HealthIndicatorResult`].
pub struct HealthEntry {
    /// Unique key for this check, e.g. `"database"`, `"redis"`.
    pub key: String,
    /// `"up"` when healthy, `"down"` when unhealthy.
    pub status: &'static str,
    /// Extra fields merged into the check's JSON object (response time, thresholds, etc.).
    pub details: Value,
}

impl HealthEntry {
    pub fn up(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            status: "up",
            details: json!({}),
        }
    }

    pub fn up_with(key: impl Into<String>, details: Value) -> Self {
        Self {
            key: key.into(),
            status: "up",
            details,
        }
    }

    pub fn down(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            status: "down",
            details: json!({}),
        }
    }

    pub fn down_with(key: impl Into<String>, details: Value) -> Self {
        Self {
            key: key.into(),
            status: "down",
            details,
        }
    }

    fn to_json(&self) -> Value {
        let mut map = serde_json::Map::new();
        map.insert("status".to_string(), Value::String(self.status.to_string()));
        if let Value::Object(extras) = &self.details {
            map.extend(extras.clone());
        }
        Value::Object(map)
    }
}

/// `Ok(entry)` = indicator is healthy; `Err(entry)` = indicator is unhealthy.
///
/// Both arms carry a [`HealthEntry`] so the aggregated response can always
/// include full details regardless of outcome.
pub type HealthIndicatorResult = Result<HealthEntry, HealthEntry>;

/// Aggregated result of all health checks run by [`HealthCheckService::check`].
///
/// Implements [`IntoResponse`]: returns **HTTP 200** when all checks pass,
/// **HTTP 503** when any check fails. The JSON shape matches NestJS Terminus:
///
/// ```json
/// {
///   "status": "ok",
///   "info":    { "redis": { "status": "up" } },
///   "error":   {},
///   "details": { "redis": { "status": "up" } }
/// }
/// ```
pub struct HealthCheckResult {
    status: &'static str,
    info: Vec<HealthEntry>,
    error: Vec<HealthEntry>,
}

impl HealthCheckResult {
    pub(crate) fn from_results(results: Vec<HealthIndicatorResult>) -> Self {
        let mut info = Vec::new();
        let mut error = Vec::new();

        for result in results {
            match result {
                Ok(entry) => info.push(entry),
                Err(entry) => error.push(entry),
            }
        }

        let status = if error.is_empty() { "ok" } else { "error" };
        Self { status, info, error }
    }

    pub fn status(&self) -> &'static str {
        self.status
    }

    pub fn is_healthy(&self) -> bool {
        self.status == "ok"
    }
}

impl IntoResponse for HealthCheckResult {
    fn into_response(self) -> HttpResponse {
        let http_status: u16 = if self.status == "ok" { 200 } else { 503 };

        let mut info_map = serde_json::Map::new();
        let mut error_map = serde_json::Map::new();
        let mut details_map = serde_json::Map::new();

        for entry in &self.info {
            let v = entry.to_json();
            details_map.insert(entry.key.clone(), v.clone());
            info_map.insert(entry.key.clone(), v);
        }
        for entry in &self.error {
            let v = entry.to_json();
            details_map.insert(entry.key.clone(), v.clone());
            error_map.insert(entry.key.clone(), v);
        }

        let body = json!({
            "status":  self.status,
            "info":    Value::Object(info_map),
            "error":   Value::Object(error_map),
            "details": Value::Object(details_map),
        });

        HttpResponse {
            status: http_status,
            body: Some(Body::json(body)),
            headers: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_body(response: HttpResponse) -> Value {
        let bytes = response
            .body
            .unwrap()
            .try_bytes()
            .unwrap()
            .to_vec();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[test]
    fn all_passing_is_ok() {
        let result = HealthCheckResult::from_results(vec![
            Ok(HealthEntry::up("db")),
            Ok(HealthEntry::up("redis")),
        ]);
        assert_eq!(result.status(), "ok");
        assert!(result.is_healthy());
    }

    #[test]
    fn any_failing_is_error() {
        let result = HealthCheckResult::from_results(vec![
            Ok(HealthEntry::up("db")),
            Err(HealthEntry::down("redis")),
        ]);
        assert_eq!(result.status(), "error");
        assert!(!result.is_healthy());
    }

    #[test]
    fn all_failing_is_error() {
        let result = HealthCheckResult::from_results(vec![
            Err(HealthEntry::down("db")),
            Err(HealthEntry::down("redis")),
        ]);
        assert_eq!(result.status(), "error");
    }

    #[test]
    fn empty_checks_is_ok() {
        let result = HealthCheckResult::from_results(vec![]);
        assert_eq!(result.status(), "ok");
        assert!(result.is_healthy());
    }

    #[test]
    fn http_200_when_all_pass() {
        let result = HealthCheckResult::from_results(vec![Ok(HealthEntry::up("db"))]);
        assert_eq!(result.into_response().status, 200);
    }

    #[test]
    fn http_503_when_any_fail() {
        let result = HealthCheckResult::from_results(vec![Err(HealthEntry::down("db"))]);
        assert_eq!(result.into_response().status, 503);
    }

    #[test]
    fn json_shape_all_passing() {
        let result = HealthCheckResult::from_results(vec![Ok(HealthEntry::up("db"))]);
        let body = parse_body(result.into_response());

        assert_eq!(body["status"], "ok");
        assert_eq!(body["info"]["db"]["status"], "up");
        assert_eq!(body["error"], json!({}));
        assert_eq!(body["details"]["db"]["status"], "up");
    }

    #[test]
    fn json_shape_with_failure() {
        let result = HealthCheckResult::from_results(vec![
            Ok(HealthEntry::up("db")),
            Err(HealthEntry::down_with("redis", json!({ "message": "connection refused" }))),
        ]);
        let body = parse_body(result.into_response());

        assert_eq!(body["status"], "error");
        assert_eq!(body["info"]["db"]["status"], "up");
        assert_eq!(body["error"]["redis"]["status"], "down");
        assert_eq!(body["error"]["redis"]["message"], "connection refused");
        assert_eq!(body["details"]["db"]["status"], "up");
        assert_eq!(body["details"]["redis"]["status"], "down");
    }

    #[test]
    fn extra_details_merged_into_entry() {
        let result = HealthCheckResult::from_results(vec![Ok(HealthEntry::up_with(
            "memory",
            json!({ "rss": 1024, "threshold": 2048 }),
        ))]);
        let body = parse_body(result.into_response());

        assert_eq!(body["info"]["memory"]["status"], "up");
        assert_eq!(body["info"]["memory"]["rss"], 1024);
        assert_eq!(body["info"]["memory"]["threshold"], 2048);
    }
}
