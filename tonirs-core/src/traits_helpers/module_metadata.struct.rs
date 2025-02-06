use super::{Controller, Provider};
pub trait ModuleMetadata: Send + Sync {
    fn get_id(&self) -> String;
    fn get_name(&self) -> String;
    fn imports(&self) -> Option<Vec<Box<dyn ModuleMetadata>>>;
    fn controllers(&self) -> Option<Vec<Box<dyn Controller>>>;
    fn providers(&self) -> Option<Vec<Box<dyn Provider>>>;
    fn exports(&self) -> Option<Vec<String>>;
}