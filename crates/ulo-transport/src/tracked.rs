//! Clean end versus cut off (transports DESIGN §2.6, X5).

use std::pin::Pin;
use std::task::{Context, Poll};

use futures_core::Stream;
use pin_project_lite::pin_project;
use ulo::{ExecutionRef, StreamOutcome};

pin_project! {
    /// Every streaming answer, wrapped: it records how the reply stream ended and reports it once
    /// through `ExecutionRef::report_stream_end`, which runs the execution's `on_stream_end`
    /// callbacks.
    ///
    /// `Completed` when the inner stream returned `None` before this wrapper is dropped;
    /// `CutOff(reason)` when it is dropped before that, `reason` the execution's cancellation
    /// reason. A transport drops the wrapper once it has finished writing the stream's end, the
    /// terminating chunk, `END_STREAM`, an `end` frame or a `complete` envelope, so a stream whose
    /// end never reached the wire reports as cut off.
    pub struct Tracked<S> {
        #[pin]
        inner: S,
        exec: ExecutionRef,
        ended: bool,
    }

    impl<S> PinnedDrop for Tracked<S> {
        fn drop(this: Pin<&mut Self>) {
            let this = this.project();
            let outcome = if *this.ended {
                StreamOutcome::Completed
            } else {
                StreamOutcome::CutOff(this.exec.cancel_reason())
            };
            this.exec.report_stream_end(outcome);
        }
    }
}

impl<S> Tracked<S> {
    pub fn new(inner: S, exec: ExecutionRef) -> Self {
        Tracked { inner, exec, ended: false }
    }
}

impl<S: Stream> Stream for Tracked<S> {
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<S::Item>> {
        let this = self.project();
        let polled = this.inner.poll_next(cx);
        if let Poll::Ready(None) = polled {
            *this.ended = true;
        }
        polled
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}
