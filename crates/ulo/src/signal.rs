use std::borrow::Cow;
use std::fmt;

/// The name of what started a shutdown: an OS signal (`"SIGTERM"`) or a reason a job gives
/// `close` (`"done"`).
///
/// The first trigger's signal is the one `BeforeApplicationShutdown` and `OnApplicationShutdown`
/// receive and the one [`Shutdown::signal`](crate::Shutdown) reports, so a `close` caller whose
/// own signal lost learns what ended the app.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Signal {
    name: Cow<'static, str>,
}

impl Signal {
    pub fn new(name: impl Into<Cow<'static, str>>) -> Self {
        Signal { name: name.into() }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name)
    }
}
