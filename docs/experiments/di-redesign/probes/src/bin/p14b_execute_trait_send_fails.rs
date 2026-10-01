//! P14b: the same `execute` as a trait method. A trait has to write the `Send` bound on its
//! returned future, and the impl's body then needs `F`'s call future to be `Send` for every `F`,
//! which no stable bound can state: the only name for it is the unstable
//! `AsyncFnOnce::CallOnceFuture`. Expected: two errors, the future not `Send` at the impl, and
//! E0658 on the bound that names the associated type.
use std::future::Future;

pub struct Execution { pub name: &'static str }
impl Execution { pub async fn get(&self) -> Result<&'static str, ()> { Ok(self.name) } }

pub trait Exec {
    fn execute<F, R>(&self, f: F) -> impl Future<Output = R> + Send
    where
        F: AsyncFnOnce(&Execution) -> R + Send,
        R: Send;
}

pub struct App;
impl Exec for App {
    fn execute<F, R>(&self, f: F) -> impl Future<Output = R> + Send
    where
        F: AsyncFnOnce(&Execution) -> R + Send,
        R: Send,
    {
        async move {
            let exec = Execution { name: "borrowed" };
            f(&exec).await
        }
    }
}

pub trait ExecBounded {
    fn execute<F, R>(&self, f: F) -> impl Future<Output = R> + Send
    where
        F: AsyncFnOnce(&Execution) -> R + Send,
        for<'a> <F as AsyncFnOnce<(&'a Execution,)>>::CallOnceFuture: Send,
        R: Send;
}

fn main() {}
