//! Bridge between a `#[controller]` struct and its optional `#[routes]` impl.
//!
//! `#[controller]` emits the factory and the `Controller` object on the struct; the object's
//! `routes()` calls `Self::__toni_routes(&state)` at the concrete type. `#[routes]` emits an inherent
//! `__toni_routes` that out-ranks the blanket empty default below. So a controller with a `#[routes]`
//! impl exposes its handlers, and one without is a valid controller with no routes — the struct macro
//! dispatches to the routes, it doesn't detect them.
//!
//! The call sits at a concrete-type site (the generated object names the struct); inherent-wins
//! resolution is a property of that site, not available through a generic `T`.

#![doc(hidden)]

use std::sync::Arc;

use crate::traits_helpers::{ControllerInstance, Route};

/// Blanket "no routes" default, implemented for every type. `#[routes]` shadows this with an inherent
/// `__toni_routes` of the same name, which wins at the call site.
pub trait RoutesBridge {
    fn __toni_routes(_state: &ControllerInstance) -> Vec<Arc<dyn Route>> {
        Vec::new()
    }
}

impl<T: ?Sized> RoutesBridge for T {}
