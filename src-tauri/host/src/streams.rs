use crate::herdr_service::*;
use crate::protocol::{ConnectionOwner, Outcome, MAX_FRAME_BYTES, PROTOCOL_VERSION};
use crate::stream_protocol::*;
use crate::wire::read_frame;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, watch};

// This guard lives inside the opening task too: a cancelled opener must still
// release any connector that finishes starting after the caller has gone away.
struct OwnedRuntime(Arc<HerdrManager>);
impl Drop for OwnedRuntime {
    fn drop(&mut self) {
        self.0.release_all_connectors();
    }
}

pub(crate) async fn write_frame<W: AsyncWrite + Unpin>(
    output: &mut W,
    owner: &ConnectionOwner,
    payload: StreamPayload,
) -> Result<(), String> {
    let capacity = match &payload {
        StreamPayload::Terminal {
            event: HerdrTerminalEvent::Frame { bytes_base64, .. },
        } if bytes_base64.len() >= 4096 => {
            // Leave room for the envelope without doubling a large payload buffer.
            bytes_base64.len().saturating_add(512).min(MAX_FRAME_BYTES)
        }
        _ => 128,
    };
    let mut frame = Vec::with_capacity(capacity);
    serde_json::to_writer(
        &mut frame,
        &StreamFrame {
            version: PROTOCOL_VERSION,
            owner: owner.clone(),
            payload,
        },
    )
    .map_err(|e| e.to_string())?;
    if frame.len() >= MAX_FRAME_BYTES {
        return Err("frame-too-large".into());
    }
    frame.push(b'\n');
    tokio::time::timeout(Duration::from_secs(10), async {
        output.write_all(&frame).await?;
        output.flush().await
    })
    .await
    .map_err(|_| "stream-write-timeout".to_string())?
    .map_err(|e| e.to_string())
}

fn outcome(result: Result<Value, String>) -> Outcome {
    match result {
        Ok(value) => Outcome::Ok { value },
        Err(message) => Outcome::Error {
            code: message.split(':').next().unwrap_or("stream-error").into(),
            message,
        },
    }
}

