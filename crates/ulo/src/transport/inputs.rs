use std::panic::Location;

use crate::key::Key;
use crate::transport::Transport;
use crate::type_name::TypeName;

/// The execution inputs one transport seeds, written by [`Transport::inputs`](crate::Transport::inputs)
/// (transports DESIGN §2.10, X4).
///
/// Each key is recorded with the transport as its seeder and
/// [`InputOrigin::Transport`](crate::InputOrigin::Transport) as its origin, which a report prints
/// as "declared by transport `Http`" rather than a module.
pub struct Inputs {
    pub(crate) keys: Vec<InputKey>,
}

/// One input a transport declared, with the call that declared it.
#[derive(Clone)]
pub(crate) struct InputKey {
    pub(crate) key: Key,
    /// The other transports that seed the key, from `also_seeded_by`; the declaring transport is
    /// not among them.
    pub(crate) also: Vec<TypeName>,
    pub(crate) location: &'static Location<'static>,
}

impl Inputs {
    pub(crate) fn new() -> Self {
        Inputs { keys: Vec::new() }
    }

    /// The input `T`, seeded with `Execution::seed` at every call of this transport.
    #[track_caller]
    pub fn input<T: Send + Sync + 'static>(&mut self) -> &mut Self {
        self.keys.push(InputKey { key: Key::of::<T, ()>(), also: Vec::new(), location: Location::caller() });
        self
    }

    /// Another transport that seeds the input written last, for an input several transports seed
    /// (X19): `Ws::inputs` writes `d.input::<SessionHandle>().also_seeded_by::<WsConnect>()` and
    /// `WsConnect::inputs` the mirror.
    ///
    /// The freeze merges two transports' declarations of one key into one when their seeder sets
    /// are equal, the declaring transport counted in each; any other pair is
    /// `WiringError::InputConflict`. A handler of any seeder passes the input check. Written
    /// before any `input`, it declares nothing.
    pub fn also_seeded_by<U: Transport>(&mut self) -> &mut Self {
        if let Some(last) = self.keys.last_mut() {
            let seeder = TypeName::of::<U>();
            if !last.also.contains(&seeder) {
                last.also.push(seeder);
            }
        }
        self
    }
}
