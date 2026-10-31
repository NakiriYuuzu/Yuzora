//! Framed mode codec (`[channel u32 LE][kind u8][len u32 LE][payload]`, upstream
//! `schema/README.md` "Framed mode") and viewer-side credit accounting.

/// Header bytes.
pub const HEADER: usize = 9;
/// Largest payload the core accepts or sends.
pub const MAX_PAYLOAD: usize = 16 << 20;

pub const DATA: u8 = 0;
pub const CREDIT: u8 = 1;
pub const CLOSE: u8 = 2;
pub const RESIZE: u8 = 3;
pub const FOCUS: u8 = 4;
pub const FACTS: u8 = 5;

/// Credit a stream starts with before the viewer acknowledges anything.
pub const INITIAL_CREDIT: u64 = 256 * 1024;
/// A viewer further behind than this gets a fresh repaint instead of the backlog.
pub const MAX_PENDING: u64 = 4 << 20;

pub fn kind_name(kind: u8) -> &'static str {
    match kind {
        DATA => "DATA",
        CREDIT => "CREDIT",
        CLOSE => "CLOSE",
        RESIZE => "RESIZE",
        FOCUS => "FOCUS",
        FACTS => "FACTS",
        _ => "UNKNOWN",
    }
}

pub fn kind_from_name(name: &str) -> Option<u8> {
    Some(match name {
        "DATA" => DATA,
        "CREDIT" => CREDIT,
        "CLOSE" => CLOSE,
        "RESIZE" => RESIZE,
        "FOCUS" => FOCUS,
        "FACTS" => FACTS,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub channel: u32,
    pub kind: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn new(channel: u32, kind: u8, payload: impl Into<Vec<u8>>) -> Frame {
        Frame {
            channel,
            kind,
            payload: payload.into(),
        }
    }

    pub fn data(channel: u32, bytes: impl Into<Vec<u8>>) -> Frame {
        Frame::new(channel, DATA, bytes)
    }

    pub fn credit(channel: u32, bytes: u32) -> Frame {
        Frame::new(channel, CREDIT, bytes.to_le_bytes().to_vec())
    }

    /// A viewer detaching: an empty CLOSE.
    pub fn close(channel: u32) -> Frame {
        Frame::new(channel, CLOSE, Vec::new())
    }

    pub fn resize(channel: u32, cols: u16, rows: u16) -> Frame {
        let mut payload = cols.to_le_bytes().to_vec();
        payload.extend(rows.to_le_bytes());
        Frame::new(channel, RESIZE, payload)
    }

    pub fn focus(channel: u32, focused: bool) -> Frame {
        Frame::new(channel, FOCUS, vec![u8::from(focused)])
    }

    pub fn facts(channel: u32, facts: &serde_json::Value) -> Frame {
        Frame::new(channel, FACTS, facts.to_string().into_bytes())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.payload.len());
        out.extend_from_slice(&self.channel.to_le_bytes());
        out.push(self.kind);
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    pub fn credit_value(&self) -> Option<u32> {
        let bytes: [u8; 4] = self.payload.as_slice().try_into().ok()?;
        (self.kind == CREDIT).then(|| u32::from_le_bytes(bytes))
    }

    pub fn resize_value(&self) -> Option<(u16, u16)> {
        let [c0, c1, r0, r1]: [u8; 4] = self.payload.as_slice().try_into().ok()?;
        (self.kind == RESIZE).then(|| (u16::from_le_bytes([c0, c1]), u16::from_le_bytes([r0, r1])))
    }

    pub fn close_reason(&self) -> Option<CloseReason> {
        (self.kind == CLOSE).then(|| CloseReason::parse(&self.payload))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    TooLarge(usize),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::TooLarge(n) => write!(f, "frame of {n} bytes exceeds {MAX_PAYLOAD}"),
        }
    }
}

/// Incremental decoder: reads arrive in arbitrary pieces.
#[derive(Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Frame>, FrameError> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        let mut at = 0;
        while self.buf.len() - at >= HEADER {
            let header = &self.buf[at..at + HEADER];
            let channel = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
            let kind = header[4];
            let len = u32::from_le_bytes([header[5], header[6], header[7], header[8]]) as usize;
            if len > MAX_PAYLOAD {
                return Err(FrameError::TooLarge(len));
            }
            if self.buf.len() - at - HEADER < len {
                break;
            }
            let payload = self.buf[at + HEADER..at + HEADER + len].to_vec();
            out.push(Frame {
                channel,
                kind,
                payload,
            });
            at += HEADER + len;
        }
        self.buf.drain(..at);
        Ok(out)
    }

    /// Bytes of an incomplete frame still held.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}

/// Why a stream ended, from the core's CLOSE payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseReason {
    /// Empty CLOSE: the core's answer to our detach.
    Detached,
    /// `{"reason": ..., "exit_code": ..., "signal": ...}`.
    Ended {
        reason: String,
        exit_code: Option<i64>,
        signal: Option<i64>,
    },
    Unparsed(String),
}

