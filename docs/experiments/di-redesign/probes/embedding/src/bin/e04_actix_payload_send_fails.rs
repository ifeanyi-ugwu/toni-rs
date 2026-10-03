//! actix's request payload is `!Send`: `BoxedPayloadStream` is `Pin<Box<dyn Stream<..>>>` with no
//! `Send` bound. The compile error is the result.

fn is_send<T: Send>() {}

fn main() {
    is_send::<actix_web::dev::Payload>();
}
