//! A bounded queue that keeps the order its sends were called in, without a runtime: the frames
//! one link side writes. `ulo-rpc`'s TCP, UDP, Redis, RabbitMQ and Kafka links feed their writers
//! through one.
//!
//! [`Sender::send`] puts its item in the queue at the call, synchronously, and returns a [`Room`]
//! that resolves once the item is among the first `bound` the receiver has not taken: the bound
//! is applied after the place is fixed. A bounded channel whose send waits for a permit before it
//! pushes fixes the place when the permit is used, so a send waiting for room can be overtaken by
//! one called after it that finds a place freed.
//!
//! An item stays queued when its `Room` is dropped, and the receiver takes it in its turn. The
//! queue holds every item whose send was called and not yet taken, so its length past `bound` is
//! the number of sends waiting for room.

use std::collections::VecDeque;
use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use event_listener::{Event, EventListener};

/// A queue bounded at `bound` items in flight to the receiver, `bound` at least one.
pub fn channel<T>(bound: usize) -> (Sender<T>, Receiver<T>) {
    let inner = Arc::new(Inner {
        state: Mutex::new(State { queue: VecDeque::new(), pushed: 0, taken: 0, senders: 1, receiving: true }),
        bound: bound.max(1),
        room: Event::new(),
        item: Event::new(),
    });
    (Sender { inner: Arc::clone(&inner) }, Receiver { inner, listener: None })
}

struct Inner<T> {
    state: Mutex<State<T>>,
    bound: usize,
    /// Wakes the sends waiting for room, each time the receiver takes an item.
    room: Event,
    /// Wakes the receiver, at each item and when the last sender goes.
    item: Event,
}

struct State<T> {
    queue: VecDeque<T>,
    /// Items ever queued; the next send's place.
    pushed: u64,
    /// Items ever taken by the receiver.
    taken: u64,
    senders: usize,
    receiving: bool,
}

impl<T> Inner<T> {
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The receiver has gone; the item was dropped with the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closed;

impl fmt::Display for Closed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the queue's receiver has gone")
    }
}

impl std::error::Error for Closed {}

/// Why [`Sender::try_send`] refused its item.
#[derive(Debug, PartialEq, Eq)]
pub enum TrySendError<T> {
    /// `bound` items are waiting for the receiver.
    Full(T),
    Closed(T),
}

/// The sending side; cheap to clone, every clone the same queue.
pub struct Sender<T> {
    inner: Arc<Inner<T>>,
}

impl<T> Sender<T> {
    /// Queues `item` now, behind every item whose send was called before, and answers a future
    /// that resolves once it is among the first `bound` items not yet taken, or with [`Closed`]
    /// when the receiver goes first.
    pub fn send(&self, item: T) -> Room<T> {
        let mut state = self.inner.lock();
        if !state.receiving {
            return Room { inner: Arc::clone(&self.inner), place: None, listener: None };
        }
        let place = state.pushed;
        state.pushed += 1;
        state.queue.push_back(item);
        drop(state);
        self.inner.item.notify(1);
        Room { inner: Arc::clone(&self.inner), place: Some(place), listener: None }
    }

    /// Queues `item` when fewer than `bound` items wait for the receiver.
    pub fn try_send(&self, item: T) -> Result<(), TrySendError<T>> {
        let mut state = self.inner.lock();
        if !state.receiving {
            return Err(TrySendError::Closed(item));
        }
        if state.queue.len() >= self.inner.bound {
            return Err(TrySendError::Full(item));
        }
        state.pushed += 1;
        state.queue.push_back(item);
        drop(state);
        self.inner.item.notify(1);
        Ok(())
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        self.inner.lock().senders += 1;
        Sender { inner: Arc::clone(&self.inner) }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut state = self.inner.lock();
        state.senders -= 1;
        let last = state.senders == 0;
        drop(state);
        if last {
            self.inner.item.notify(usize::MAX);
        }
    }
}

/// A queued item's wait for room. Dropping it leaves the item queued.
pub struct Room<T> {
    inner: Arc<Inner<T>>,
    /// `None` for an item refused because the receiver had gone.
    place: Option<u64>,
    listener: Option<EventListener>,
}

impl<T> Future for Room<T> {
    type Output = Result<(), Closed>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let room = self.get_mut();
        let Some(place) = room.place else { return Poll::Ready(Err(Closed)) };
        loop {
            {
                let state = room.inner.lock();
                if place < state.taken {
                    return Poll::Ready(Ok(()));
                }
                if !state.receiving {
                    return Poll::Ready(Err(Closed));
                }
                if place < state.taken + room.inner.bound as u64 {
                    return Poll::Ready(Ok(()));
                }
            }
            // Registered, then the state read again: a take between the read and the listen
            // still wakes this wait.
            match room.listener.as_mut() {
                None => room.listener = Some(room.inner.room.listen()),
                Some(listener) => {
                    if Pin::new(listener).poll(cx).is_pending() {
                        return Poll::Pending;
                    }
                    room.listener = None;
                }
            }
        }
    }
}

