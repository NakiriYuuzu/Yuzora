//! Cross-platform Herdr public NDJSON local-socket transport.
//!
//! Matches official Herdr v0.8.0 `src/ipc.rs`: Unix domain sockets via
//! `GenericFilePath`, Windows named pipes via the `GenericNamespaced`
//! `\\.\pipe\` mapping. The advertised Herdr `socket_path` marker is never
//! rewritten in DTOs or cache keys.

use std::io::{self, Read, Write};
#[cfg(any(unix, test))]
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

use interprocess::local_socket::traits::Stream as _;
#[cfg(unix)]
use interprocess::local_socket::ConnectOptions;
use interprocess::ConnectWaitMode;

use crate::herdr_limits::{BoundedNdjsonReadError, HerdrProtocolError};

pub(crate) type LocalStream = interprocess::local_socket::Stream;
#[cfg(test)]
pub(crate) type LocalListener = interprocess::local_socket::Listener;

const POLL_INTERVAL: Duration = Duration::from_millis(100);
const READ_CHUNK_BYTES: usize = 8 * 1024;

pub(crate) enum LocalStreamRead {
    Data(usize),
    Pending,
    Closed,
}

pub(crate) fn remaining_timeout(deadline: Instant) -> io::Result<Duration> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "local stream timed out",
        ));
    }
    Ok(remaining)
}

pub(crate) fn connect_local_stream(
    socket_path: &str,
    deadline: Instant,
) -> io::Result<LocalStream> {
    if socket_path.trim().is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "empty socket path",
        ));
    }
    let timeout = remaining_timeout(deadline)?;
    let stream = connect_named(socket_path, timeout)?;
    prepare_local_stream(&stream)?;
    Ok(stream)
}

fn connect_named(socket_path: &str, timeout: Duration) -> io::Result<LocalStream> {
    #[cfg(unix)]
    {
        use interprocess::local_socket::{prelude::*, GenericFilePath};

        let name = Path::new(socket_path).to_fs_name::<GenericFilePath>()?;
        ConnectOptions::new()
            .name(name)
            .wait_mode(ConnectWaitMode::Timeout(timeout))
            .nonblocking_stream(true)
            .connect_sync()
    }

    #[cfg(windows)]
    {
        use interprocess::os::windows::named_pipe::{
            local_socket::Stream as WindowsLocalStream, pipe_mode, DuplexPipeStream,
        };

        // GenericNamespaced prepends \\.\pipe\ to the advertised marker. The
        // local_socket ConnectOptions adapter does not forward ConnectWaitMode,
        // so the bounded constructor must be used directly.
        let pipe_path = windows_generic_namespaced_pipe_path(socket_path);
        let duplex = DuplexPipeStream::<pipe_mode::Bytes>::connect_by_path_with_wait_mode(
            pipe_path.as_str(),
            ConnectWaitMode::Timeout(timeout),
        )?;
        Ok(LocalStream::from(WindowsLocalStream::from(duplex)))
    }
}

#[cfg(windows)]
fn windows_generic_namespaced_pipe_path(marker: &str) -> String {
    format!(r"\\.\pipe\{marker}")
}

fn prepare_local_stream(stream: &LocalStream) -> io::Result<()> {
    stream.set_nonblocking(true)
}

#[cfg(test)]
pub(crate) fn bind_local_listener(path: &Path) -> io::Result<LocalListener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    #[cfg(unix)]
    {
        use interprocess::local_socket::{prelude::*, GenericFilePath, ListenerOptions};

        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        let name = path.to_fs_name::<GenericFilePath>()?;
        ListenerOptions::new()
            .name(name)
            .reclaim_name(false)
            .create_sync()
    }

    #[cfg(windows)]
    {
        use interprocess::local_socket::{prelude::*, GenericNamespaced, ListenerOptions};

        let name = path.to_string_lossy().to_string();
        let name = name.to_ns_name::<GenericNamespaced>()?;
        let listener = ListenerOptions::new()
            .name(name)
            .reclaim_name(false)
            .create_sync()?;
        std::fs::write(path, windows_socket_marker())?;
        Ok(listener)
    }
}

