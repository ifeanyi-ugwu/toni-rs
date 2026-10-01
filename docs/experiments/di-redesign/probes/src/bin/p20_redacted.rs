//! P20: `Redacted` enforces what it claims. Built from a domain error whose message carries a
//! credential, it prints only the redacted text under `{}` and `{:?}`, answers `None` from
//! `source()` so a chain-walking reporter never reaches the original, answers
//! `downcast_ref::<TenantNotFound>()` with `Some` through an outer `LookupError`, gives the
//! original back through `into_inner`, and is `Send + Sync + 'static`, so it sits in a `BoxError`.
//! Expected: compiles; every assertion holds; prints the two renderings and the mapped status.
use std::error::Error;
use std::fmt;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

pub struct Redacted { inner: BoxError, text: String }

impl Redacted {
    pub fn downcast_ref<E: Error + 'static>(&self) -> Option<&E> { self.inner.downcast_ref::<E>() }
    pub fn into_inner(self) -> BoxError { self.inner }
}
impl fmt::Display for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.text) }
}
impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str(&self.text) }
}
impl Error for Redacted {
    fn source(&self) -> Option<&(dyn Error + 'static)> { None }
}

/// The one function every outside error passes through: the graph's registered secrets, then
/// the URL-userinfo strip as a backstop for a secret that was never registered.
pub fn redact(inner: BoxError, secrets: &[&str]) -> Redacted {
    let mut text = inner.to_string();
    for s in secrets { text = text.replace(s, "[redacted]"); }
    Redacted { inner, text: strip_userinfo(&text) }
}

fn strip_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + 3);
        out.push_str(head);
        let authority_end = tail.find(|c: char| c == '/' || c.is_whitespace()).unwrap_or(tail.len());
        let (authority, after) = tail.split_at(authority_end);
        match authority.rfind('@') {
            Some(i) => { out.push_str("[redacted]@"); out.push_str(&authority[i + 1..]); }
            None => out.push_str(authority),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[derive(Debug)]
pub struct TenantNotFound { pub tenant: String, pub url: String }
impl fmt::Display for TenantNotFound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tenant {} not found at {}", self.tenant, self.url)
    }
}
impl Error for TenantNotFound {}

#[derive(Debug)]
pub enum FailureReason { Errored(Redacted) }

/// The outer error an error handler receives inside a `BoxError`; its `source()` is the `Redacted`.
#[derive(Debug)]
pub enum LookupError { Construct { key: &'static str, reason: FailureReason } }
impl fmt::Display for LookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self { LookupError::Construct { key, .. } => write!(f, "building `{key}` inside the call failed") }
    }
}
impl Error for LookupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self { LookupError::Construct { reason: FailureReason::Errored(r), .. } => Some(r) }
    }
}

/// A reporter of the kind that prints every link of the chain.
fn report(e: &dyn Error) -> String {
    let mut s = e.to_string();
    let mut cur = e.source();
    while let Some(next) = cur { s.push_str(": "); s.push_str(&next.to_string()); cur = next.source(); }
    s
}

/// An error handler: a `BoxError` in, a status out.
fn handle(err: &BoxError) -> u16 {
    match err.downcast_ref::<LookupError>() {
        Some(LookupError::Construct { reason: FailureReason::Errored(r), .. }) if r.downcast_ref::<TenantNotFound>().is_some() => 404,
        _ => 500,
    }
}

fn assert_send_sync_static<T: Send + Sync + 'static>() {}

fn main() {
    const URL: &str = "postgres://admin:hunter2@db:5432/app";
    let original = TenantNotFound { tenant: "acme".into(), url: URL.into() };
    let original_text = original.to_string();
    let redacted = redact(Box::new(original), &[]);                 // the secret was never registered: the strip alone

    let display = format!("{redacted}");
    let debug = format!("{redacted:?}");
    assert_eq!(display, "tenant acme not found at postgres://[redacted]@db:5432/app");
    assert_eq!(debug, display);
    for s in ["hunter2", "admin"] { assert!(!display.contains(s) && !debug.contains(s), "{s} leaked"); }
    assert!(redacted.source().is_none());
    assert!(redacted.downcast_ref::<TenantNotFound>().is_some());

    let lookup = LookupError::Construct { key: "TenantCtx", reason: FailureReason::Errored(redacted) };
    let chain = report(&lookup);
    assert!(!chain.contains("hunter2"), "the chain reporter reached the original");
    let via_debug = format!("{lookup:?}");                          // the derived Debug of the outer error formats the `Redacted` through its own `Debug`
    assert!(!via_debug.contains("hunter2"), "a derived Debug reached the original");

    let boxed: BoxError = Box::new(lookup);
    let status = handle(&boxed);
    assert_eq!(status, 404);

    let Ok(lookup) = boxed.downcast::<LookupError>() else { unreachable!() };
    let LookupError::Construct { reason: FailureReason::Errored(redacted), .. } = *lookup;
    let inner = redacted.into_inner();
    assert_eq!(inner.to_string(), original_text);                   // the original, unredacted, for the caller that asked for it
    assert!(inner.downcast_ref::<TenantNotFound>().is_some());

    assert_send_sync_static::<Redacted>();
    let _: BoxError = Box::new(redact(Box::new(TenantNotFound { tenant: "x".into(), url: URL.into() }), &["hunter2"]));

    println!("display: {display}");
    println!("chain:   {chain}");
    println!("status:  {status}");
}
