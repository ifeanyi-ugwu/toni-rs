//! # Parsing, validating and refusing input — the complete guide
//!
//! Written for developers arriving from NestJS, where all of this is the job of
//! `PipeTransform`. toni has no pipe. What a Nest pipe does — receive the value
//! a handler is about to be given, and either reshape it or refuse it — is what
//! an extractor does here, and the handler's signature says which rules ran.
//!
//! ## The map
//!
//! | NestJS | toni |
//! | --- | --- |
//! | `@Body()` | `Json<T>` |
//! | `@Body(ValidationPipe)` | `Validated<Json<T>>` |
//! | `@Param('id', ParseIntPipe)` | `Path<u32>` |
//! | `@Param('id', ParseUUIDPipe)` | `Path<Uuid>` |
//! | `@Query('archived', ParseBoolPipe)` | a `bool` field on the query struct |
//! | `@Query('status', new ParseEnumPipe(Status))` | an enum field on the query struct |
//! | `@Query('page', new DefaultValuePipe(1))` | `#[serde(default = "…")]`, or `Option<T>` |
//! | `@Query('ids', new ParseArrayPipe({items: Number}))` | `#[serde(deserialize_with = "…")]` |
//! | `@Body(new ParseArrayPipe({items: Dto}))` | `Validated<Json<Vec<Dto>>>` |
//! | a custom `PipeTransform` | a `FromContext` impl |
//! | `@UsePipes()` refusing a request | a `Guard`, or an `Interceptor` that answers |
//! | `app.useGlobalPipes(new ValidationPipe())` | nothing — validation is per-signature |
//!
//! The last row is the one worth pausing on. Nest needs a global pipe because
//! TypeScript's types are gone by the time the request arrives, so the framework
//! rebuilds them at runtime from `design:paramtypes`. Rust's are still there, so
//! the declaration that says what a value *is* can also say what makes it valid.
//!
//! Run it:
//!
//! ```console
//! cargo run --example validation_complete_guide
//! ```

use std::fmt;

use serde::{Deserialize, Deserializer};
use toni::async_trait;
use toni::context::HttpContext;
use toni::extractors::{FromContext, Json, Path, Payload, Query, Validated};
use toni::http_helpers::{Body, HttpResponse};
use toni::rpc::{RpcData, RpcError};
use toni::traits_helpers::{Guard, Interceptor, InterceptorNext};
use toni::websocket::{WsClient, WsHandlerResult, WsMessage};
use toni::{controller, get, injectable, module, patterns, post, routes, rpc_controller};
use toni_macros::{new, subscriptions, websocket_gateway};
use validator::Validate;

// Parsing: what ParseIntPipe and its siblings were for

/// `ParseBoolPipe` and `ParseEnumPipe` at once: both are fields whose declared
/// type does the checking. An unknown `status` is refused by serde, and the
/// error names the field.
///
/// ```typescript
/// @Get('products')
/// list(
///   @Query('archived', ParseBoolPipe) archived: boolean,
///   @Query('status', new ParseEnumPipe(Status)) status: Status,
/// ) {}
/// ```
#[derive(Debug, Deserialize)]
pub struct ListProducts {
    /// `DefaultValuePipe(1)` — supplied when the parameter is absent.
    #[serde(default = "first_page")]
    page: u32,

    /// Absent is a meaningful answer here, so it stays `Option` rather than
    /// taking a default.
    archived: Option<bool>,

    #[serde(default)]
    status: Status,

    /// `ParseArrayPipe({items: Number, separator: ','})` — `?ids=1,2,3`.
    /// `serde_urlencoded` will not split a delimited string on its own, so the
    /// splitting is named on the field.
    #[serde(default, deserialize_with = "comma_separated_u32")]
    ids: Vec<u32>,
}

#[derive(Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    #[default]
    Active,
    Banned,
}

fn first_page() -> u32 {
    1
}

fn comma_separated_u32<'de, D: Deserializer<'de>>(de: D) -> Result<Vec<u32>, D::Error> {
    let raw = String::deserialize(de)?;
    raw.split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.trim().parse::<u32>().map_err(serde::de::Error::custom))
        .collect()
}

// Validation: what ValidationPipe was for

/// The `class-validator` decorators, as `validator` attributes. `Validated<E>`
/// runs them at extraction; a failure is answered before the handler runs.
///
/// ```typescript
/// class CreateUserDto {
///   @IsEmail() email: string;
///   @Length(8, 64) password: string;
///   @Min(18) @IsOptional() age?: number;
/// }
/// ```
#[derive(Debug, Deserialize, Validate)]
pub struct CreateUser {
    #[validate(email)]
    email: String,