#[cfg(all(test, windows))]
fn windows_socket_marker() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}:{now}", std::process::id())
}

pub(crate) fn poll_local_stream_read(
    stream: &mut LocalStream,
    buffer: &mut [u8],
) -> io::Result<LocalStreamRead> {
    #[cfg(unix)]
    {
        match stream.read(buffer) {
            Ok(0) => Ok(LocalStreamRead::Closed),
            Ok(read) => Ok(LocalStreamRead::Data(read)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(LocalStreamRead::Pending),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                Ok(LocalStreamRead::Pending)
            }
            Err(error) if is_connection_closed_error(&error) => Ok(LocalStreamRead::Closed),
            Err(error) => Err(error),
        }
    }

    #[cfg(windows)]
    {
        match windows_named_pipe_available(stream)? {
            None => Ok(LocalStreamRead::Closed),
            Some(0) => Ok(LocalStreamRead::Pending),
            Some(available) => {
                let readable = usize::try_from(available)
                    .unwrap_or(usize::MAX)
                    .min(buffer.len());
                match stream.read(&mut buffer[..readable]) {
                    Ok(0) => Ok(LocalStreamRead::Closed),
                    Ok(read) => Ok(LocalStreamRead::Data(read)),
                    Err(error) if is_connection_closed_error(&error) => Ok(LocalStreamRead::Closed),
                    Err(error) => Err(error),
                }
            }
        }
    }
}

#[cfg(windows)]
fn windows_named_pipe_available(stream: &mut LocalStream) -> io::Result<Option<u32>> {
    use std::os::windows::io::{AsHandle, AsRawHandle};

    let LocalStream::NamedPipe(pipe) = stream;
    let mut available = 0u32;
    let ok = unsafe {
        windows_sys::Win32::System::Pipes::PeekNamedPipe(
            pipe.as_handle().as_raw_handle() as _,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &mut available,
            std::ptr::null_mut(),
        )
    };
    if ok != 0 {
        return Ok(Some(available));
    }

    let err = io::Error::last_os_error();
    if is_connection_closed_error(&err) || windows_named_pipe_closed_error(&err) {
        return Ok(None);
    }
    Err(err)
}

fn is_connection_closed_error(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::NotConnected
            | io::ErrorKind::UnexpectedEof
            | io::ErrorKind::WriteZero
    )
}

#[cfg(windows)]
fn windows_named_pipe_closed_error(err: &io::Error) -> bool {
    matches!(err.raw_os_error(), Some(6 | 109 | 232 | 233))
}

pub(crate) fn read_local_ndjson_line(
    stream: &mut LocalStream,
    pending: &mut Vec<u8>,
    deadline: Option<Instant>,
    max_bytes: usize,
) -> Result<Option<String>, BoundedNdjsonReadError> {
    let mut buffer = [0u8; READ_CHUNK_BYTES];
    loop {
        if let Some(newline) = pending.iter().position(|byte| *byte == b'\n') {
            let remainder = pending.split_off(newline + 1);
            let line = std::mem::replace(pending, remainder);
            return decode_completed_line(&line, max_bytes).map(Some);
        }
        // Allow one extra content byte so a terminated MAX+1 line is
        // LineTooLarge instead of UnterminatedOverLimit.
        if pending.len() > max_bytes.saturating_add(1) {
            pending.clear();
            return Err(BoundedNdjsonReadError::Protocol(
                HerdrProtocolError::UnterminatedOverLimit,
            ));
        }
        if deadline_exceeded(deadline) {
            return Err(BoundedNdjsonReadError::Protocol(
                HerdrProtocolError::TimedOut,
            ));
        }
        match poll_local_stream_read(stream, &mut buffer) {
            Ok(LocalStreamRead::Data(read)) => pending.extend_from_slice(&buffer[..read]),
            Ok(LocalStreamRead::Pending) => {
                sleep_until(deadline);
            }
            Ok(LocalStreamRead::Closed) => {
                if pending.is_empty() {
                    return Ok(None);
                }
                let line = std::mem::take(pending);
                return decode_completed_line(&line, max_bytes).map(Some);
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(BoundedNdjsonReadError::Io(error)),
        }
    }
}

fn decode_completed_line(bytes: &[u8], max_bytes: usize) -> Result<String, BoundedNdjsonReadError> {
    let terminated = bytes.last() == Some(&b'\n');
    let content_len = if terminated {
        bytes.len().saturating_sub(1)
    } else {
        bytes.len()
    };
    if content_len > max_bytes {
        return Err(BoundedNdjsonReadError::Protocol(if terminated {
            HerdrProtocolError::LineTooLarge
        } else {
            HerdrProtocolError::UnterminatedOverLimit
        }));
    }
    String::from_utf8(bytes.to_vec())
        .map_err(|_| BoundedNdjsonReadError::Protocol(HerdrProtocolError::InvalidUtf8))
}

pub(crate) fn write_local_all_until(
    stream: &mut LocalStream,
    bytes: &[u8],
    deadline: Instant,
) -> io::Result<()> {
    prepare_local_stream(stream)?;
    let mut offset = 0usize;
    while offset < bytes.len() {
        remaining_timeout(deadline)?;
        match stream.write(&bytes[offset..]) {
            // A nonblocking Windows named pipe reports a full output buffer as
            // a successful zero-byte write. Treat that as backpressure and let
            // the same deadline used for WouldBlock terminate the request.
            Ok(0) if cfg!(windows) => sleep_until(Some(deadline)),
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "local stream write returned zero bytes",
                ));
            }
            Ok(written) => offset += written,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                sleep_until(Some(deadline));
            }
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn deadline_exceeded(deadline: Option<Instant>) -> bool {
    deadline.is_some_and(|value| Instant::now() >= value)
}

