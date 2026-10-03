//! P23 (transports §2.2): `Param<T>` for the core's injection-point types and the transport's own
//! extractors, with a user `FromContainer` type accepted bare through autoref: rank one is
//! `P: Param<T>`, rank two wraps `S: FromContainer` as `Injected<S>`. `Option<P>` is a `Param`
//! over `Param`, not over `FromContainer`: the two blanket impls would overlap (P23b), and the
//! `Param` one covers `Option<Dep<U>>` because `Dep<U>` is a `Param`. The transport is a parameter
//! of the probe, not of the method, since lookup checks an impl's where-clauses and not a
//! method's. Expected: compiles, prints the four lines in the match arms below.
use std::marker::PhantomData;

pub trait Transport: 'static {}
pub struct Http;
impl Transport for Http {}

/// The core's trait, as built.
pub trait FromContainer: Sized + Send + 'static {
    fn name() -> &'static str;
}
/// The transport layer's trait.
pub trait Param<T: Transport>: Sized + Send + 'static {
    const CONSUMES_BODY: bool = false;
    fn describe() -> String;
}

pub struct Dep<U>(PhantomData<fn() -> U>);
pub struct Json<U>(PhantomData<fn() -> U>);
pub struct Session;
pub struct Injected<S>(pub S);

impl<U: 'static> FromContainer for Dep<U> {
    fn name() -> &'static str {
        "Dep"
    }
}
impl FromContainer for Session {
    fn name() -> &'static str {
        "Session"
    }
}
impl<S: FromContainer> FromContainer for Option<S> {
    fn name() -> &'static str {
        "Option<FromContainer>"
    }
}

// One concrete impl per core injection-point type.
impl<T: Transport, U: 'static> Param<T> for Dep<U> {
    fn describe() -> String {
        "param: Dep".into()
    }
}
// The transport's own extractor.
impl<U: 'static> Param<Http> for Json<U> {
    const CONSUMES_BODY: bool = true;
    fn describe() -> String {
        "param: Json".into()
    }
}
// `Option` over `Param`, forwarding the body flag.
impl<T: Transport, P: Param<T>> Param<T> for Option<P> {
    const CONSUMES_BODY: bool = P::CONSUMES_BODY;
    fn describe() -> String {
        format!("param: Option<{}>", P::describe())
    }
}
// The wrapper rank two produces.
impl<T: Transport, S: FromContainer> Param<T> for Injected<S> {
    fn describe() -> String {
        format!("injected: {}", S::name())
    }
}

pub struct ParamProbe<T, X>(PhantomData<fn() -> (T, X)>);
impl<T, X> ParamProbe<T, X> {
    pub fn new() -> Self {
        ParamProbe(PhantomData)
    }
}
pub trait ViaParam {
    fn kind(&self) -> String;
}
impl<T: Transport, P: Param<T>> ViaParam for ParamProbe<T, P> {
    fn kind(&self) -> String {
        P::describe()
    }
}
pub trait ViaContainer {
    fn kind(&self) -> String;
}
impl<T: Transport, S: FromContainer> ViaContainer for &ParamProbe<T, S> {
    fn kind(&self) -> String {
        <Injected<S> as Param<T>>::describe()
    }
}

fn main() {
    // What the macro writes at each parameter, the transport known, the type as written.
    println!("{}", (&ParamProbe::<Http, Dep<u32>>::new()).kind()); // param: Dep
    println!("{}", (&ParamProbe::<Http, Option<Json<u32>>>::new()).kind()); // param: Option<param: Json>
    println!("{}", (&ParamProbe::<Http, Session>::new()).kind()); // injected: Session
    println!("{}", (&ParamProbe::<Http, Option<Session>>::new()).kind()); // injected: Option<FromContainer>
    println!("consumes: {}", <Option<Json<u32>> as Param<Http>>::CONSUMES_BODY); // true
}