    #[validate(length(min = 8, max = 64))]
    password: String,

    #[validate(range(min = 18))]
    age: Option<u8>,

    /// Nested structs need `#[validate(nested)]`, the counterpart of Nest's
    /// `@ValidateNested()` + `@Type(() => Address)`.
    #[validate(nested)]
    address: Address,
}

#[derive(Debug, Deserialize, Validate)]
pub struct Address {
    #[validate(length(min = 2))]
    city: String,
}

/// Validation that a type carries rather than a handler declaring: the value
/// cannot exist in an invalid state, so nothing downstream re-checks it. This
/// is the one shape a pipe has no answer for — a validated DTO and an
/// unvalidated one are the same TypeScript type.
/// `#[serde(try_from)]` runs the conversion during deserialisation, so a `Slug`
/// that exists is one that parsed.
#[derive(Debug, Deserialize)]
#[serde(try_from = "String")]
pub struct Slug(String);

impl TryFrom<String> for Slug {
    type Error = String;

    fn try_from(raw: String) -> Result<Self, Self::Error> {
        let trimmed = raw.trim().to_lowercase();
        if trimmed.is_empty() {
            return Err("a slug cannot be empty".to_string());
        }
        if !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(format!("`{trimmed}` is not a slug"));
        }
        Ok(Slug(trimmed))
    }
}

#[derive(Debug, Deserialize)]
pub struct CreatePost {
    slug: Slug,

    /// `deserialize_with` is per-field normalisation — Nest's transforming
    /// pipe, applied where the field is declared.
    #[serde(deserialize_with = "trimmed")]
    title: String,
}

fn trimmed<'de, D: Deserializer<'de>>(de: D) -> Result<String, D::Error> {
    Ok(String::deserialize(de)?.trim().to_string())
}

// A custom extractor: the true PipeTransform analogue

/// A custom `PipeTransform` receives a value and returns the one the handler
/// gets. A `FromContext` impl does the same, and says which context it reads
/// from — so reaching for this in a WebSocket handler is a trait-bound error
/// rather than something that fails at runtime.
///
/// ```typescript
/// @Injectable()
/// export class ApiVersionPipe implements PipeTransform {
///   transform(value: string) {
///     const v = parseInt(value, 10);
///     if (isNaN(v)) throw new BadRequestException('bad version');
///     return v;
///   }
/// }
/// ```
pub struct ApiVersion(pub u8);

#[derive(Debug)]
pub struct MissingApiVersion;

impl fmt::Display for MissingApiVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "x-api-version must be present and numeric")
    }
}

impl FromContext<HttpContext> for ApiVersion {
    type Error = MissingApiVersion;

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        // Reading the parts leaves the body alone, so this extractor sits
        // beside a body extractor on the same handler.
        ctx.request()
            .headers
            .get("x-api-version")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u8>().ok())
            .map(ApiVersion)
            .ok_or(MissingApiVersion)
    }
}

// Refusing, and answering, without the handler

/// A pipe returning a response was doing one of two jobs. This is the first:
/// refusing on policy. The rejection is a `GuardRejection`, which
/// `#[catch(GuardRejection)]` reshapes centrally rather than per-pipe.
#[injectable]
pub struct AdminGuard {}

#[async_trait]
impl Guard<HttpContext> for AdminGuard {
    async fn can_activate(&self, ctx: &HttpContext) -> bool {
        ctx.request()
            .headers
            .get("x-role")
            .and_then(|v| v.to_str().ok())
            == Some("admin")
    }
}

/// The second job: answering in place of the handler, with a response of your
/// own choosing. An interceptor that returns without calling `next` skips
/// everything downstream — and unlike a pipe it can await while deciding.
#[injectable]
pub struct MaintenanceWindow {}

#[async_trait]
impl Interceptor<HttpContext, HttpResponse> for MaintenanceWindow {
    async fn intercept(
        &self,
        ctx: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpResponse>>,
    ) -> HttpResponse {
        if ctx.request().headers.contains_key("x-maintenance") {
            return HttpResponse {
                status: 503,
                body: Some(Body::json(serde_json::json!({
                    "error": "closed for maintenance",
                }))),
                headers: vec![],
            };
        }
        next.run(ctx).await
    }
}

// HTTP

#[controller("/api")]
pub struct CatalogController {}

