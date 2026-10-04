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
