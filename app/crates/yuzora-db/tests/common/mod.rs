//! What the integration tests share. Each test binary uses part of it.
//!
//! A delay line between the app and a database server ([`DelayLine`]) is shared
//! by the measurements (`perf_open_table.rs`) and the tests that count round
//! trips (`open_rounds.rs`). Every chunk is held for the one-way delay before
//! it passes on, so pipelined requests stay pipelined and a large result is not
//! slowed by it. There is no bandwidth limit, no loss and no jitter. The line
//! logs the direction of every chunk of every connection, so a round trip is
//! counted, not guessed: a flight of server data that follows a flight of
//! client data.
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
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// A cancel request, until [`LaggingPostgres::release`]: however long the
    /// test takes to look, the server has not heard it.
    CancelsUntilReleased,
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
    arrived: Arc<AtomicUsize>,
}

impl LaggingPostgres {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hold = Arc::new(Mutex::new(Hold::Nothing));
        let heard = Arc::new(Mutex::new(Vec::new()));
        let arrived = Arc::new(AtomicUsize::new(0));
        let (shared, log, count) = (hold.clone(), heard.clone(), arrived.clone());
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let hold = *shared.lock().unwrap_or_else(|e| e.into_inner());
                let (log, gate, count) = (log.clone(), shared.clone(), count.clone());
                std::thread::spawn(move || relay(client, hold, gate, log, count));
            }
        });
        LaggingPostgres { port, hold, heard, arrived }
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

    pub fn hold_cancels_until_released(&self) {
        self.set(Hold::CancelsUntilReleased);
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

    /// [`heard`](Self::heard) once it has `count` things in it. What the client
    /// sends after its own call returned (a dropped transaction's ROLLBACK) is
    /// written by the connection's task and read by a relay thread, so it is
    /// there when it gets there; ten seconds is the most this waits for it.
    pub fn heard_until(&self, count: usize) -> Vec<Heard> {
        let started = std::time::Instant::now();
        loop {
            let heard = self.heard();
            if heard.len() >= count || started.elapsed() > Duration::from_secs(10) {
                return heard;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Cancel requests that have reached the relay, held back or not: the
    /// client has sent them, the server has not necessarily heard them.
    pub fn stops_arrived(&self) -> usize {
        self.arrived.load(Ordering::SeqCst)
    }
}

fn relay(mut client: TcpStream, hold: Hold, gate: Arc<Mutex<Hold>>, heard: Arc<Mutex<Vec<Heard>>>, arrived: Arc<AtomicUsize>) {
    let mut buf = [0u8; 4096];
    let Ok(n) = client.read(&mut buf) else { return };
    let cancel = buf[..n].starts_with(&CANCEL_REQUEST);
    if cancel {
        arrived.fetch_add(1, Ordering::SeqCst);
    }
    let note = |what| heard.lock().unwrap_or_else(|e| e.into_inner()).push(what);
    let (lead, lag) = match hold {
        Hold::Cancels(delay) if buf[..n].starts_with(&CANCEL_REQUEST) => (delay, Duration::ZERO),
        Hold::Connections(delay) => (Duration::ZERO, delay),
        _ => (Duration::ZERO, Duration::ZERO),
    };
    std::thread::sleep(lead);
    let gated =
        |gate: &Mutex<Hold>| matches!(*gate.lock().unwrap_or_else(|e| e.into_inner()), Hold::CancelsUntilReleased);
    while cancel && matches!(hold, Hold::CancelsUntilReleased) && gated(&gate) {
        std::thread::sleep(Duration::from_millis(2));
    }
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

// ---------------------------------------------------------------- the delay line

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dir {
    Up,
    Down,
}

#[derive(Default)]
pub struct ConnLog {
    accepted: Option<Instant>,
    events: Vec<(Instant, Dir, usize)>,
}

/// What one connection through the line did during a sample.
#[derive(Debug, Clone)]
pub struct ConnStat {
    /// The connection was already open when the sample began.
    pub pooled: bool,
    /// When its first chunk went by in the sample, from the start of the sample.
    pub first_ms: f64,
    /// When its last chunk went by, from the start of the sample.
    pub last_ms: f64,
    /// Flights of server data that follow a flight of client data: round trips.
    pub exchanges: usize,
    pub up: usize,
    pub down: usize,
}

pub struct DelayLine {
    pub addr: SocketAddr,
    log: Arc<Mutex<Vec<ConnLog>>>,
}

impl DelayLine {
    /// Passes `listener`'s connections to `target`, every chunk held for `one_way` each way.
    pub fn start(target: SocketAddr, one_way: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let log: Arc<Mutex<Vec<ConnLog>>> = Arc::default();
        let shared = log.clone();
        std::thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let Ok(server) = TcpStream::connect(target) else { continue };
                let _ = client.set_nodelay(true);
                let _ = server.set_nodelay(true);
                let index = {
                    let mut all = shared.lock().unwrap();
                    all.push(ConnLog { accepted: Some(Instant::now()), events: Vec::new() });
                    all.len() - 1
                };
                let (c2, s2) = (client.try_clone().unwrap(), server.try_clone().unwrap());
                let (up_log, down_log) = (shared.clone(), shared.clone());
                std::thread::spawn(move || pump(client, s2, Dir::Up, one_way, up_log, index));
                std::thread::spawn(move || pump(server, c2, Dir::Down, one_way, down_log, index));
            }
        });
        DelayLine { addr, log }
    }

    /// The connections that carried anything since `since`, in the order they first did.
    pub fn window(&self, since: Instant) -> Vec<ConnStat> {
        self.window_between(since, None)
    }

    /// [`window`](Self::window), looking only at what went by up to `until`.
    pub fn window_between(&self, since: Instant, until: Option<Instant>) -> Vec<ConnStat> {
        let all = self.log.lock().unwrap();
        let mut stats: Vec<ConnStat> = all
            .iter()
            .filter_map(|c| {
                let events: Vec<_> =
                    c.events.iter().filter(|e| e.0 >= since && until.is_none_or(|u| e.0 <= u)).collect();
                let (first, last) = (events.first()?, events.last()?);
                let (mut exchanges, mut previous, mut up, mut down) = (0, None, 0, 0);
                for (_, dir, n) in &events {
                    if *dir == Dir::Down && previous == Some(Dir::Up) {
                        exchanges += 1;
                    }
                    previous = Some(*dir);
                    match dir {
                        Dir::Up => up += n,
                        Dir::Down => down += n,
                    }
                }
                Some(ConnStat {
                    pooled: c.accepted.is_some_and(|a| a < since),
                    first_ms: ms(first.0 - since),
                    last_ms: ms(last.0 - since),
                    exchanges,
                    up,
                    down,
                })
            })
            .collect();
        stats.sort_by(|a, b| a.first_ms.partial_cmp(&b.first_ms).unwrap());
        stats
    }
}

fn pump(mut from: TcpStream, to: TcpStream, dir: Dir, delay: Duration, log: Arc<Mutex<Vec<ConnLog>>>, index: usize) {
    let (tx, rx) = mpsc::channel::<(Instant, Option<Vec<u8>>)>();
    let writer = std::thread::spawn(move || {
        let mut to = to;
        while let Ok((due, data)) = rx.recv() {
            // The OS timer overshoots by milliseconds: sleep most of it, spin the rest.
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                if wait > Duration::from_millis(3) {
                    std::thread::sleep(wait - Duration::from_millis(3));
                }
                while Instant::now() < due {
                    std::hint::spin_loop();
                }
            }
            match data {
                Some(data) => {
                    if to.write_all(&data).is_err() {
                        break;
                    }
                }
                None => {
                    let _ = to.shutdown(Shutdown::Write);
                    break;
                }
            }
        }
    });
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        match from.read(&mut buf) {
            Ok(0) | Err(_) => {
                let _ = tx.send((Instant::now() + delay, None));
                break;
            }
            Ok(n) => {
                let now = Instant::now();
                log.lock().unwrap()[index].events.push((now, dir, n));
                if tx.send((now + delay, Some(buf[..n].to_vec()))).is_err() {
                    break;
                }
            }
        }
    }
    let _ = writer.join();
}
