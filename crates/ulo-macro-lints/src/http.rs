//! `#[routes]` over every HTTP verb attribute, with the extractors, the reply forms, the enhancer
//! spellings at both tiers, `#[meta]`, a generic controller and `self: Arc<Self>`.

use std::marker::PhantomData;
use std::sync::Arc;

use futures_util::stream::{self, BoxStream};
use serde::Deserialize;
use ulo::{Dep, injectable, routes};
use ulo_http::{Event, HttpCx, Json, Path, Query, Sse, Timeout};
use ulo_transport::Valid;

use crate::derives::{Item, Lookup, Refusal};
use crate::di::Store;
use crate::enhancers::{Allow, Pass, Relay, Tag};

#[derive(Debug, Deserialize)]
pub struct Page {
    pub page: u32,
}

#[injectable]
pub struct Pages {
    pub store: Dep<Store>,
}

#[routes]
#[guards(Allow, http = Allow, value = Allow, with = || Allow)]
#[interceptors(Pass, value = Pass)]
#[error_handlers(Relay, value = Relay)]
#[meta(Tag("pages"), Timeout::after(std::time::Duration::from_secs(5)))]
impl Pages {
    #[ulo_http::get("/")]
    fn index(&self) -> &'static str {
        "index"
    }

    #[ulo_http::get("/items/{id}")]
    async fn item(&self, id: Path<u32>, page: Query<Page>) -> Result<String, Lookup> {
        if id.0 == 0 { Err(Lookup::Missing) } else { Ok(format!("{}:{}", id.0, page.0.page)) }
    }

    #[ulo_http::post("/items")]
    #[guards(value = Allow)]
    #[interceptors(with = || Pass)]
    #[error_handlers(Relay)]
    #[meta(Tag("create"))]
    async fn create(&self, item: Valid<Json<Item>>) -> Result<Json<String>, Refusal> {
        Ok(Json(item.into_inner().0.name))
    }

    #[ulo_http::put("/items/{id}")]
    fn replace(&self, id: Path<u32>, cx: HttpCx) -> String {
        format!("{} {}", id.0, cx.exec().is_draining())
    }

    #[ulo_http::patch("/items/{id}")]
    fn amend(&self, id: Path<u32>) -> String {
        id.0.to_string()
    }

    #[ulo_http::delete("/items/{id}")]
    async fn remove(self: Arc<Self>, id: Path<u32>) -> Result<(), Refusal> {
        let _ = (self.store.hits, id.0);
        Ok(())
    }

    #[ulo_http::head("/items")]
    fn probe(&self) {}

    #[ulo_http::options("/items")]
    fn allowed(&self) -> &'static str {
        "GET, POST"
    }

    #[ulo_http::get("/events")]
    fn events(&self) -> Sse<BoxStream<'static, Event>> {
        Sse::new(Box::pin(stream::iter([Event::default().data("one")])))
    }
}

/// A generic controller.
#[injectable]
pub struct Generic<T: Send + Sync + 'static> {
    #[injectable(default)]
    pub marker: PhantomData<fn() -> T>,
}

#[routes]
impl<T: Send + Sync + 'static> Generic<T> {
    #[ulo_http::get("/generic")]
    fn generic(&self) -> &'static str {
        std::any::type_name::<T>()
    }
}
