use crate::injector::Context;

pub trait Interceptor: Send + Sync {
    fn before_execute(&self, context: &mut Context);
    fn after_execute(&self, context: &mut Context);
}