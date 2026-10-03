use std::panic::Location;

use crate::key::Key;

/// The execution inputs one transport seeds, written by [`Transport::inputs`](crate::Transport::inputs)
/// (transports DESIGN §2.10, X4).
///
/// Each key is recorded with the transport as its seeder; a report naming where the input was
/// declared prints "declared by transport `Http`" rather than a module.
pub struct Inputs {
    pub(crate) keys: Vec<InputKey>,
}

/// One input a transport declared, with the call that declared it.
#[derive(Clone, Copy)]
pub(crate) struct InputKey {
    pub(crate) key: Key,
    pub(crate) location: &'static Location<'static>,
}

impl Inputs {
    pub(crate) fn new() -> Self {
        Inputs { keys: Vec::new() }
    }

    /// The input `T`, seeded with `Execution::seed` at every call of this transport.
    #[track_caller]
    pub fn input<T: Send + Sync + 'static>(&mut self) -> &mut Self {
        self.keys.push(InputKey { key: Key::of::<T, ()>(), location: Location::caller() });
        self
    }
}
