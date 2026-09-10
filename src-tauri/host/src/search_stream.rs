//! Search has its own bounded stream and stops on EOF or explicit close.
use crate::protocol::{ConnectionOwner, Outcome};
use crate::search::{run_search, SearchEvent};
use crate::stream_protocol::*;
use crate::streams::write_frame;
use crate::wire::read_frame;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

struct Cancel(Arc<AtomicU64>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(2, Ordering::Release);
    }
}

pub(crate) async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut input: BufReader<R>,
    mut output: W,
    owner: ConnectionOwner,
    id: String,
    path: String,
    query: String,
    case_sensitive: bool,
) -> Result<(), String> {
    if query.len() > 4096 {
        return Err("search-query-too-large".into());
    }
    let root = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    if !root.is_dir() {
        return Err("search-root-not-directory".into());
    }
    let cancel = Cancel(Arc::new(AtomicU64::new(1)));
    let generation = cancel.0.clone();
    let (sender, mut receiver) = tokio::sync::mpsc::channel(STREAM_QUEUE_CAPACITY);
    let worker = tokio::task::spawn_blocking(move || {
        run_search(
            &root,
            &query,
            case_sensitive,
            1,
            &generation,
            &mut |event| {
                if sender.blocking_send(event).is_err() {
                    generation.store(2, Ordering::Release);
                }
            },
        );
    });
    write_frame(
        &mut output,
        &owner,
        StreamPayload::Reply {
            id,
            outcome: Outcome::Ok {
                value: serde_json::Value::Null,
            },
        },
    )
    .await?;
    {
        let next = read_frame(&mut input);
        tokio::pin!(next);
        let bytes = loop {
            tokio::select! {
                incoming = &mut next => break incoming?,
                event = receiver.recv() => {
                    let Some(event) = event else { return worker.await.map_err(|e| e.to_string()); };
                    let done = matches!(event, SearchEvent::Done { .. });
                    write_frame(&mut output, &owner, StreamPayload::Search { event }).await?;
                    if done { return worker.await.map_err(|e| e.to_string()); }
                }
            }
        };
        let Some(bytes) = bytes else {
            return Ok(());
        };
        let request: StreamRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        request.validate(&owner)?;
        if !matches!(request.operation, StreamCommand::Close) {
            return Err("invalid-search-command".into());
        }
        write_frame(
            &mut output,
            &owner,
            StreamPayload::Reply {
                id: request.id,
                outcome: Outcome::Ok {
                    value: serde_json::Value::Null,
                },
            },
        )
        .await?;
        Ok(())
    }
}
