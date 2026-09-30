//! P03c: an `async fn` in the impl whose body holds a `!Send` value across an await, against a
//! trait declaring `+ Send`. Expected: E0277 naming the `Rc`, at the impl's fn.
use std::future::Future;
use std::rc::Rc;
pub trait OnModuleInit { fn on_module_init(&self) -> impl Future<Output = ()> + Send; }
pub struct Cache;
impl OnModuleInit for Cache {
    async fn on_module_init(&self) {
        let rc = Rc::new(1);
        std::future::ready(()).await;
        let _ = rc;
    }
}
fn main() {}
