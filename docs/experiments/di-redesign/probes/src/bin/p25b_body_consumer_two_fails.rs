//! P25b (§2.2): P25 with two consumers, the second written through an alias. Expected: E0080 at
//! the second assertion, carrying "`user` and `login` both consume the body".
pub trait Param: Sized + Send + 'static {
    const CONSUMES_BODY: bool = false;
}
pub struct Path<T>(pub T);
pub struct Json<T>(pub T);
pub struct Form<T>(pub T);
impl<T: Send + 'static> Param for Path<T> {}
impl<T: Send + 'static> Param for Json<T> {
    const CONSUMES_BODY: bool = true;
}
impl<T: Send + 'static> Param for Form<T> {
    const CONSUMES_BODY: bool = true;
}
pub type Login<T> = Form<T>;

// handler: fn create(&self, id: Path<u64>, user: Json<NewUser>, login: Login<Creds>)
const _: () = assert!(
    !(<Path<u64> as Param>::CONSUMES_BODY && <Json<u8> as Param>::CONSUMES_BODY),
    "`id` and `user` both consume the body; a handler reads the body once"
);
const _: () = assert!(
    !(<Json<u8> as Param>::CONSUMES_BODY && <Login<u8> as Param>::CONSUMES_BODY),
    "`user` and `login` both consume the body; a handler reads the body once"
);

fn main() {}
