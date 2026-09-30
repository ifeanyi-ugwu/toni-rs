//! P03: `trait OnModuleInit: Construct<Scope: HookCapable>` as a supertrait with an
//! associated-type bound; a user's `async fn` in an impl of a trait declaring
//! `-> impl Future<Output = ..> + Send`. Expected: compiles.
use std::future::Future;
mod sealed { pub trait Sealed {} }
pub trait Scope: sealed::Sealed + 'static {}
pub struct Singleton; pub struct PerExecution; pub struct Auto;
impl sealed::Sealed for Singleton {} impl Scope for Singleton {}
impl sealed::Sealed for PerExecution {} impl Scope for PerExecution {}
impl sealed::Sealed for Auto {} impl Scope for Auto {}

#[diagnostic::on_unimplemented(
    message = "lifecycle hooks are only allowed on singletons",
    label = "`{Self}` scope cannot have hooks",
    note = "remove the scope argument from #[injectable], or move the hook to a singleton"
)]
pub trait HookCapable: Scope {}
impl HookCapable for Singleton {} impl HookCapable for Auto {}

pub trait Construct: Sized + Send + Sync + 'static { type Scope: Scope; }
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

pub trait OnModuleInit: Construct<Scope: HookCapable> {
    fn on_module_init(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}

pub struct Cache;
impl Construct for Cache { type Scope = Auto; }
impl OnModuleInit for Cache {
    async fn on_module_init(&self) -> Result<(), BoxError> { Ok(()) }
}

fn assert_send<F: Future + Send>(_: F) {}
fn main() {
    let c = Cache;
    assert_send(c.on_module_init());
    println!("ok");
}
