//! P05: `Site::read(r: &Resolver<'_>) -> impl Future + Send` borrowing the resolver; `Option<S>`
//! forwarding; a generated `Construct::construct` awaiting two sites; and the erased constructor
//! slot the registry must hold, `for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, _>`.
//! Expected: compiles, with `Resolver: Sync` required for the future to be `Send`.
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub struct Execution { pub extensions: HashMap<TypeId, Arc<dyn Any + Send + Sync>> }
pub struct Resolver<'e> { pub store: &'e HashMap<TypeId, Arc<dyn Any + Send + Sync>>, pub exec: Option<&'e Execution> }
#[derive(Debug)] pub enum ResolveError { NotFound(&'static str), ExecutionRequired(&'static str) }

pub trait Site: Sized + Send + 'static {
    fn describe(d: &mut Vec<&'static str>);
    fn read(r: &Resolver<'_>) -> impl Future<Output = Result<Self, ResolveError>> + Send;
}
pub struct Dep<T: ?Sized>(pub Arc<T>);
impl<T: ?Sized + Send + Sync + 'static> Site for Dep<T> {
    fn describe(d: &mut Vec<&'static str>) { d.push(std::any::type_name::<T>()); }
    async fn read(r: &Resolver<'_>) -> Result<Self, ResolveError> {
        let slot = r.store.get(&TypeId::of::<T>()).ok_or(ResolveError::NotFound(std::any::type_name::<T>()))?;
        Ok(Dep(slot.clone().downcast::<Arc<T>>().map(|a| (*a).clone()).map_err(|_| ResolveError::NotFound("kind"))?))
    }
}
pub struct Ext<T>(pub Arc<T>);
impl<T: Send + Sync + 'static> Site for Ext<T> {
    fn describe(d: &mut Vec<&'static str>) { d.push("ext"); }
    async fn read(r: &Resolver<'_>) -> Result<Self, ResolveError> {
        let e = r.exec.ok_or(ResolveError::ExecutionRequired(std::any::type_name::<T>()))?;
        let v = e.extensions.get(&TypeId::of::<T>()).ok_or(ResolveError::NotFound("ext"))?;
        Ok(Ext(v.clone().downcast::<T>().map_err(|_| ResolveError::NotFound("ext kind"))?))
    }
}
impl<S: Site> Site for Option<S> {
    fn describe(d: &mut Vec<&'static str>) { S::describe(d); d.push("(optional)"); }
    async fn read(r: &Resolver<'_>) -> Result<Self, ResolveError> {
        match S::read(r).await { Ok(s) => Ok(Some(s)), Err(ResolveError::NotFound(_)) => Ok(None), Err(e) => Err(e) }
    }
}

pub trait Construct: Sized + Send + Sync + 'static {
    fn sites(s: &mut Vec<&'static str>);
    fn construct(r: &Resolver<'_>) -> impl Future<Output = Result<Self, ResolveError>> + Send;
}
pub struct Repo;
pub struct CurrentUser(pub &'static str);
pub struct Service { pub repo: Dep<Repo>, pub user: Option<Ext<CurrentUser>> }
impl Construct for Service {
    fn sites(s: &mut Vec<&'static str>) { <Dep<Repo>>::describe(s); <Option<Ext<CurrentUser>>>::describe(s); }
    async fn construct(r: &Resolver<'_>) -> Result<Self, ResolveError> {
        let repo = <Dep<Repo>>::read(r).await?;
        let user = <Option<Ext<CurrentUser>>>::read(r).await?;
        Ok(Service { repo, user })
    }
}

// The registry slot, and the helper that erases a `Construct` type into it. A closure written
// inline at the slot does not get the HRTB signature; the helper's declared type does.
pub type ErasedCtor = Arc<dyn for<'a> Fn(&'a Resolver<'a>) -> BoxFuture<'a, Result<Arc<dyn Any + Send + Sync>, ResolveError>> + Send + Sync>;
pub fn erase<T: Construct>() -> ErasedCtor {
    Arc::new(|r| Box::pin(async move { T::construct(r).await.map(|v| Arc::new(Arc::new(v)) as Arc<dyn Any + Send + Sync>) }))
}

fn main() {
    let mut store: HashMap<TypeId, Arc<dyn Any + Send + Sync>> = HashMap::new();
    store.insert(TypeId::of::<Repo>(), Arc::new(Arc::new(Repo)));
    let mut sites = Vec::new(); Service::sites(&mut sites);
    println!("sites: {sites:?}");
    let exec = Execution { extensions: HashMap::new() };
    let r = Resolver { store: &store, exec: Some(&exec) };
    let ctor = erase::<Service>();
    let fut = ctor(&r);
    fn assert_send<F: Future + Send>(f: F) -> F { f }
    let built = block_on(assert_send(fut)).expect("built");
    let svc = built.downcast::<Arc<Service>>().expect("Arc<Service>");
    println!("service built; user present={}", svc.user.is_some());
}
fn block_on<F: Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
    fn clone(_: *const ()) -> RawWaker { RawWaker::new(std::ptr::null(), &VT) }
    fn noop(_: *const ()) {}
    static VT: RawWakerVTable = RawWakerVTable::new(clone, noop, noop, noop);
    let waker = unsafe { Waker::from_raw(RawWaker::new(std::ptr::null(), &VT)) };
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop { if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) { return v; } }
}
