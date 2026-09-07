use crate::process_service::{DevServerStatus, ProcessManager};
use crate::protocol::{ConnectionOwner, Outcome};
use crate::stream_protocol::*;
use crate::{streams::write_frame, wire::read_frame};
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

async fn write_event<W: AsyncWrite + Unpin>(
    output: &mut W,
    owner: &ConnectionOwner,
    payload: StreamPayload,
    overflowed: &mut tokio::sync::watch::Receiver<bool>,
) -> Result<(), String> {
    tokio::select! {
        biased;
        _ = async { let _ = overflowed.wait_for(|overflow| *overflow).await; } => Err("dev-server-output-backpressure".into()),
        result = write_frame(output, owner, payload) => result,
    }
}

struct OwnedProcess(Arc<ProcessManager>);
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

pub(crate) async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut input: BufReader<R>,
    mut output: W,
    owner: ConnectionOwner,
    id: String,
    path: String,
    command: String,
    port: Option<u16>,
) -> Result<(), String> {
    let (status, mut statuses) = tokio::sync::watch::channel(None);
    let manager = OwnedProcess(Arc::new(ProcessManager::with_seams(
        Box::new(|_| {}),
        Arc::new(move |info| {
            status.send_replace(Some(info));
        }),
    )));
    let (lines, mut received) = tokio::sync::mpsc::channel(STREAM_QUEUE_CAPACITY);
    let (overflow, mut overflowed) = tokio::sync::watch::channel(false);
    let starting = manager.0.clone();
    let host_id = owner.host_id.clone();
    let started = tokio::task::spawn_blocking(move || {
        let identity = crate::trust_command::host_trust(&host_id)?.require_trusted(&path)?;
        if identity.canonical_path != path {
            return Err("workspace-not-canonical".into());
        }
        // Desktop consumes the exact-command challenge on the primary helper
        // before constructing this lane. This worker rechecks persisted trust.
        starting.start(
            &path,
            &command,
            port,
            Arc::new(move |text| {
                if lines.try_send(text).is_err() {
                    overflow.send_replace(true);
                }
            }),
        )
    });
    let next = read_frame(&mut input);
    tokio::pin!(next);
    let info = tokio::select! {
        result = started => result.map_err(|e| e.to_string())??,
        _ = &mut next => return Ok(()),
    };
    write_event(
        &mut output,
        &owner,
        StreamPayload::Reply {
            id,
            outcome: Outcome::Ok {
                value: serde_json::to_value(info).map_err(|e| e.to_string())?,
            },
        },
        &mut overflowed,
    )
    .await?;
    loop {
        tokio::select! {
            biased;
            _ = async { let _ = overflowed.wait_for(|overflow| *overflow).await; } => return Err("dev-server-output-backpressure".into()),
            request = &mut next => {
                let Some(bytes) = request? else { return Ok(()); };
                let request: StreamRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
                request.validate(&owner)?;
                if !matches!(request.operation, StreamCommand::Close) { return Err("invalid-dev-server-command".into()); }
                write_frame(&mut output, &owner, StreamPayload::Reply { id: request.id, outcome: Outcome::Ok { value: serde_json::Value::Null } }).await?;
                return Ok(());
            }
            changed = statuses.changed() => {
                if changed.is_err() { return Ok(()); }
                let info = statuses.borrow_and_update().clone();
                if let Some(info) = info {
                    let terminal = matches!(info.status, DevServerStatus::Exited { .. } | DevServerStatus::Failed { .. });
                    write_event(&mut output, &owner, StreamPayload::DevServerStatus { info }, &mut overflowed).await?;
                    if terminal { return Ok(()); }
                }
            }
            Some(text) = received.recv() => write_event(&mut output, &owner, StreamPayload::DevServerOutput { text }, &mut overflowed).await?,
        }
    }
}
