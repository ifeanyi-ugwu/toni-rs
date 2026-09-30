//! P09: `providers = [AppConfig::from_env()?]` inside a synchronous `register(&self, ..)` that
//! returns `()`. Expected: E0277, `?` in a function returning `()`.
pub struct ModuleDef;
impl ModuleDef { pub fn value<T>(&mut self, _v: T) {} }
pub struct AppConfig;
impl AppConfig { pub fn from_env() -> Result<Self, String> { Ok(AppConfig) } }
pub trait Module { fn register(&self, m: &mut ModuleDef); }
pub struct ConfigModule;
impl Module for ConfigModule {
    fn register(&self, m: &mut ModuleDef) { m.value(AppConfig::from_env()?); }
}
fn main() {}
