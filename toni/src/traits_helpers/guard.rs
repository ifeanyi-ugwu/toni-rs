use crate::injector::Context;

pub trait Guard: Send + Sync {
    fn can_activate(&self, context: &Context) -> bool;
}
