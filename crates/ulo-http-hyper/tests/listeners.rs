//! Both listeners serve the same endpoints the same way, since both adopt `ulo-net`'s sockets:
//! port 0 reports the port the OS chose, and a socket handed down through the socket-activation
//! protocol (`LISTEN_FDS`, `LISTEN_FDNAMES`, `LISTEN_PID`) is adopted by name and served. The
//! inherited socket goes to a child process, the test binary run again as the ignored test
//! `child_serves_an_inherited_socket`, which finds it as descriptor 3, `LISTEN_PID` set to its own
//! process ID by the shell it is started through.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ulo::Signal;
use ulo::app::{Bound, Connected};
use ulo::App;
use ulo_http_conformance::{Harness, OnSmol, OnTokio};
use ulo_http_hyper::ServerOn;
use ulo_listen_smol::SmolListener;
use ulo_net::Endpoint;

/// How long any one step may take.
const PATIENCE: Duration = Duration::from_secs(10);

/// The variable naming the listener the child serves on; unset, the child test does nothing.
const CHILD: &str = "ULO_LISTENER_CHILD";

/// The name the inherited socket carries in `LISTEN_FDNAMES`.
const NAME: &str = "web";

/// `GET /hit` on a blocking connection of its own, read to the end.
fn get_hit(addr: SocketAddr) -> String {
    let mut stream = TcpStream::connect(addr).expect("the server accepts");
    stream.set_read_timeout(Some(PATIENCE)).expect("a read timeout");
    stream.write_all(b"GET /hit HTTP/1.1\r\nHost: listeners\r\nConnection: close\r\n\r\n").expect("the request is written");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).expect("the answer is read to its end");
    String::from_utf8_lossy(&answer).into_owned()
}

/// The suite's app on `app`'s runtime, bound to the server on `endpoint` through listener
/// `listener`, and listening.
async fn listening(app: App<Connected>, listener: &str, endpoint: Endpoint) -> App<Bound> {
    let bound = match listener {
        "tokio" => app.bind(ulo_http_hyper::Server::new(endpoint)).listen().await,
        "smol" => app.bind(ServerOn::<SmolListener>::new(endpoint)).listen().await,
        other => panic!("no listener named {other}"),
    };
    bound.unwrap_or_else(|error| panic!("the app did not listen on the {listener} listener: {error}"))
}

/// Port 0 on `listener`: the bound address carries the OS's choice, and a request to it is served.
async fn port_zero(app: App<Connected>, listener: &str) {
    let app = listening(app, listener, Endpoint::parse("127.0.0.1:0").expect("an address")).await;
    let [bound] = app.addresses().try_into().expect("one bound address");
    assert_ne!(bound.addr.port(), 0, "the bound address on the {listener} listener carries the port the OS chose");
    let handle = app.handle();
    let runtime = std::sync::Arc::clone(handle.runtime().expect("the app's runtime"));
    let serving = runtime.spawn(Box::pin(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    }));
    let answer = get_hit(bound.addr);
    assert!(answer.starts_with("HTTP/1.1 200") && answer.ends_with("hit"), "port 0 on the {listener} listener answered {answer:?}");
    let _ = handle.close(Signal::new("listeners")).await;
    let _ = serving.await;
}

#[test]
fn port_0_on_the_tokio_listener_reports_and_serves_the_port_the_os_chose() {
    OnTokio::block_on(async { port_zero(ulo_http_conformance::app(OnTokio::runtime()).await, "tokio").await });
}

#[test]
fn port_0_on_the_smol_listener_reports_and_serves_the_port_the_os_chose() {
    OnSmol::block_on(async { port_zero(ulo_http_conformance::app(OnSmol::runtime()).await, "smol").await });
}

/// The test binary started again as `child_serves_an_inherited_socket` on `listener`, `socket` its
/// descriptor 3, through a shell that sets `LISTEN_PID` to the process ID the binary then runs as.
fn spawn_child(listener: &str, socket: &TcpListener) -> Child {
    let fd = socket.as_raw_fd();
    let mut command = Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("LISTEN_PID=$$ exec \"$0\" \"$@\"")
        .arg(std::env::current_exe().expect("the test binary's path"))
        .args(["--exact", "child_serves_an_inherited_socket", "--ignored", "--nocapture", "--test-threads=1"])
        .env(CHILD, listener)
        .env("LISTEN_FDS", "1")
        .env("LISTEN_FDNAMES", NAME)
        .env_remove("LISTEN_PID")
        .env_remove("ULO_DEV")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped());
    // SAFETY: the closure runs between fork and exec and calls only `dup2` and `fcntl`, both
    // async-signal-safe.
    unsafe {
        command.pre_exec(move || {
            // `dup2` onto the descriptor it already is changes nothing, `FD_CLOEXEC` included, so
            // that case clears the flag itself.
            let placed = if fd == 3 { libc::fcntl(3, libc::F_SETFD, 0) } else { libc::dup2(fd, 3) };
            if placed == -1 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
        });
    }
    command.spawn().expect("the child starts")
}

