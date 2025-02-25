use crate::injector::Context;

pub trait Pipe: Send + Sync {
    fn process(&self, data: &mut Context);
}
