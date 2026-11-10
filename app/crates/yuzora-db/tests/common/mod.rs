//! What the integration tests share. Each test binary uses part of it.
//!
//! Which engines a run selects:
//!
//! `YUZORA_DATABASE_TEST_ENGINES` picks the engines whose fixtures a run uses;
//! a test of an engine left out ends early and passes. CI sets
//! `YUZORA_DATABASE_REQUIRE_ENGINES` to the engines it must exercise: one of
//! those missing from the selection fails the test instead of skipping it, so
//! a lost or misspelled variable can't turn the real-database tests green.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The PostgreSQL fixture of `tests/database/docker-compose.yml`.
const UPSTREAM: &str = "127.0.0.1:55432";

/// A cancel request: length 16, then the code 80877102.
const CANCEL_REQUEST: [u8; 8] = [0, 0, 0, 16, 4, 210, 22, 46];

#[derive(Clone, Copy)]
enum Hold {
    Nothing,
    /// A cancel request, told apart by its first bytes (so only on a
    /// connection that does not start with TLS).
    Cancels(Duration),
    /// Every connection made from now on, whatever it carries: each piece the
    /// client sends reaches the server this much later.
    Connections(Duration),
}

/// What the relay saw the server do, in the order it happened.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Heard {
    /// The server closed the connection a cancel request came on: it has
    /// acted on the request, as it does before it closes.
    StopDealtWith,
    /// A client sent ROLLBACK to the server.
    RollBack,
}

/// A TCP relay in front of the PostgreSQL fixture that holds back what it is
/// told to before the server hears of it. Cancelling is a message to the
/// server about whatever the connection runs when it gets there: this makes
/// that as late as the test wants, however fast the machine is.
pub struct LaggingPostgres {
    port: u16,
    hold: Arc<Mutex<Hold>>,
    heard: Arc<Mutex<Vec<Heard>>>,
}

impl LaggingPostgres {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hold = Arc::new(Mutex::new(Hold::Nothing));
        let heard = Arc::new(Mutex::new(Vec::new()));
        let (shared, log) = (hold.clone(), heard.clone());
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let hold = *shared.lock().unwrap_or_else(|e| e.into_inner());
                let log = log.clone();
                std::thread::spawn(move || relay(client, hold, log));
            }
        });
        LaggingPostgres { port, hold, heard }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn addr(&self) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], self.port))
    }

    fn set(&self, hold: Hold) {
        *self.hold.lock().unwrap_or_else(|e| e.into_inner()) = hold;
    }

    pub fn hold_cancels(&self, delay: Duration) {
        self.set(Hold::Cancels(delay));
    }

    pub fn hold_connections(&self, delay: Duration) {
        self.set(Hold::Connections(delay));
    }

    pub fn release(&self) {
        self.set(Hold::Nothing);
    }

    /// What the server has been seen to do since the relay started.
    pub fn heard(&self) -> Vec<Heard> {
        self.heard.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

fn relay(mut client: TcpStream, hold: Hold, heard: Arc<Mutex<Vec<Heard>>>) {
    let mut buf = [0u8; 4096];
    let Ok(n) = client.read(&mut buf) else { return };
    let cancel = buf[..n].starts_with(&CANCEL_REQUEST);
    let note = |what| heard.lock().unwrap_or_else(|e| e.into_inner()).push(what);
    let (lead, lag) = match hold {
        Hold::Cancels(delay) if buf[..n].starts_with(&CANCEL_REQUEST) => (delay, Duration::ZERO),
        Hold::Connections(delay) => (Duration::ZERO, delay),
        _ => (Duration::ZERO, Duration::ZERO),
    };
    std::thread::sleep(lead);
    let Ok(mut server) = TcpStream::connect(UPSTREAM) else { return };
    let (mut from_client, mut to_server) = (client.try_clone().unwrap(), server.try_clone().unwrap());
    let rollbacks = heard.clone();
    std::thread::spawn(move || {
        let mut piece = buf[..n].to_vec();
        loop {
            std::thread::sleep(lag);
            if piece.windows(8).any(|w| w == b"ROLLBACK") {
                rollbacks.lock().unwrap_or_else(|e| e.into_inner()).push(Heard::RollBack);
            }
            if to_server.write_all(&piece).is_err() {
                break;
            }
            match from_client.read(&mut buf) {
                Ok(n) if n > 0 => piece = buf[..n].to_vec(),
                _ => break,
            }
        }
        let _ = to_server.shutdown(Shutdown::Write);
    });
    let _ = std::io::copy(&mut server, &mut client);
    if cancel {
        note(Heard::StopDealtWith);
    }
    let _ = client.shutdown(Shutdown::Both);
}

/// Something that listens where a PostgreSQL server might, and is not one: it
/// takes every connection and closes it at once, which is also what a server
/// does with a cancel request for a backend it does not have.
pub struct ClosedDoor {
    addr: SocketAddr,
    taken: Arc<AtomicUsize>,
}

impl ClosedDoor {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let taken = Arc::new(AtomicUsize::new(0));
        let count = taken.clone();
        std::thread::spawn(move || {
            for connection in listener.incoming().flatten() {
                count.fetch_add(1, Ordering::SeqCst);
                drop(connection);
            }
        });
        ClosedDoor { addr, taken }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Connections taken so far.
    pub fn taken(&self) -> usize {
        self.taken.load(Ordering::SeqCst)
    }
}

fn listed(var: &str, engine: &str) -> bool {
    std::env::var(var).is_ok_and(|v| v.split(',').any(|e| e.trim().eq_ignore_ascii_case(engine)))
}

pub fn enabled(engine: &str) -> bool {
    let selected = listed("YUZORA_DATABASE_TEST_ENGINES", engine);
    assert!(
        selected || !listed("YUZORA_DATABASE_REQUIRE_ENGINES", engine),
        "{engine} is in YUZORA_DATABASE_REQUIRE_ENGINES but not in YUZORA_DATABASE_TEST_ENGINES"
    );
    selected
}
