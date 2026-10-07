//! `#[injectable]` on a struct and on an impl, with each scope, a timeout, `#[construct]`, an
//! `async` fallible constructor and a lifecycle hook.

use std::time::Duration;

use ulo::{BoxError, Dep, OnModuleInit, injectable};

/// A struct-form injectable with no dependency.
#[injectable(singleton)]
pub struct Clock;

/// A struct-form injectable with a dependency and a defaulted field.
#[injectable]
pub struct Store {
    pub clock: Dep<Clock>,
    #[injectable(default)]
    pub hits: u64,
}

/// An impl-form injectable whose constructor is `new`, `async` and fallible, with a timeout and a
/// lifecycle hook.
pub struct Named {
    pub name: String,
}

#[injectable(execution, timeout = Duration::from_secs(1))]
impl Named {
    pub async fn new(clock: Dep<Clock>) -> Result<Self, BoxError> {
        let _ = clock;
        Ok(Named { name: "named".to_owned() })
    }
}

/// An impl-form injectable whose constructor is marked `#[construct]`.
pub struct Defaulted {
    pub store: Dep<Store>,
}

#[injectable(transient)]
impl Defaulted {
    #[construct]
    pub fn build(store: Dep<Store>) -> Self {
        Defaulted { store }
    }
}

impl OnModuleInit for Clock {
    async fn on_module_init(&self) -> Result<(), BoxError> {
        Ok(())
    }
}
