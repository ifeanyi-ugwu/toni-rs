//! The upgrade hand-off through rocket's `IoHandler`: rocket takes the hyper upgrade itself when the
//! app's 101 names the protocol the request asked for, writes the two upgrade headers, and hands
//! the adapter an `IoStream`, which becomes the app's `Upgraded`.

use std::io;
use std::pin::Pin;

use rocket::data::{IoHandler, IoStream};
use tokio::sync::oneshot;
use ulo::BoxError;
use ulo_http::{OnUpgrade, Upgraded};

/// For a request asking for an upgrade: the app's upgrade future, and the I/O handler the response
/// carries when the app answers 101. Either half dropped resolves the other to an error.
pub(crate) fn pending(request: &rocket::Request<'_>) -> (Option<OnUpgrade>, Option<Handoff>) {
    if !request.headers().contains("upgrade") {
        return (None, None);
    }
    let (sender, receiver) = oneshot::channel::<IoStream>();
    let upgrade = OnUpgrade::new(async move {
        let io = receiver.await.map_err(|_| BoxError::from("rocket did not hand over the upgraded connection"))?;
        Ok::<_, BoxError>(Upgraded::new(io))
    });
    (Some(upgrade), Some(Handoff(sender)))
}

/// The `IoHandler` a 101 carries: it passes rocket's upgraded stream to the app's waiting
/// `OnUpgrade` and returns, the app driving the connection from then on.
pub(crate) struct Handoff(oneshot::Sender<IoStream>);

#[rocket::async_trait]
impl IoHandler for Handoff {
    async fn io(self: Pin<Box<Self>>, io: IoStream) -> io::Result<()> {
        let Handoff(sender) = *Pin::into_inner(self);
        sender
            .send(io)
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "the app no longer waits for the upgraded connection"))
    }
}
