//! JSON-RPC 2.0 messages as herdr-core speaks them (plain mode: one per line; framed mode: one
//! per channel-0 DATA frame, and NDJSON lines inside an events channel).

use serde_json::{json, Value};

pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// Plain-mode bytes for one message.
pub fn line(msg: &Value) -> Vec<u8> {
    let mut bytes = msg.to_string().into_bytes();
    bytes.push(b'\n');
    bytes
}

/// A core error: stable `data.kind`, a hint, and the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    pub kind: String,
    pub hint: String,
}

impl RpcError {
    pub fn from_json(v: &Value) -> RpcError {
        let text = |pointer: &str| {
            v.pointer(pointer)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        RpcError {
            code: v.get("code").and_then(Value::as_i64).unwrap_or(0),
            message: text("/message"),
            kind: text("/data/kind"),
            hint: text("/data/hint"),
        }
    }

    pub fn is_conflict(&self) -> bool {
        self.kind == "conflict"
    }
}

impl std::fmt::Display for RpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}): {}", self.kind, self.code, self.message)
    }
}

impl std::error::Error for RpcError {}

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response {
        id: u64,
        result: Result<Value, RpcError>,
    },
    Notification {
        method: String,
        params: Value,
    },
}

pub fn parse(bytes: &[u8]) -> Result<Incoming, String> {
    let value: Value = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    parse_value(value)
}

pub fn parse_value(v: Value) -> Result<Incoming, String> {
    if let Some(method) = v.get("method").and_then(Value::as_str) {
        if v.get("id").is_none_or(Value::is_null) {
            return Ok(Incoming::Notification {
                method: method.to_string(),
                params: v.get("params").cloned().unwrap_or(Value::Null),
            });
        }
        return Err(format!("unexpected request from core: {method}"));
    }
    let id = v
        .get("id")
        .and_then(Value::as_u64)
        .ok_or("response without numeric id")?;
    let result = match v.get("error") {
        Some(e) => Err(RpcError::from_json(e)),
        None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
    };
    Ok(Incoming::Response { id, result })
}

/// A line longer than the splitter's limit: the peer is not speaking NDJSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineTooLong(pub usize);

/// Splits NDJSON input into lines (reads cut lines anywhere), refusing unbounded lines.
pub struct LineSplitter {
    buf: Vec<u8>,
    max: usize,
}

impl LineSplitter {
    pub fn new(max: usize) -> LineSplitter {
        LineSplitter {
            buf: Vec::new(),
            max,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, LineTooLong> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        while let Some(at) = self.buf.iter().position(|b| *b == b'\n') {
            let mut line: Vec<u8> = self.buf.drain(..=at).collect();
            line.pop();
            if line.len() > self.max {
                return Err(LineTooLong(line.len()));
            }
            if !line.is_empty() {
                out.push(line);
            }
        }
        if self.buf.len() > self.max {
            return Err(LineTooLong(self.buf.len()));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_responses_errors_and_notifications() {
        let ok =
            parse(br#"{"id":2,"jsonrpc":"2.0","result":{"hidden_finished":0,"terminals":[]}}"#)
                .unwrap();
        assert!(matches!(
            ok,
            Incoming::Response {
                id: 2,
                result: Ok(_)
            }
        ));
        let err = parse(br#"{"error":{"code":-32601,"data":{"hint":"see `hc schema`","kind":"unknown_method"},"message":"unknown method \"nope\""},"id":3,"jsonrpc":"2.0"}"#).unwrap();
        match err {
            Incoming::Response {
                id: 3,
                result: Err(e),
            } => {
                assert_eq!((e.code, e.kind.as_str()), (-32601, "unknown_method"));
                assert_eq!(e.hint, "see `hc schema`");
                assert!(!e.is_conflict());
            }
            other => panic!("{other:?}"),
        }
        let note = parse(br#"{"jsonrpc":"2.0","method":"heartbeat","params":{"seq":42}}"#).unwrap();
        assert_eq!(
            note,
            Incoming::Notification {
                method: "heartbeat".into(),
                params: json!({"seq": 42})
            }
        );
        assert!(parse(br#"{"jsonrpc":"2.0","id":1,"method":"x"}"#).is_err());
        assert!(parse(br#"{"jsonrpc":"2.0","result":1}"#).is_err());
    }

    #[test]
    fn request_and_line_shape() {
        let msg = request(7, "doc.get", json!({"name": "yuzora"}));
        assert_eq!(msg["id"], 7);
        assert_eq!(msg["method"], "doc.get");
        let bytes = line(&msg);
        assert_eq!(bytes.last(), Some(&b'\n'));
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes[..bytes.len() - 1]).unwrap(),
            msg
        );
    }

    #[test]
    fn line_splitter_handles_cut_reads() {
        let mut s = LineSplitter::new(1024);
        assert!(s.push(b"{\"a\":").unwrap().is_empty());
        assert_eq!(
            s.push(b"1}\n{\"b\":2}\n{").unwrap(),
            vec![b"{\"a\":1}".to_vec(), b"{\"b\":2}".to_vec()]
        );
        assert_eq!(s.push(b"}\n").unwrap(), vec![b"{}".to_vec()]);
    }

    #[test]
    fn line_splitter_refuses_unbounded_lines() {
        let mut s = LineSplitter::new(16);
        assert!(s.push(&[b'x'; 16]).unwrap().is_empty());
        assert_eq!(s.push(b"x").unwrap_err(), LineTooLong(17));
        let mut s = LineSplitter::new(4);
        assert_eq!(s.push(b"abcdef\n").unwrap_err(), LineTooLong(6));
    }
}