fn sleep_until(deadline: Option<Instant>) {
    let remaining = deadline
        .map(|value| value.saturating_duration_since(Instant::now()))
        .unwrap_or(POLL_INTERVAL);
    if remaining.is_zero() {
        return;
    }
    thread::sleep(remaining.min(POLL_INTERVAL));
}

#[cfg(test)]
pub(crate) fn unique_local_socket_path(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "yuzora-herdr-{label}-{}-{}.sock",
        std::process::id(),
        Instant::now().elapsed().as_nanos()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::herdr_limits::MAX_NDJSON_LINE_BYTES;
    use interprocess::local_socket::traits::Listener as _;

    // Framing tests transfer several 1 MiB lines. This deliberately separates
    // their functional I/O allowance from the short deadline assertions below.
    const FUNCTIONAL_FRAME_IO_DEADLINE: Duration = Duration::from_secs(15);

    fn local_pair(label: &str) -> (LocalListener, PathBuf) {
        let path = unique_local_socket_path(label);
        let listener = bind_local_listener(&path).expect("bind local listener");
        (listener, path)
    }

    fn connect_framing_test_client(advertised: &str) -> LocalStream {
        connect_local_stream(advertised, Instant::now() + FUNCTIONAL_FRAME_IO_DEADLINE)
            .expect("connect framing test client")
    }

    fn read_framing_test_line(
        client: &mut LocalStream,
        pending: &mut Vec<u8>,
    ) -> Result<Option<String>, BoundedNdjsonReadError> {
        read_local_ndjson_line(
            client,
            pending,
            Some(Instant::now() + FUNCTIONAL_FRAME_IO_DEADLINE),
            MAX_NDJSON_LINE_BYTES,
        )
    }

    #[test]
    fn ping_roundtrip_preserves_authoritative_path_string() {
        let (listener, path) = local_pair("ping");
        let advertised = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            let mut stream = listener.accept().expect("accept");
            prepare_local_stream(&stream).unwrap();
            let mut pending = Vec::new();
            let request = read_local_ndjson_line(
                &mut stream,
                &mut pending,
                Some(Instant::now() + Duration::from_secs(2)),
                MAX_NDJSON_LINE_BYTES,
            )
            .unwrap()
            .expect("request line");
            write_local_all_until(
                &mut stream,
                b"{\"result\":{\"type\":\"pong\",\"version\":\"0.8.0\",\"protocol\":19}}\n",
                Instant::now() + Duration::from_secs(2),
            )
            .unwrap();
            request
        });

        let mut client = connect_local_stream(&advertised, Instant::now() + Duration::from_secs(2))
            .expect("connect");
        write_local_all_until(
            &mut client,
            b"{\"id\":\"t\",\"method\":\"ping\",\"params\":{}}\n",
            Instant::now() + Duration::from_secs(2),
        )
        .unwrap();
        let mut pending = Vec::new();
        let response = read_local_ndjson_line(
            &mut client,
            &mut pending,
            Some(Instant::now() + Duration::from_secs(2)),
            MAX_NDJSON_LINE_BYTES,
        )
        .unwrap()
        .expect("response");
        assert!(response.contains("\"protocol\":19"), "{response}");
        let request = server.join().unwrap();
        assert!(request.contains("\"method\":\"ping\""), "{request}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reader_retains_partial_bytes_across_pending_polls() {
        let (listener, path) = local_pair("fragment");
        let advertised = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            let mut stream = listener.accept().expect("accept");
            thread::sleep(Duration::from_millis(40));
            stream.write_all(b"{\"result\":").unwrap();
            thread::sleep(Duration::from_millis(120));
            stream.write_all(b"{\"ok\":true}}\n").unwrap();
        });

        let mut client = connect_local_stream(&advertised, Instant::now() + Duration::from_secs(2))
            .expect("connect");
        let mut pending = Vec::new();
        let line = read_local_ndjson_line(
            &mut client,
            &mut pending,
            Some(Instant::now() + Duration::from_secs(2)),
            MAX_NDJSON_LINE_BYTES,
        )
        .unwrap()
        .expect("assembled line");
        assert!(line.contains("\"ok\":true"), "{line}");
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reader_preserves_following_line_from_one_chunk() {
        let (listener, path) = local_pair("two-lines");
        let advertised = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            let mut stream = listener.accept().expect("accept");
            stream.write_all(b"{\"line\":1}\n{\"line\":2}\n").unwrap();
        });

        let mut client = connect_local_stream(&advertised, Instant::now() + Duration::from_secs(2))
            .expect("connect");
        let mut pending = Vec::new();
        let first = read_local_ndjson_line(
            &mut client,
            &mut pending,
            Some(Instant::now() + Duration::from_secs(2)),
            MAX_NDJSON_LINE_BYTES,
        )
        .unwrap()
        .expect("first line");
        let second = read_local_ndjson_line(
            &mut client,
            &mut pending,
            Some(Instant::now() + Duration::from_secs(2)),
            MAX_NDJSON_LINE_BYTES,
        )
        .unwrap()
        .expect("second line");
        assert!(first.contains("\"line\":1"), "{first}");
        assert!(second.contains("\"line\":2"), "{second}");
        assert!(pending.is_empty());
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reader_keeps_partial_bytes_across_short_deadlines() {
        let prefix = b"{\"ok\":";
        let (listener, path) = local_pair("resume");
        let advertised = path.to_string_lossy().into_owned();
        let (prefix_ready_tx, prefix_ready_rx) = std::sync::mpsc::sync_channel(0);
        let (resume_tx, resume_rx) = std::sync::mpsc::sync_channel(0);
        let server = thread::spawn(move || {
            let mut stream = listener.accept().expect("accept");
            stream.write_all(prefix).unwrap();
            prefix_ready_tx.send(()).expect("report prefix");
            resume_rx.recv().expect("release remainder");
            stream.write_all(b"true}\n").unwrap();
        });

        let mut client = connect_local_stream(&advertised, Instant::now() + Duration::from_secs(2))
            .expect("connect");
        prefix_ready_rx.recv().expect("prefix ready");
        let mut pending = Vec::new();
        let prefix_deadline = Instant::now() + Duration::from_secs(2);
        let mut buffer = [0u8; READ_CHUNK_BYTES];
        while pending.len() < prefix.len() {
            assert!(
                Instant::now() < prefix_deadline,
                "prefix should become readable"
            );
            match poll_local_stream_read(&mut client, &mut buffer).expect("poll prefix") {
                LocalStreamRead::Data(read) => pending.extend_from_slice(&buffer[..read]),
                LocalStreamRead::Pending => sleep_until(Some(prefix_deadline)),
                LocalStreamRead::Closed => panic!("stream closed before prefix"),
            }
        }
        assert_eq!(pending.as_slice(), prefix);
        let first = read_local_ndjson_line(
            &mut client,
            &mut pending,
            Some(Instant::now() + Duration::from_millis(120)),
            MAX_NDJSON_LINE_BYTES,
        )
        .expect_err("first poll should time out with a prefix");
        assert!(matches!(
            first,
            BoundedNdjsonReadError::Protocol(HerdrProtocolError::TimedOut)
        ));
        assert!(!pending.is_empty());
        resume_tx.send(()).expect("resume server");
        let line = read_local_ndjson_line(
            &mut client,
            &mut pending,
            Some(Instant::now() + Duration::from_secs(2)),
            MAX_NDJSON_LINE_BYTES,
        )
        .unwrap()
        .expect("resumed line");
        assert!(line.contains("true"), "{line}");
        assert!(pending.is_empty());
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn idle_reader_honors_deadline_without_timeout_setters() {
        let (listener, path) = local_pair("idle");
        let advertised = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            let _stream = listener.accept().expect("accept");
            thread::sleep(Duration::from_millis(500));
        });

        let mut client = connect_local_stream(&advertised, Instant::now() + Duration::from_secs(2))
            .expect("connect");
        let started = Instant::now();
        let mut pending = Vec::new();
        let error = read_local_ndjson_line(
            &mut client,
            &mut pending,
            Some(Instant::now() + Duration::from_millis(180)),
            MAX_NDJSON_LINE_BYTES,
        )
        .expect_err("deadline");
        assert!(
            matches!(
                error,
                BoundedNdjsonReadError::Protocol(HerdrProtocolError::TimedOut)
            ),
            "{error:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        let _ = server.join();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reader_accepts_exact_cap_and_rejects_hostile_lines() {
        let prefix =
            b"{\"result\":{\"type\":\"pong\",\"version\":\"0.8.0\",\"protocol\":19,\"pad\":\"";
        let suffix = b"\"}}";
        let pad = MAX_NDJSON_LINE_BYTES - prefix.len() - suffix.len();
        let mut at_cap = Vec::with_capacity(MAX_NDJSON_LINE_BYTES + 1);
        at_cap.extend_from_slice(prefix);
        at_cap.extend(std::iter::repeat_n(b'a', pad));
        at_cap.extend_from_slice(suffix);
        assert_eq!(at_cap.len(), MAX_NDJSON_LINE_BYTES);
        at_cap.push(b'\n');

        let mut over = at_cap.clone();
        over.insert(over.len() - 1, b'b');

        let (listener, path) = local_pair("cap");
        let advertised = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            let mut stream = listener.accept().unwrap();
            stream.write_all(&at_cap).unwrap();
            let mut stream = listener.accept().unwrap();
            stream.write_all(&over).unwrap();
            let mut stream = listener.accept().unwrap();
            stream.write_all(&[0xff, 0xfe, b'\n']).unwrap();
            let mut stream = listener.accept().unwrap();
            stream
                .write_all(&vec![b'x'; MAX_NDJSON_LINE_BYTES + 2])
                .unwrap();
        });

        let mut client = connect_framing_test_client(&advertised);
        let mut pending = Vec::new();
        let ok = read_framing_test_line(&mut client, &mut pending)
            .unwrap()
            .unwrap();
        assert_eq!(ok.len(), MAX_NDJSON_LINE_BYTES + 1);

        let mut client = connect_framing_test_client(&advertised);
        let mut pending = Vec::new();
        let over_err = read_framing_test_line(&mut client, &mut pending).unwrap_err();
        assert!(
            matches!(
                over_err,
                BoundedNdjsonReadError::Protocol(HerdrProtocolError::LineTooLarge)
            ),
            "{over_err:?}"
        );

        let mut client = connect_framing_test_client(&advertised);
        let mut pending = Vec::new();
        let utf8_err = read_framing_test_line(&mut client, &mut pending).unwrap_err();
        assert!(
            matches!(
                utf8_err,
                BoundedNdjsonReadError::Protocol(HerdrProtocolError::InvalidUtf8)
            ),
            "{utf8_err:?}"
        );

        let mut client = connect_framing_test_client(&advertised);
        let mut pending = Vec::new();
        let unterminated = read_framing_test_line(&mut client, &mut pending).unwrap_err();
        assert!(
            matches!(
                unterminated,
                BoundedNdjsonReadError::Protocol(HerdrProtocolError::UnterminatedOverLimit)
            ),
            "{unterminated:?}"
        );
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn empty_peer_close_is_none_not_a_line() {
        let (listener, path) = local_pair("eof");
        let advertised = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            let _stream = listener.accept().unwrap();
        });
        let mut client =
            connect_local_stream(&advertised, Instant::now() + Duration::from_secs(2)).unwrap();
        let mut pending = Vec::new();
        let line = read_local_ndjson_line(
            &mut client,
            &mut pending,
            Some(Instant::now() + Duration::from_secs(2)),
            MAX_NDJSON_LINE_BYTES,
        )
        .unwrap();
        assert!(line.is_none());
        server.join().unwrap();
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn connect_rejects_elapsed_deadline_without_waiting() {
        let path = unique_local_socket_path("expired");
        let advertised = path.to_string_lossy().into_owned();
        let started = Instant::now();
        let error = connect_local_stream(&advertised, Instant::now() - Duration::from_millis(1))
            .expect_err("elapsed deadline");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_millis(200));
    }

    #[test]
    fn write_honors_deadline_when_peer_does_not_read() {
        let (listener, path) = local_pair("write-backpressure");
        let advertised = path.to_string_lossy().into_owned();
        let server = thread::spawn(move || {
            let _stream = listener.accept().unwrap();
            thread::sleep(Duration::from_secs(2));
        });
        let mut client =
            connect_local_stream(&advertised, Instant::now() + Duration::from_secs(2)).unwrap();
        let payload = vec![b'x'; 4 * 1024 * 1024];
        let started = Instant::now();
        let error = write_local_all_until(
            &mut client,
            &payload,
            Instant::now() + Duration::from_millis(250),
        )
        .expect_err("deadline");
        assert_eq!(error.kind(), io::ErrorKind::TimedOut, "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(client);
        let _ = server.join();
        let _ = std::fs::remove_file(path);
    }

    #[cfg(windows)]
    #[test]
    fn windows_generic_namespaced_mapping_prepends_local_pipe_namespace() {
        assert_eq!(
            windows_generic_namespaced_pipe_path(r"C:\Users\me\herdr.sock"),
            r"\\.\pipe\C:\Users\me\herdr.sock"
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_busy_pipe_connect_returns_by_deadline() {
        let (listener, path) = local_pair("busy-pipe");
        let advertised = path.to_string_lossy().into_owned();
        // Occupy the listener's only waiting instance without accept(), so the
        // server does not create another instance for a second client.
        let occupant =
            connect_local_stream(&advertised, Instant::now() + Duration::from_secs(2)).unwrap();
        let started = Instant::now();
        let error = connect_local_stream(&advertised, Instant::now() + Duration::from_millis(250))
            .expect_err("busy pipe should not connect unbounded");
        assert!(
            matches!(
                error.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ),
            "{error:?}"
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(occupant);
        drop(listener);
        let _ = std::fs::remove_file(path);
    }

    #[cfg(windows)]
    #[test]
    fn windows_named_pipe_distinguishes_idle_from_closed_peer() {
        let (listener, path) = local_pair("win-idle");
        let advertised = path.to_string_lossy().into_owned();
        let client =
            connect_local_stream(&advertised, Instant::now() + Duration::from_secs(2)).unwrap();
        let mut server = listener.accept().unwrap();
        let mut buffer = [0u8; 1];
        assert!(matches!(
            poll_local_stream_read(&mut server, &mut buffer).unwrap(),
            LocalStreamRead::Pending
        ));
        drop(client);
        assert!(matches!(
            poll_local_stream_read(&mut server, &mut buffer).unwrap(),
            LocalStreamRead::Closed
        ));
        let _ = std::fs::remove_file(path);
    }
}
