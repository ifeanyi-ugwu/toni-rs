//! P03b: a hook on an explicitly execution-scoped type. Expected: E0277 at the impl, carrying
//! the `HookCapable` message with `{Self}` rendered as `PerExecution`.
use std::future::Future;
mod sealed { pub trait Sealed {} }
pub trait Scope: sealed::Sealed + 'static {}
pub struct Singleton; pub struct PerExecution;
impl sealed::Sealed for Singleton {} impl Scope for Singleton {}
impl sealed::Sealed for PerExecution {} impl Scope for PerExecution {}
#[diagnostic::on_unimplemented(
    message = "lifecycle hooks are only allowed on singletons",
    label = "`{Self}` scope cannot have hooks",
    note = "remove the scope argument from #[injectable], or move the hook to a singleton"
)]
pub trait HookCapable: Scope {}
impl HookCapable for Singleton {}
pub trait Construct: Sized + Send + Sync + 'static { type Scope: Scope; }
pub trait OnModuleInit: Construct<Scope: HookCapable> {
    fn on_module_init(&self) -> impl Future<Output = ()> + Send;
}
pub struct AuditContext;
impl Construct for AuditContext { type Scope = PerExecution; }
impl OnModuleInit for AuditContext {
    async fn on_module_init(&self) {}
}
fn main() {}
