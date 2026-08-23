use crate::common::TestServer;
use serde::Deserialize;
use toni::{
    controller,
    extractors::{Bytes as RenamedBytes, Json, Path, Query, Validated},
    get, module, post, routes, Body as ToniBody,
};
use validator::Validate;

#[derive(Debug, Deserialize)]
struct SearchParams {
    q: String,
    limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
struct CreateUserDto {
    name: String,
    email: String,
}

#[controller("/api")]
pub struct ExtractorController;

#[routes]
impl ExtractorController {
    #[get("/search")]
    fn search(&self, Query(params): Query<SearchParams>) -> ToniBody {
        let limit = params.limit.unwrap_or(10);
        ToniBody::text(format!("Searching for '{}' with limit {}", params.q, limit))
    }

    #[post("/users")]
    fn create_user(&self, Json(dto): Json<CreateUserDto>) -> ToniBody {
        ToniBody::text(format!("Created user: {} <{}>", dto.name, dto.email))
    }

    #[post("/echo")]
    fn echo_json(&self, body: Json<serde_json::Value>) -> ToniBody {
        ToniBody::json(body.into_inner())
    }

    /// Verifies the controller macro routes an aliased import of a body-consuming
    /// extractor through the body code path instead of dropping it into the
    /// `Unknown` parts-only branch. Without the fix, `body.0` would always be
    /// empty here regardless of what the client sent.
    #[post("/aliased-bytes")]
    fn aliased_bytes(&self, body: RenamedBytes) -> ToniBody {
        ToniBody::text(format!("len={}", body.0.len()))
    }

    #[get("/items/{id}")]
    fn typed_path(&self, Path(id): Path<i32>) -> ToniBody {
        ToniBody::text(format!("id={}", id))
    }
}

#[module(
    controllers: [ExtractorController],
    providers: [],
)]
impl ExtractorModule {}

#[tokio_localset_test::localset_test]
async fn test_query_extractor() {
    let server = TestServer::start(ExtractorModule).await;

    let resp = server
        .client()
        .get(server.url("/api/search?q=rust&limit=5"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.text().await.unwrap(),
        "Searching for 'rust' with limit 5"
    );

    let resp = server
        .client()
        .get(server.url("/api/search?q=toni"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.text().await.unwrap(),
        "Searching for 'toni' with limit 10"
    );

    // missing required param → 400
    let resp = server
        .client()
        .get(server.url("/api/search"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio_localset_test::localset_test]
async fn test_aliased_body_extractor_receives_body() {
    let server = TestServer::start(ExtractorModule).await;

    let resp = server
        .client()
        .post(server.url("/api/aliased-bytes"))
        .body("payload-of-12")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "len=13");
}

#[tokio_localset_test::localset_test]
async fn test_json_extractor() {
    let server = TestServer::start(ExtractorModule).await;

    let resp = server
        .client()
        .post(server.url("/api/users"))
        .json(&serde_json::json!({"name": "John Doe", "email": "john@example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.text().await.unwrap(),
        "Created user: John Doe <john@example.com>"
    );

    // echo generic JSON value
    let resp = server
        .client()
        .post(server.url("/api/echo"))
        .json(&serde_json::json!({"foo": "bar", "num": 42}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["foo"], "bar");
    assert_eq!(body["num"], 42);

    // missing required field → 400
    let resp = server
        .client()
        .post(server.url("/api/users"))
        .json(&serde_json::json!({"name": "John Doe"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[derive(Debug, Deserialize, Validate)]
struct ValidatedUserDto {
    #[validate(length(min = 3, message = "Name must be at least 3 characters"))]
    name: String,
    #[validate(email(message = "Invalid email format"))]
    email: String,
}

#[controller("/validated")]
pub struct ValidatedController;

#[routes]
impl ValidatedController {
    #[post("/users")]
    fn create_user(&self, Validated(Json(dto)): Validated<Json<ValidatedUserDto>>) -> ToniBody {
        ToniBody::text(format!(
            "Created validated user: {} <{}>",
            dto.name, dto.email
        ))
    }
}

#[module(
    controllers: [ValidatedController],
    providers: [],
)]
impl ValidatedModule {}

#[tokio_localset_test::localset_test]
async fn test_validated_extractor() {
    let server = TestServer::start(ValidatedModule).await;

    let resp = server
        .client()
        .post(server.url("/validated/users"))
        .json(&serde_json::json!({"name": "John Doe", "email": "john@example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.text().await.unwrap(),
        "Created validated user: John Doe <john@example.com>"
    );

    // name too short
    let resp = server
        .client()
        .post(server.url("/validated/users"))
        .json(&serde_json::json!({"name": "Jo", "email": "john@example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // invalid email
    let resp = server
        .client()
        .post(server.url("/validated/users"))
        .json(&serde_json::json!({"name": "John Doe", "email": "not-an-email"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // both invalid
    let resp = server
        .client()
        .post(server.url("/validated/users"))
        .json(&serde_json::json!({"name": "Jo", "email": "bad"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio_localset_test::localset_test]
async fn test_typed_path_extractor() {
    let server = TestServer::start(ExtractorModule).await;

    let resp = server
        .client()
        .get(server.url("/api/items/42"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "id=42");

    // non-numeric segment → 400
    let resp = server
        .client()
        .get(server.url("/api/items/abc"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[derive(Debug, Deserialize, Validate)]
struct SearchQuery {
    #[validate(length(min = 3, message = "Query must be at least 3 characters"))]
    q: String,
}

#[controller("/wrapped")]
pub struct WrappedQueryController;

#[routes]
impl WrappedQueryController {
    /// `Validated<Query<T>>` reads the query string and nothing else, so a body
    /// extractor on the same handler still finds a body to read. Wrapping used
    /// to route the parameter through the body path, which both emptied the body
    /// and made this signature a compile error.
    #[post("/search")]
    fn search(
        &self,
        Validated(Query(params)): Validated<Query<SearchQuery>>,
        Json(dto): Json<CreateUserDto>,
    ) -> ToniBody {
        ToniBody::text(format!("q={} name={}", params.q, dto.name))
    }
}

#[module(
    controllers: [WrappedQueryController],
    providers: [],
)]
impl WrappedQueryModule {}

#[tokio_localset_test::localset_test]
async fn test_validated_query_leaves_the_body_alone() {
    let server = TestServer::start(WrappedQueryModule).await;

    let resp = server
        .client()
        .post(server.url("/wrapped/search?q=rust"))
        .json(&serde_json::json!({"name": "John Doe", "email": "john@example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "q=rust name=John Doe");

    // The query is still validated.
    let resp = server
        .client()
        .post(server.url("/wrapped/search?q=ab"))
        .json(&serde_json::json!({"name": "John Doe", "email": "john@example.com"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}