/// The receiving side. Dropped, it drops every queued item and fails the sends waiting for room.
pub struct Receiver<T> {
    inner: Arc<Inner<T>>,
    listener: Option<EventListener>,
}

impl<T> Receiver<T> {
    /// The next item in the order the sends were called, or `None` once the queue is empty and
    /// every sender has gone. Cancel-safe: an item is taken only when the future answers it.
    pub fn recv(&mut self) -> impl Future<Output = Option<T>> + '_ {
        poll_fn(move |cx| self.poll_recv(cx))
    }

    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<T>> {
        loop {
            {
                let mut state = self.inner.lock();
                if let Some(item) = state.queue.pop_front() {
                    state.taken += 1;
                    drop(state);
                    self.inner.room.notify(usize::MAX);
                    self.listener = None;
                    return Poll::Ready(Some(item));
                }
                if state.senders == 0 {
                    return Poll::Ready(None);
                }
            }
            match self.listener.as_mut() {
                None => self.listener = Some(self.inner.item.listen()),
                Some(listener) => {
                    if Pin::new(listener).poll(cx).is_pending() {
                        return Poll::Pending;
                    }
                    self.listener = None;
                }
            }
        }
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        let dropped = {
            let mut state = self.inner.lock();
            state.receiving = false;
            std::mem::take(&mut state.queue)
        };
        // Dropped outside the lock: an item may own something whose drop reaches the queue.
        drop(dropped);
        self.inner.room.notify(usize::MAX);
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};

    use super::channel;

    fn poll_once<F: Future>(fut: std::pin::Pin<&mut F>) -> Poll<F::Output> {
        fut.poll(&mut Context::from_waker(Waker::noop()))
    }

    /// The race a bounded channel loses: a send waiting for room, then room freed for two, then a
    /// send called after it and polled before it is polled again. The first send's item still
    /// comes out first.
    #[test]
    fn a_send_waiting_for_room_keeps_its_place_ahead_of_a_later_one() {
        let (sender, mut receiver) = channel::<&str>(1);
        let mut filling = pin!(sender.send("filling"));
        assert!(poll_once(filling.as_mut()).is_ready(), "the first item fits the bound");
        let mut early = pin!(sender.send("early"));
        assert!(poll_once(early.as_mut()).is_pending(), "the second item waits for room");

        assert_eq!(poll_once(pin!(receiver.recv())), Poll::Ready(Some("filling")));
        // The receiver frees room once more before the waiting send runs again.
        let mut late = pin!(sender.send("late"));
        assert!(poll_once(late.as_mut()).is_pending(), "the late item waits behind the early one");
        assert_eq!(poll_once(pin!(receiver.recv())), Poll::Ready(Some("early")), "a later send took the earlier one's place");
        assert!(poll_once(early.as_mut()).is_ready());
        assert!(poll_once(late.as_mut()).is_ready());
        assert_eq!(poll_once(pin!(receiver.recv())), Poll::Ready(Some("late")));
    }

    #[test]
    fn an_item_whose_wait_is_dropped_is_still_received() {
        let (sender, mut receiver) = channel::<u32>(1);
        drop(sender.send(1));
        drop(sender.send(2));
        assert_eq!(poll_once(pin!(receiver.recv())), Poll::Ready(Some(1)));
        assert_eq!(poll_once(pin!(receiver.recv())), Poll::Ready(Some(2)));
        drop(sender);
        assert_eq!(poll_once(pin!(receiver.recv())), Poll::Ready(None));
    }

    #[test]
    fn a_receiver_gone_fails_the_waiting_sends_and_refuses_new_ones() {
        let (sender, receiver) = channel::<u32>(1);
        drop(sender.send(1));
        let mut waiting = pin!(sender.send(2));
        assert!(poll_once(waiting.as_mut()).is_pending());
        drop(receiver);
        assert_eq!(poll_once(waiting.as_mut()), Poll::Ready(Err(super::Closed)));
        assert_eq!(poll_once(pin!(sender.send(3))), Poll::Ready(Err(super::Closed)));
        assert!(matches!(sender.try_send(4), Err(super::TrySendError::Closed(4))));
    }

    #[test]
    fn try_send_refuses_past_the_bound() {
        let (sender, _receiver) = channel::<u32>(1);
        assert_eq!(sender.try_send(1), Ok(()));
        assert!(matches!(sender.try_send(2), Err(super::TrySendError::Full(2))));
    }
}
