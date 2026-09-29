// A lifecycle hook runs on an instance the framework holds. It holds no execution-scoped or
// transient one, so `#[injectable]` refuses the hook at the scope, naming both.

#[ulo::injectable(scope = "execution")]
pub struct Session {}

impl Session {
    #[ulo::on_module_init]
    async fn init(&self) -> ulo::di::InitResult {
        Ok(())
    }
}

#[ulo::injectable(scope = "transient")]
pub struct Draft {}

impl Draft {
    #[ulo::on_module_destroy]
    async fn destroy(&self) {}
}

fn main() {}