#[routes]
impl CatalogController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// `Path<u32>` is `ParseIntPipe`: the segment is parsed against the
    /// declared type, and `/api/products/abc` is a 400 the handler never sees.
    ///
    /// ```typescript
    /// @Get('products/:id')
    /// findOne(@Param('id', ParseIntPipe) id: number) {}
    /// ```
    #[get("/products/{id}")]
    fn find_one(&self, Path(id): Path<u32>) -> Body {
        Body::json(serde_json::json!({ "id": id }))
    }

    /// Defaults, a bool, an enum and a delimited list, all decided by the
    /// declared type of each field.
    #[get("/products")]
    fn list(&self, Query(q): Query<ListProducts>) -> Body {
        Body::json(serde_json::json!({
            "page": q.page,
            "archived": q.archived,
            "banned": q.status == Status::Banned,
            "ids": q.ids,
        }))
    }

    /// `@Body(ValidationPipe)`. Invalid input is refused with a 400 naming the
    /// field, before the handler is entered.
    #[post("/users")]
    fn create_user(&self, Validated(Json(dto)): Validated<Json<CreateUser>>) -> Body {
        Body::json(serde_json::json!({
            "email": dto.email,
            "city": dto.address.city,
            "age": dto.age,
        }))
    }

    /// `Validated` reads only what the extractor it wraps reads, so validating
    /// the query string here leaves the body for `Json`.
    #[post("/posts")]
    fn create_post(
        &self,
        Query(q): Query<ListProducts>,
        Json(post): Json<CreatePost>,
        ApiVersion(version): ApiVersion,
    ) -> Body {
        Body::json(serde_json::json!({
            "page": q.page,
            "slug": post.slug.0,
            "title": post.title,
            "version": version,
        }))
    }

    /// `Validated<Json<Vec<T>>>` is `ParseArrayPipe({items: Dto})`: every
    /// element is validated, and the first failure names its index.
    #[post("/users/bulk")]
    fn bulk(&self, Validated(Json(dtos)): Validated<Json<Vec<CreateUser>>>) -> Body {
        Body::json(serde_json::json!({ "accepted": dtos.len() }))
    }

    /// Refusal on policy sits with the guard, not with the input.
    #[get("/admin/report")]
    #[use_guards(AdminGuard)]
    fn report(&self) -> Body {
        Body::text("admin only".to_string())
    }

    /// Answering in place of the handler sits with the interceptor.
    #[get("/status")]
    #[use_interceptors(MaintenanceWindow)]
    fn status(&self) -> Body {
        Body::text("ok".to_string())
    }
}

// WebSocket

/// The same wrapper, over the extractor that reads a frame. Nest would reach
/// for `@UsePipes(new ValidationPipe())` on the gateway and an
/// `exceptionFactory` to turn the HTTP exception into a `WsException`; here the
/// refusal is already a WebSocket error frame.
#[websocket_gateway("/ws")]
pub struct CatalogGateway {}

#[subscriptions]
impl CatalogGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("users.create")]
    async fn create_user(
        &self,
        _client: WsClient,
        Validated(Payload(dto)): Validated<Payload<CreateUser>>,
    ) -> WsHandlerResult {
        Ok(WsMessage::text(format!("created {}", dto.email)).into())
    }
}

// RPC

/// And over the payload of a call. One DTO, one set of rules, three transports.
#[rpc_controller]
pub struct CatalogRpcController {}

#[patterns]
impl CatalogRpcController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("users.create")]
    async fn create_user(
        &self,
        Validated(Payload(dto)): Validated<Payload<CreateUser>>,
    ) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!({ "email": dto.email })))
    }
}

#[module(
    controllers: [CatalogController, CatalogRpcController],
    providers: [AdminGuard, MaintenanceWindow, CatalogGateway],
)]
impl CatalogModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = toni::ToniFactory::create(CatalogModule).await?;

    app.use_http_adapter(toni_axum::AxumAdapter::new(), ("127.0.0.1", 3000))?;
    app.use_rpc_adapter(toni_tcp::TcpAdapter::new("127.0.0.1", 3001))?;

    println!("HTTP on http://127.0.0.1:3000, RPC on 127.0.0.1:3001, WebSocket at /ws");
    println!();
    println!("  GET  /api/products/42                     typed path parameter");
    println!("  GET  /api/products/abc                    400, the handler is never entered");
    println!("  GET  /api/products?ids=1,2,3&status=banned defaults, enum, delimited list");
    println!("  POST /api/users                           validated body");
    println!("  POST /api/users/bulk                      validated array body");
    println!("  POST /api/posts                           query + body + custom extractor");
    println!("  GET  /api/admin/report                    needs x-role: admin");
    println!("  GET  /api/status                          send x-maintenance to be answered 503");

    app.start().await?;
    Ok(())
}