pub async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    input: R,
    mut output: W,
) -> Result<(), String> {
    let mut input = BufReader::new(input);
    let first = tokio::time::timeout(Duration::from_secs(15), read_frame(&mut input))
        .await
        .map_err(|_| "stream-open-timeout")??
        .ok_or("stream-closed")?;
    let first: StreamRequest = serde_json::from_slice(&first).map_err(|e| e.to_string())?;
    let owner = first.owner.clone();
    first.validate(&owner)?;
    let StreamCommand::Open { config } = first.operation else {
        return Err("stream-open-required".into());
    };
    if let StreamConfig::Search {
        path,
        query,
        case_sensitive,
    } = config
    {
        return crate::search_stream::serve(
            input,
            output,
            owner,
            first.id,
            path,
            query,
            case_sensitive,
        )
        .await;
    }
    let native_client = matches!(&config, StreamConfig::Client { .. });
    let (events_tx, events_rx) = mpsc::channel(STREAM_QUEUE_CAPACITY);
    let (overflow_tx, mut overflow_rx) = watch::channel(false);
    let emit = move |event| {
        // HERDR replays its retained topology events at subscription start.
        // The subscription reader is a dedicated blocking thread: apply bounded
        // backpressure there instead of failing every reconnect on a short burst.
        if native_client || matches!(event, StreamPayload::Subscription { .. }) {
            return events_tx
                .blocking_send(event)
                .map_err(|_| "stream-closed".to_string());
        }
        events_tx.try_send(event).map_err(|_| {
            let _ = overflow_tx.send(true);
            "stream-backpressure-exceeded".to_string()
        })
    };
    let host_id = owner.host_id.clone();
    let opened = tokio::task::spawn_blocking(move || {
        if let StreamConfig::Git {
            path,
            repository_root,
        } = &config
        {
            let trust = crate::trust_command::host_trust(&host_id)?;
            let identity = trust.require_trusted(path)?;
            if identity.canonical_path != *path {
                return Err("workspace-not-canonical".into());
            }
            let crate::git_service::GitEnvironment::Ready { root, .. } =
                crate::git_service::detect_environment(std::path::Path::new(path))
            else {
                return Err("git-repository-not-open".into());
            };
            if root != *repository_root {
                return Err("git-repository-identity-mismatch".into());
            }
            let id = path.clone();
            let workspace_root = path.clone();
            let watcher = crate::git_watch::build_repository_watcher(
                std::path::Path::new(&root),
                move || {
                    let _ = emit(StreamPayload::Git {
                        workspace_root: workspace_root.clone(),
                    });
                },
            )?;
            return Ok((
                None,
                Some(Box::new(watcher) as Box<dyn Send>),
                id.clone(),
                Value::String(id),
                false,
            ));
        }
        if let StreamConfig::Files { path } = &config {
            let root = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
            let id = root.to_str().ok_or("path-not-utf8")?.to_owned();
            let workspace_root = id.clone();
            let watcher = crate::watcher::build_watcher(&root, move |paths| {
                let _ = emit(StreamPayload::Files {
                    workspace_root: workspace_root.clone(),
                    paths,
                });
            })?;
            return Ok((
                None,
                Some(Box::new(watcher) as Box<dyn Send>),
                id.clone(),
                Value::String(id),
                false,
            ));
        }
        let binary = match &config {
            StreamConfig::Client { binary, .. }
            | StreamConfig::Terminal { binary, .. }
            | StreamConfig::Events { binary, .. } => binary,
            StreamConfig::Files { .. } | StreamConfig::Search { .. } | StreamConfig::Git { .. } => {
                unreachable!()
            }
        };
        let runtime = OwnedRuntime(Arc::new(if binary == "herdr" {
            HerdrManager::new()
        } else {
            HerdrManager::with_binary(binary.into())
        }));
        let (id, value, terminal) = match config {
            StreamConfig::Client {
                session_name, size, ..
            } => {
                let opened = runtime.0.open_native_client(
                    &session_name,
                    size,
                    Arc::new(move |event| emit(StreamPayload::Terminal { event })),
                )?;
                (
                    opened.session_id.clone(),
                    serde_json::to_value(opened).map_err(|e| e.to_string())?,
                    true,
                )
            }
            StreamConfig::Files { .. } | StreamConfig::Search { .. } | StreamConfig::Git { .. } => {
                unreachable!()
            }
            StreamConfig::Terminal {
                session_name,
                target,
                mode,
                takeover,
                cols,
                rows,
                ..
            } => {
                let opened = runtime.0.open_terminal(
                    target,
                    mode,
                    takeover,
                    cols,
                    rows,
                    Some(session_name),
                    Arc::new(move |event| emit(StreamPayload::Terminal { event })),
                )?;
                (
                    opened.session_id.clone(),
                    serde_json::to_value(opened).map_err(|e| e.to_string())?,
                    true,
                )
            }
            StreamConfig::Events {
                session_name,
                pane_ids,
                ..
            } => {
                let id = runtime.0.events_subscribe(
                    Some(session_name),
                    pane_ids,
                    Arc::new(move |event| emit(StreamPayload::Subscription { event })),
                )?;
                (id.clone(), Value::String(id), false)
            }
        };
        Ok::<_, String>((Some(runtime), None, id, value, terminal))
    })
    .await
    .map_err(|e| e.to_string())?;
    let (runtime, _watcher, resource_id, value, terminal) = match opened {
        Ok(opened) => opened,
        Err(error) => {
            write_frame(
                &mut output,
                &owner,
                StreamPayload::Reply {
                    id: first.id,
                    outcome: outcome(Err(error)),
                },
            )
            .await?;
            return Ok(());
        }
    };
    // Drop the receiver before OwnedRuntime joins the subscription reader, so
    // EOF/write failure also unblocks a reader waiting on a full bounded queue.
    let mut events_rx = events_rx;
    write_frame(
        &mut output,
        &owner,
        StreamPayload::Reply {
            id: first.id,
            outcome: Outcome::Ok { value },
        },
    )
    .await?;
    let mut overflow_open = true;
    loop {
        // Keep the same read future across output events: cancelling read_frame
        // after a partial read would discard bytes and corrupt the next command.
        let next = read_frame(&mut input);
        tokio::pin!(next);
        let bytes = loop {
            tokio::select! {
                frame = &mut next => break frame?,
                changed = overflow_rx.changed(), if overflow_open => {
                    if *overflow_rx.borrow() { return Err("stream-backpressure-exceeded".into()); }
                    if changed.is_err() { overflow_open = false; }
                },
                event = events_rx.recv() => {
                    let Some(event) = event else {return Ok(());};
                    write_frame(&mut output, &owner, event).await?;
                },
            }
        };
        let Some(bytes) = bytes else {
            return Ok(());
        };
        let request: StreamRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        request.validate(&owner)?;
        let close = matches!(request.operation, StreamCommand::Close);
        let manager = runtime.as_ref().map(|runtime| runtime.0.clone());
        let id = resource_id.clone();
        let result = tokio::task::spawn_blocking(move || match request.operation {
            StreamCommand::Close => Ok(Value::Null),
            StreamCommand::Input { text, bytes_base64 } if terminal => manager
                .ok_or("terminal-unavailable")?
                .terminal_input(&id, text, bytes_base64)
                .map(|_| Value::Null),
            StreamCommand::Resize { cols, rows } if terminal => manager
                .ok_or("terminal-unavailable")?
                .terminal_resize(&id, cols, rows)
                .map(|_| Value::Null),
            StreamCommand::Scroll {
                direction,
                lines,
                column,
                row,
            } if terminal => manager
                .ok_or("terminal-unavailable")?
                .terminal_scroll(&id, direction, lines, column, row)
                .map(|_| Value::Null),
            _ => Err("invalid-stream-command".into()),
        })
        .await
        .map_err(|e| e.to_string())?;
        write_frame(
            &mut output,
            &owner,
            StreamPayload::Reply {
                id: request.id,
                outcome: outcome(result),
            },
        )
        .await?;
        if close {
            return Ok(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terminal_frame(data: String, metadata: &str) -> StreamFrame {
        StreamFrame {
            version: PROTOCOL_VERSION,
            owner: ConnectionOwner {
                host_id: metadata.into(),
                generation: u64::MAX,
            },
            payload: StreamPayload::Terminal {
                event: HerdrTerminalEvent::Frame {
                    session_id: metadata.into(),
                    seq: u64::MAX,
                    full: true,
                    encoding: "base64".into(),
                    width: u32::MAX,
                    height: u32::MAX,
                    bytes_base64: data,
                },
            },
        }
    }

    #[tokio::test]
    async fn terminal_capacity_hint_preserves_wire_bytes_and_size_limit() {
        for size in [0, 1, 4095, 4096, 4097, 65536, 1024 * 1024] {
            for (unit, metadata) in [("A", "host"), ("\0\n\"\\", "host"), ("終端", "主機")] {
                let frame = terminal_frame(unit.repeat(size), metadata);
                let mut expected = serde_json::to_vec(&frame).unwrap();
                expected.push(b'\n');
                let mut actual = Vec::new();
                write_frame(&mut actual, &frame.owner, frame.payload)
                    .await
                    .unwrap();
                assert_eq!(actual, expected);
            }
        }
        let frame = terminal_frame("A".repeat(4096), &"large metadata".repeat(1024));
        let mut expected = serde_json::to_vec(&frame).unwrap();
        expected.push(b'\n');
        let mut actual = Vec::new();
        write_frame(&mut actual, &frame.owner, frame.payload)
            .await
            .unwrap();
        assert_eq!(actual, expected);

        let overhead = serde_json::to_vec(&terminal_frame(String::new(), "host"))
            .unwrap()
            .len();
        for size in [MAX_FRAME_BYTES - 1, MAX_FRAME_BYTES, MAX_FRAME_BYTES + 1] {
            let frame = terminal_frame("A".repeat(size - overhead), "host");
            let mut actual = Vec::new();
            let result = write_frame(&mut actual, &frame.owner, frame.payload).await;
            if size < MAX_FRAME_BYTES {
                result.unwrap();
                assert_eq!(actual.len(), MAX_FRAME_BYTES);
                assert_eq!(actual.last(), Some(&b'\n'));
            } else {
                assert_eq!(result.unwrap_err(), "frame-too-large");
                assert!(actual.is_empty());
            }
        }
    }

    #[tokio::test]
    async fn terminal_capacity_hint_preserves_write_and_flush_errors() {
        use std::pin::Pin;
        use std::task::{Context, Poll};
        struct FailOutput {
            fail_write: bool,
            bytes: usize,
        }
        impl AsyncWrite for FailOutput {
            fn poll_write(
                mut self: Pin<&mut Self>,
                _: &mut Context<'_>,
                bytes: &[u8],
            ) -> Poll<std::io::Result<usize>> {
                if self.fail_write {
                    Poll::Ready(Err(std::io::Error::other("owned-write-error")))
                } else {
                    self.bytes += bytes.len();
                    Poll::Ready(Ok(bytes.len()))
                }
            }
            fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
                Poll::Ready(Err(std::io::Error::other("owned-flush-error")))
            }
            fn poll_shutdown(
                self: Pin<&mut Self>,
                _: &mut Context<'_>,
            ) -> Poll<std::io::Result<()>> {
                Poll::Ready(Ok(()))
            }
        }
        for fail_write in [true, false] {
            let mut output = FailOutput {
                fail_write,
                bytes: 0,
            };
            let frame = terminal_frame("A".repeat(65536), "host");
            let bytes = serde_json::to_vec(&frame).unwrap().len() + 1;
            let error = write_frame(&mut output, &frame.owner, frame.payload)
                .await
                .unwrap_err();
            assert_eq!(
                error,
                if fail_write {
                    "owned-write-error"
                } else {
                    "owned-flush-error"
                }
            );
            assert_eq!(output.bytes, if fail_write { 0 } else { bytes });
        }
    }

    #[tokio::test]
    async fn rejects_control_before_opening_a_resource() {
        let request = StreamRequest {
            version: PROTOCOL_VERSION,
            owner: ConnectionOwner {
                host_id: "test".into(),
                generation: 1,
            },
            id: "1".into(),
            operation: StreamCommand::Close,
        };
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        assert_eq!(
            serve(bytes.as_slice(), tokio::io::sink())
                .await
                .unwrap_err(),
            "stream-open-required"
        );
    }
}