/// The child's stdout read until it reports it is serving, within [`PATIENCE`].
fn wait_ready(child: &mut Child) {
    let stdout = child.stdout.take().expect("the child's stdout");
    let (ready, readied) = mpsc::channel();
    std::thread::spawn(move || {
        // libtest writes the test's name before the first line the test prints, on the same line.
        for line in BufReader::new(stdout).lines() {
            match line {
                Ok(line) if line.contains("ulo-listeners: ready") => {
                    let _ = ready.send(Ok(()));
                }
                Ok(line) if line.contains("ulo-listeners: failed") => {
                    let _ = ready.send(Err(line));
                }
                Ok(_) => {}
                Err(_) => return,
            }
        }
    });
    match readied.recv_timeout(PATIENCE) {
        Ok(Ok(())) => {}
        Ok(Err(line)) => panic!("the child did not serve the inherited socket: {line}"),
        Err(_) => panic!("the child did not report serving within {PATIENCE:?}"),
    }
}

/// Closes the child's stdin, which stops its app, and waits for it to exit.
fn stop_child(mut child: Child) {
    drop(child.stdin.take());
    let deadline = Instant::now() + PATIENCE;
    loop {
        match child.try_wait().expect("the child's status") {
            Some(status) => {
                assert!(status.success(), "the child exited with {status}");
                return;
            }
            None if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            None => {
                let _ = child.kill();
                panic!("the child did not exit within {PATIENCE:?} of its stdin closing");
            }
        }
    }
}

/// Socket activation on `listener`: this process binds a port, hands the socket to the child as
/// descriptor 3, and a request to the port is answered by the child's app.
fn activated(listener: &str) {
    let socket = TcpListener::bind("127.0.0.1:0").expect("a port");
    let addr = socket.local_addr().expect("its address");
    let mut child = Killed(Some(spawn_child(listener, &socket)));
    wait_ready(child.0.as_mut().expect("the child"));
    let answer = get_hit(addr);
    assert!(
        answer.starts_with("HTTP/1.1 200") && answer.ends_with("hit"),
        "the inherited socket on the {listener} listener answered {answer:?}"
    );
    drop(socket);
    stop_child(child.0.take().expect("the child"));
}

/// The child, killed if the test fails before stopping it.
struct Killed(Option<Child>);

impl Drop for Killed {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn an_inherited_socket_is_served_on_the_tokio_listener() {
    activated("tokio");
}

#[test]
fn an_inherited_socket_is_served_on_the_smol_listener() {
    activated("smol");
}

/// The child: the suite's app on the listener `ULO_LISTENER_CHILD` names, its endpoint the
/// inherited socket named `web`, serving until its stdin closes.
#[test]
#[ignore = "the child process of the socket-activation tests; does nothing unless they start it"]
fn child_serves_an_inherited_socket() {
    let Ok(listener) = std::env::var(CHILD) else { return };
    let (closed, stdin_closed) = futures::channel::oneshot::channel::<()>();
    std::thread::spawn(move || {
        let mut sink = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut sink);
        let _ = closed.send(());
    });
    let serve = |app: App<Connected>| async move {
        let endpoint = Endpoint::inherited(NAME);
        let app = match listener.as_str() {
            "tokio" => app.bind(ulo_http_hyper::Server::new(endpoint)).listen().await,
            "smol" => app.bind(ServerOn::<SmolListener>::new(endpoint)).listen().await,
            other => panic!("no listener named {other}"),
        };
        let app = match app {
            Ok(app) => app,
            Err(error) => {
                println!("ulo-listeners: failed: {error}");
                return;
            }
        };
        println!("ulo-listeners: ready");
        let _ = app
            .serve(async move {
                let _ = stdin_closed.await;
                Signal::new("stdin closed")
            })
            .await;
    };
    match std::env::var(CHILD).as_deref() {
        Ok("tokio") => OnTokio::block_on(async { serve(ulo_http_conformance::app(OnTokio::runtime()).await).await }),
        _ => OnSmol::block_on(async { serve(ulo_http_conformance::app(OnSmol::runtime()).await).await }),
    }
}
