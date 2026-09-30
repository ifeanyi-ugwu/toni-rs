//! P06: the per-site `AllowedIn<S>` assertion as the macro would emit it, with the design's
//! message. Expected: E0277 at the assertion; records what `{Self}` renders to (the site type,
//! not the declaring type) and whether `{S}` is available.
use std::sync::Arc;
pub struct Singleton; pub struct PerExecution;
pub trait Scope {} impl Scope for Singleton {} impl Scope for PerExecution {}
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot read per-execution data",
    label = "a `{S}` type cannot read this site",
    note = "declare the type #[injectable(execution)] or #[injectable(transient)]"
)]
pub trait AllowedIn<S: Scope> {}
pub struct Dep<T: ?Sized>(Arc<T>);
pub struct Ext<T>(Arc<T>);
impl<T: ?Sized, S: Scope> AllowedIn<S> for Dep<T> {}
impl<T> AllowedIn<PerExecution> for Ext<T> {}
pub struct CurrentUser;
pub struct ReportService { user: Ext<CurrentUser>, repo: Dep<String> }
const _: () = {
    fn assert_allowed<S: AllowedIn<Singleton>>() {}
    let _ = assert_allowed::<Ext<CurrentUser>>;
    let _ = assert_allowed::<Dep<String>>;
};
fn main() { let _ = ReportService { user: Ext(Arc::new(CurrentUser)), repo: Dep(Arc::new(String::new())) }; }
