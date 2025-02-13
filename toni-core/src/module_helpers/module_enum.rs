use crate::traits_helpers::ModuleMetadata;

pub enum ModuleDefinition {
    DefaultModule(Box<dyn ModuleMetadata>),
}
