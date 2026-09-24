//! PTY transport for the official HERDR client, not an alternative shell runtime.
//! Only an explicitly selected, running compatible named Session can be opened.
use super::*;
use base64::Engine;
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};

fn terminate_native_client(child: &mut dyn portable_pty::Child) -> Result<(), String> {
    if child.try_wait().map_err(|e| e.to_string())?.is_some() {
        return Ok(());
    }
    child.kill().map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        // try_wait reaps the owned process too. Avoid an unbounded wait in the
        // host stream Drop path: a stuck wait would prevent helper shutdown.
        if child.try_wait().map_err(|e| e.to_string())?.is_some() {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err("native-client-exit-timeout".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) struct NativeHerdrClient {
    master: Mutex<Box<dyn MasterPty + Send>>,
    input: Mutex<Option<Box<dyn Write + Send>>>,
    child: Mutex<Box<dyn portable_pty::Child + Send + Sync>>,
    reader: Mutex<Option<JoinHandle<()>>>,
    closed: AtomicBool,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HerdrClientSize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}
impl HerdrClientSize {
    fn pty_size(&self) -> Result<PtySize, String> {
        if self.cols == 0
            || self.rows == 0
            || self.cols > 1000
            || self.rows > 1000
            || self.cell_width > 128
            || self.cell_height > 256
        {
            return Err("invalid-herdr-client-size".into());
        }
        Ok(PtySize {
            cols: self.cols,
            rows: self.rows,
            pixel_width: self.cols.saturating_mul(self.cell_width),
            pixel_height: self.rows.saturating_mul(self.cell_height),
        })
    }
}

const MAX_NATIVE_CLIENTS: usize = 16;

impl HerdrManager {
    pub fn open_native_client(
        self: &Arc<Self>,
        session_name: &str,
        size: HerdrClientSize,
        on_event: OnTerminalEvent,
    ) -> Result<HerdrTerminalOpenResult, String> {
        if self.remote.is_some() {
            return Err("native-client-must-run-on-owning-host".into());
        }
        if self.native_clients.lock().unwrap().len() >= MAX_NATIVE_CLIENTS {
            return Err("too-many-native-herdr-clients".into());
        }
        if !self.session_is_compatible(session_name) {
            return Err("native-client-requires-compatible-session".into());
        }
        let (session, _) = self.require_running_session_socket(Some(session_name))?;
        let binary = self.resolve_binary().ok_or("herdr-unavailable")?;
        let pair = native_pty_system()
            .openpty(size.pty_size()?)
            .map_err(|e| e.to_string())?;
        let mut output = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let input = pair.master.take_writer().map_err(|e| e.to_string())?;
        let mut command = CommandBuilder::new(binary);
        // The explicit client subcommand never auto-starts a server if the
        // Session stops between the compatibility check and process creation.
        command.args(["--session", &session.name, "client"]);
        command.env("HERDR_SESSION", &session.name);
        command.env_remove("HERDR_ENV");
        command.env_remove("HERDR_SOCKET_PATH");
        command.env_remove("HERDR_CLIENT_SOCKET_PATH");
        command.env_remove("HERDR_PANE_ID");
        command.env_remove("HERDR_TAB_ID");
        command.env_remove("HERDR_WORKSPACE_ID");
        // This renderer supports inline Kitty data, not local file/shared-memory
        // transports. A distinct terminal identity keeps HERDR on that path.
        // xterm-kitty itself opts into HERDR's local-file optimization even
        // with a different TERM_PROGRAM. Generic xterm uses inline blobs.
        command.env("TERM", "xterm-256color");
        command.env("TERM_PROGRAM", "yuzora");
        command.env_remove("KITTY_WINDOW_ID");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| e.to_string())?;
        drop(pair.slave);
        let client = Arc::new(NativeHerdrClient {
            master: Mutex::new(pair.master),
            input: Mutex::new(Some(input)),
            child: Mutex::new(child),
            reader: Mutex::new(None),
            closed: AtomicBool::new(false),
        });
        let id = format!(
            "herdr-client-{}",
            NEXT_SESSION_ID.fetch_add(1, Ordering::Relaxed)
        );
        let reader_client = client.clone();
        let manager = Arc::downgrade(self);
        let reader_id = id.clone();
        let cols = size.cols;
        let rows = size.rows;
        {
            // Re-check under the insert lock: concurrent opens pass the early
            // check before either process is registered.
            let mut clients = self.native_clients.lock().unwrap();
            if clients.len() >= MAX_NATIVE_CLIENTS {
                drop(clients);
                let _ = terminate_native_client(client.child.lock().unwrap().as_mut());
                return Err("too-many-native-herdr-clients".into());
            }
            clients.insert(id.clone(), client.clone());
        }
        let reader = std::thread::Builder::new().name(id.clone()).spawn(move || {
            let mut bytes = [0u8; 16 * 1024];
            let mut seq = 0u64;
            while !reader_client.closed.load(Ordering::Acquire) {
                let len = match output.read(&mut bytes) {
                    Ok(0) | Err(_) => break,
                    Ok(len) => len,
                };
                seq += 1;
                if on_event(HerdrTerminalEvent::Frame {
                    session_id: reader_id.clone(),
                    seq,
                    full: false,
                    encoding: "ansi".into(),
                    width: u32::from(cols),
                    height: u32::from(rows),
                    bytes_base64: base64::engine::general_purpose::STANDARD.encode(&bytes[..len]),
                })
                .is_err()
                {
                    break;
                }
            }
            if !reader_client.closed.swap(true, Ordering::AcqRel) {
                reader_client.input.lock().unwrap().take();
                let mut child = reader_client.child.lock().unwrap();
                let _ = terminate_native_client(child.as_mut());
                drop(child);
                if let Some(manager) = manager.upgrade() {
                    manager.native_clients.lock().unwrap().remove(&reader_id);
                }
                let _ = on_event(HerdrTerminalEvent::Closed {
                    session_id: reader_id,
                    reason: None,
                });
            }
        });
        match reader {
            Ok(reader) => *client.reader.lock().unwrap() = Some(reader),
            Err(error) => {
                self.native_clients.lock().unwrap().remove(&id);
                let _ = terminate_native_client(client.child.lock().unwrap().as_mut());
                return Err(error.to_string());
            }
        }
        Ok(HerdrTerminalOpenResult {
            session_id: id,
            target: session.name,
            mode: HerdrTerminalMode::Control,
            role: HerdrTerminalRole::Controller,
            cols,
            rows,
            takeover: false,
        })
    }

    pub(super) fn native_client_input(
        &self,
        id: &str,
        text: Option<String>,
        bytes_base64: Option<String>,
    ) -> Result<(), String> {
        let bytes = match (text, bytes_base64) {
            (Some(text), None) => text.into_bytes(),
            (None, Some(encoded)) => base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .map_err(|_| "invalid-client-input")?,
            _ => return Err("exactly-one-input-format-required".into()),
        };
        if bytes.len() > 512 * 1024 {
            return Err("client-input-too-large".into());
        }
        let client = self
            .native_clients
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("herdr-client-closed")?;
        let mut input = client.input.lock().unwrap();
        let input = input.as_mut().ok_or("herdr-client-closed")?;
        input
            .write_all(&bytes)
            .and_then(|()| input.flush())
            .map_err(|e| e.to_string())
    }

    pub(super) fn native_client_resize(
        &self,
        id: &str,
        cols: u16,
        rows: u16,
    ) -> Result<(), String> {
        let client = self
            .native_clients
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or("herdr-client-closed")?;
        let master = client.master.lock().unwrap();
        let previous = master.get_size().map_err(|e| e.to_string())?;
        let size = HerdrClientSize {
            cols,
            rows,
            cell_width: previous.pixel_width.checked_div(previous.cols).unwrap_or(0),
            cell_height: previous
                .pixel_height
                .checked_div(previous.rows)
                .unwrap_or(0),
        };
        master.resize(size.pty_size()?).map_err(|e| e.to_string())
    }

    pub(super) fn release_native_client(&self, id: &str) -> Result<(), String> {
        let Some(client) = self.native_clients.lock().unwrap().remove(id) else {
            return Ok(());
        };
        client.closed.store(true, Ordering::Release);
        client.input.lock().unwrap().take();
        // Kill only the official UI client. Never attach a process-tree guard:
        // HERDR's persistent server and its pane processes belong to the user.
        let mut child = client.child.lock().unwrap();
        terminate_native_client(child.as_mut())?;
        drop(child);
        if let Some(reader) = client.reader.lock().unwrap().take() {
            let _ = reader.join();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn native_client_termination_reaps_a_pty_child_that_ignores_hangup() {
        let pair = native_pty_system().openpty(PtySize::default()).unwrap();
        let mut output = pair.master.try_clone_reader().unwrap();
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "trap '' HUP; printf R; while :; do :; done"]);
        let mut child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut ready = [0u8; 1];
        output.read_exact(&mut ready).unwrap();
        assert_eq!(ready, *b"R");
        let started = std::time::Instant::now();
        terminate_native_client(child.as_mut()).unwrap();
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(child.try_wait().unwrap().is_some());
        terminate_native_client(child.as_mut()).unwrap();
    }

    #[test]
    fn native_client_geometry_is_bounded_and_carries_pixel_cells() {
        let value = HerdrClientSize {
            cols: 120,
            rows: 40,
            cell_width: 8,
            cell_height: 16,
        }
        .pty_size()
        .unwrap();
        assert_eq!((value.pixel_width, value.pixel_height), (960, 640));
        assert!(HerdrClientSize {
            cols: 0,
            rows: 10,
            cell_width: 8,
            cell_height: 16
        }
        .pty_size()
        .is_err());
        assert!(HerdrClientSize {
            cols: 1001,
            rows: 10,
            cell_width: 8,
            cell_height: 16
        }
        .pty_size()
        .is_err());
    }
}
