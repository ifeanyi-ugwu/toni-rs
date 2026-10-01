//! P18: `open_terminal` behind a capability only the core constructs. `DrainToken` has a private
//! field and no public constructor; the core hands one to `Server::drain` when Draining begins,
//! and `Execution::open_terminal` requires it. A transport in another module reaches it through
//! `drain`; user code in a third module has no expression that produces one (P18b). The phase
//! check stays in `open_terminal`, so the token is a proof of who, not of when. Expected: compiles.
mod core {
    #[derive(Clone)]
    pub struct DrainToken(());

    #[derive(Clone, Copy, PartialEq, Debug)]
    pub enum Phase { Running, Stopping, Draining, Destroying }
    pub struct App { pub phase: Phase }
    pub struct Closed;
    pub struct Execution { pub terminal: bool }

    impl Execution {
        pub fn open(app: &App) -> Result<Execution, Closed> {
            match app.phase {
                Phase::Running | Phase::Stopping => Ok(Execution { terminal: false }),
                Phase::Draining | Phase::Destroying => Err(Closed),
            }
        }
        pub fn open_terminal(_proof: &DrainToken, app: &App) -> Result<Execution, Closed> {
            match app.phase {
                Phase::Running | Phase::Stopping | Phase::Draining => Ok(Execution { terminal: true }),
                Phase::Destroying => Err(Closed),
            }
        }
    }

    pub trait Server {
        /// The token is owned, so a transport can hand a clone to each connection task for the
        /// length of the drain window.
        fn drain(&self, app: &App, token: DrainToken) -> usize;
    }

    pub fn run_drain(app: &mut App, servers: &[&dyn Server]) -> usize {
        app.phase = Phase::Draining;
        let token = DrainToken(());
        servers.iter().map(|s| s.drain(app, token.clone())).sum()
    }
}

mod fw_ws {
    use super::core::*;
    use std::cell::RefCell;
    pub struct WsServer { pub connections: usize, pub kept: RefCell<Option<DrainToken>> }
    impl Server for WsServer {
        fn drain(&self, app: &App, token: DrainToken) -> usize {
            *self.kept.borrow_mut() = Some(token.clone());
            // Every connection is closed with 1001; each disconnect handler is a terminal execution.
            (0..self.connections)
                .filter(|_| Execution::open_terminal(&token, app).is_ok_and(|e| e.terminal))
                .count()
        }
    }
    impl WsServer {
        /// A disconnect that arrives after the drain: the token is still held, the phase refuses.
        pub fn late_disconnect(&self, app: &App) -> bool {
            let kept = self.kept.borrow();
            Execution::open_terminal(kept.as_ref().expect("drained"), app).is_ok()
        }
    }
}

mod user {
    use super::core::*;
    pub fn job(app: &App) -> Result<Execution, Closed> { Execution::open(app) }
}

fn main() {
    use core::Phase;
    let mut app = core::App { phase: Phase::Running };
    assert!(user::job(&app).is_ok());
    app.phase = Phase::Stopping;                       // before-shutdown hooks running, traffic flowing
    assert!(user::job(&app).is_ok());
    let ws = fw_ws::WsServer { connections: 3, kept: Default::default() };
    let opened = core::run_drain(&mut app, &[&ws]);
    assert!(user::job(&app).is_err());
    app.phase = Phase::Destroying;
    let late = ws.late_disconnect(&app);
    println!("terminal executions opened during drain={opened}; opened after it with the kept token={late}");
}
