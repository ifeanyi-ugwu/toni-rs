/// A count a server setting bounds, where "the default" and "no limit" are distinct answers. Each
/// setting taking one documents what its `Default` is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Count {
    /// The setting's own default, which may itself be a limit or none.
    #[default]
    Default,
    Max(u32),
    Unlimited,
}

impl Count {
    /// What `Count::Default` means for every transport's `max_inflight`: HTTP's, RPC's, gRPC's and
    /// a WebSocket gateway's.
    pub const DEFAULT_MAX_INFLIGHT: u32 = 1024;

    /// This count read as a `max_inflight`: [`DEFAULT_MAX_INFLIGHT`](Self::DEFAULT_MAX_INFLIGHT)
    /// for `Default`, `n` for `Max(n)`, and `None`, no bound, for `Unlimited`.
    pub fn max_inflight(self) -> Option<usize> {
        let calls = match self {
            Count::Default => Count::DEFAULT_MAX_INFLIGHT,
            Count::Max(calls) => calls,
            Count::Unlimited => return None,
        };
        Some(usize::try_from(calls).unwrap_or(usize::MAX))
    }
}
