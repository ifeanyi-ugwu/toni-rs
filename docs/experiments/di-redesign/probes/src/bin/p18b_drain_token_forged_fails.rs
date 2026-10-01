//! P18b: user code writing the token itself. Expected: fails, E0423 — the tuple struct's field is
//! private, so the one spelling that would build a `DrainToken` outside the core is refused.
mod core {
    #[derive(Clone)]
    pub struct DrainToken(());
    pub struct App;
    pub struct Execution;
    impl Execution {
        pub fn open_terminal(_proof: &DrainToken, _app: &App) -> Execution { Execution }
    }
}

mod user {
    use super::core::*;
    pub fn sneak(app: &App) -> Execution {
        let forged = DrainToken(());
        Execution::open_terminal(&forged, app)
    }
}

fn main() { let _ = user::sneak(&core::App); }