impl CloseReason {
    pub fn parse(payload: &[u8]) -> CloseReason {
        if payload.is_empty() {
            return CloseReason::Detached;
        }
        match serde_json::from_slice::<serde_json::Value>(payload) {
            Ok(v) => match v.get("reason").and_then(|r| r.as_str()) {
                Some(reason) => CloseReason::Ended {
                    reason: reason.to_string(),
                    exit_code: v.get("exit_code").and_then(|c| c.as_i64()),
                    signal: v.get("signal").and_then(|c| c.as_i64()),
                },
                None => CloseReason::Unparsed(String::from_utf8_lossy(payload).into_owned()),
            },
            Err(_) => CloseReason::Unparsed(String::from_utf8_lossy(payload).into_owned()),
        }
    }

    /// Whether the terminal comes back with a new shell, so a viewer should wait for
    /// `restarted` and attach again. Upstream: only a *kept* terminal does. The same CLOSE reason
    /// (`holder_lost`, `server_stop`) finishes one that is not kept, and the payload does not say
    /// which it was, so the caller supplies `keep` from the terminal's record: the event store's
    /// (`keep` events update it) or the one the attach returned. `signaled` is only ever
    /// reported for kept terminals. Any other reason is final.
    pub fn reattach_after_restart(&self, keep: bool) -> bool {
        keep && matches!(
            self,
            CloseReason::Ended { reason, .. }
                if reason == "signaled" || reason == "holder_lost" || reason == "server_stop"
        )
    }
}

/// Viewer-side credit: credit what was written out to the renderer, batched so a busy stream
/// does not send one CREDIT frame per DATA frame.
#[derive(Debug, Clone)]
pub struct CreditLedger {
    received: u64,
    credited: u64,
    batch: u64,
}

impl CreditLedger {
    /// `batch`: credit is sent once this many consumed bytes are not yet acknowledged.
    pub fn new(batch: u64) -> CreditLedger {
        CreditLedger {
            received: 0,
            credited: 0,
            batch: batch.clamp(1, INITIAL_CREDIT),
        }
    }

    /// Bytes consumed by the renderer. Returns the CREDIT value to send now, if any.
    pub fn consumed(&mut self, bytes: usize) -> Option<u32> {
        self.received += bytes as u64;
        let owed = self.received - self.credited;
        (owed >= self.batch).then(|| self.take())
    }

    /// Credit everything consumed so far (the timer path, when a stream goes quiet).
    pub fn flush(&mut self) -> Option<u32> {
        (self.received > self.credited).then(|| self.take())
    }

    fn take(&mut self) -> u32 {
        let owed = (self.received - self.credited).min(u64::from(u32::MAX));
        self.credited += owed;
        owed as u32
    }

    /// Consumed bytes not credited yet.
    pub fn owed(&self) -> u64 {
        self.received - self.credited
    }

    pub fn received(&self) -> u64 {
        self.received
    }

    /// Bytes the core may still send before it stalls: initial window plus credits minus
    /// consumed.
    pub fn window_left(&self) -> i64 {
        INITIAL_CREDIT as i64 + self.credited as i64 - self.received as i64
    }
}

/// Direction of a recorded frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    /// Core to client.
    In,
    /// Client to core.
    Out,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct WindowStats {
    pub data_bytes_in: u64,
    pub credit_sent: u64,
    /// Largest `received - credited` ever seen: never above the initial window.
    pub max_outstanding: u64,
    pub data_frames_in: usize,
    pub largest_data_frame: usize,
}

/// Checks a recorded stream against the credit rule: the core never has more than
/// `INITIAL_CREDIT + credits sent so far` bytes of DATA out on `channel`.
pub fn check_window(frames: &[(Dir, Frame)], channel: u32) -> Result<WindowStats, String> {
    let mut stats = WindowStats::default();
    for (i, (dir, frame)) in frames.iter().enumerate() {
        if frame.channel != channel {
            continue;
        }
        match (dir, frame.kind) {
            (Dir::In, DATA) => {
                stats.data_bytes_in += frame.payload.len() as u64;
                stats.data_frames_in += 1;
                stats.largest_data_frame = stats.largest_data_frame.max(frame.payload.len());
                let outstanding = stats.data_bytes_in.saturating_sub(stats.credit_sent);
                stats.max_outstanding = stats.max_outstanding.max(outstanding);
                if stats.data_bytes_in > INITIAL_CREDIT + stats.credit_sent {
                    return Err(format!(
                        "frame {i}: {} bytes in with only {} credit (initial {INITIAL_CREDIT} + sent {})",
                        stats.data_bytes_in,
                        INITIAL_CREDIT + stats.credit_sent,
                        stats.credit_sent
                    ));
                }
            }
            (Dir::Out, CREDIT) => {
                stats.credit_sent += u64::from(
                    frame
                        .credit_value()
                        .ok_or_else(|| format!("frame {i}: bad CREDIT"))?,
                )
            }
            _ => {}
        }
    }
    Ok(stats)
}

