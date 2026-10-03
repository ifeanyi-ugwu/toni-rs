//! P25 (transports §2.2): the single-body-consumer check as one `const` assertion per parameter
//! pair, each message a literal naming both parameters. A macro reads `CONSUMES_BODY` off the
//! type, never off its spelling, so no pair is skipped: n parameters emit n(n-1)/2 assertions of
//! one `&&` each. Expected: compiles, prints "3 params, 3 pairs checked".
pub trait Param: Sized + Send + 'static {
    const CONSUMES_BODY: bool = false;
}
pub struct Path<T>(pub T);
pub struct Query<T>(pub T);
pub struct Json<T>(pub T);
impl<T: Send + 'static> Param for Path<T> {}
impl<T: Send + 'static> Param for Query<T> {}
impl<T: Send + 'static> Param for Json<T> {
    const CONSUMES_BODY: bool = true;
}
/// A user alias: the check reads the type, so it is counted as what it is.
pub type Body<T> = Json<T>;

// handler: fn create(&self, id: Path<u64>, q: Query<Page>, user: Body<NewUser>)
const _: () = assert!(
    !(<Path<u64> as Param>::CONSUMES_BODY && <Query<u8> as Param>::CONSUMES_BODY),
    "`id` and `q` both consume the body; a handler reads the body once"
);
const _: () = assert!(
    !(<Path<u64> as Param>::CONSUMES_BODY && <Body<u8> as Param>::CONSUMES_BODY),
    "`id` and `user` both consume the body; a handler reads the body once"
);
const _: () = assert!(
    !(<Query<u8> as Param>::CONSUMES_BODY && <Body<u8> as Param>::CONSUMES_BODY),
    "`q` and `user` both consume the body; a handler reads the body once"
);

fn main() {
    println!("3 params, 3 pairs checked");
}