/// Marker the core appends to every paint (first paint and lag repaint): `CSI ?9001h|l`
/// (upstream `viewers.rs` `first_paint`). Counting it in a stream counts paints, as long as the
/// program itself never emits 9001 (true for the macOS test programs; ConPTY on Windows does).
pub fn count_paint_markers(stream: &[u8]) -> usize {
    let (on, off) = (b"\x1b[?9001h", b"\x1b[?9001l");
    stream
        .windows(on.len())
        .filter(|w| *w == on || *w == off)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_decode_round_trip_in_pieces() {
        let frames = vec![
            Frame::data(0, br#"{"jsonrpc":"2.0","id":1}"#.to_vec()),
            Frame::credit(3, 65536),
            Frame::resize(3, 120, 40),
            Frame::focus(3, true),
            Frame::close(3),
            Frame::data(3, vec![0x1b; 70_000]),
        ];
        let wire: Vec<u8> = frames.iter().flat_map(|f| f.encode()).collect();
        for piece in [1, 2, 7, 9, 10, 4096, wire.len()] {
            let mut decoder = Decoder::default();
            let mut got = Vec::new();
            for chunk in wire.chunks(piece) {
                got.extend(decoder.push(chunk).unwrap());
            }
            assert_eq!(got, frames, "piece size {piece}");
            assert_eq!(decoder.pending(), 0);
        }
        assert_eq!(frames[1].credit_value(), Some(65536));
        assert_eq!(frames[2].resize_value(), Some((120, 40)));
        assert_eq!(frames[4].close_reason(), Some(CloseReason::Detached));
        assert_eq!(
            frames[0].credit_value(),
            None,
            "only CREDIT frames carry credit"
        );
    }

    #[test]
    fn oversized_frame_is_refused() {
        let mut header = vec![0u8; HEADER];
        header[5..9].copy_from_slice(&((MAX_PAYLOAD + 1) as u32).to_le_bytes());
        assert_eq!(
            Decoder::default().push(&header),
            Err(FrameError::TooLarge(MAX_PAYLOAD + 1))
        );
    }

    #[test]
    fn credit_ledger_batches_and_tracks_window() {
        let mut ledger = CreditLedger::new(64 * 1024);
        assert_eq!(ledger.consumed(16 * 1024), None);
        assert_eq!(ledger.window_left(), (INITIAL_CREDIT - 16 * 1024) as i64);
        assert_eq!(ledger.consumed(48 * 1024), Some(64 * 1024));
        assert_eq!(ledger.window_left(), INITIAL_CREDIT as i64);
        assert_eq!(ledger.consumed(10), None);
        assert_eq!(ledger.owed(), 10);
        assert_eq!(ledger.flush(), Some(10));
        assert_eq!(ledger.flush(), None);
    }

    #[test]
    fn window_check_rejects_data_beyond_credit() {
        let chunk = Frame::data(1, vec![0; 64 * 1024]);
        let mut frames: Vec<(Dir, Frame)> = (0..4).map(|_| (Dir::In, chunk.clone())).collect();
        assert_eq!(
            check_window(&frames, 1).unwrap().max_outstanding,
            INITIAL_CREDIT
        );
        frames.push((Dir::In, Frame::data(1, vec![0; 1])));
        assert!(
            check_window(&frames, 1).is_err(),
            "one byte past the initial window"
        );
        frames.insert(4, (Dir::Out, Frame::credit(1, 1)));
        assert!(check_window(&frames, 1).is_ok());
    }

    #[test]
    fn close_reasons_parse() {
        assert_eq!(
            CloseReason::parse(br#"{"reason":"exited","exit_code":0,"signal":null}"#),
            CloseReason::Ended {
                reason: "exited".into(),
                exit_code: Some(0),
                signal: None
            }
        );
        assert!(matches!(
            CloseReason::parse(b"not json"),
            CloseReason::Unparsed(_)
        ));
        assert!(matches!(
            CloseReason::parse(br#"{"no":"reason"}"#),
            CloseReason::Unparsed(_)
        ));
    }

    #[test]
    fn only_a_kept_terminal_comes_back_with_a_new_shell() {
        let ended = |reason: &str| {
            CloseReason::parse(
                format!(r#"{{"reason":"{reason}","exit_code":null,"signal":null}}"#).as_bytes(),
            )
        };
        // The payload of a kept terminal's loss and of a plain terminal's end is the same
        // text; only the terminal's `keep` tells them apart.
        for reason in ["signaled", "holder_lost", "server_stop"] {
            assert!(ended(reason).reattach_after_restart(true), "{reason} kept");
            assert!(
                !ended(reason).reattach_after_restart(false),
                "{reason} not kept: finished for good"
            );
        }
        // Everything else is final, kept or not.
        for reason in [
            "exited",
            "killed",
            "host_shutdown",
            "stopped_by_restart",
            "something_new",
        ] {
            for keep in [true, false] {
                assert!(
                    !ended(reason).reattach_after_restart(keep),
                    "{reason} keep={keep}"
                );
            }
        }
        // Our own detach and a payload we cannot read never promise a new shell.
        assert!(!CloseReason::Detached.reattach_after_restart(true));
        assert!(!CloseReason::parse(b"not json").reattach_after_restart(true));
    }
}
