//! Installation owns a separate stream; progress coalesces and SQL/terminal/UI
//! control lanes remain available while a download or package manager runs.
use crate::protocol::{ConnectionOwner, Outcome};
use crate::stream_protocol::*;
use crate::{streams::write_frame, wire::read_frame};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(crate) async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut input: BufReader<R>,
    mut output: W,
    owner: ConnectionOwner,
    id: String,
    path: String,
    language: String,
    global: bool,
) -> Result<(), String> {
    let cancel = Cancel(Arc::new(AtomicBool::new(false)));
    let token = cancel.0.clone();
    let host_id = owner.host_id.clone();
    let (progress, mut received) = tokio::sync::watch::channel(None);
    let mut job = tokio::task::spawn_blocking(move || {
        crate::cancellation::with_cancellation(token, || {
            let identity = crate::trust_command::host_trust(&host_id)?.require_trusted(&path)?;
            if identity.canonical_path != path {
                return Err("workspace-not-canonical".into());
            }
            crate::lsp_download::install((!global).then_some(path.as_str()), &language, &|event| {
                progress.send_replace(Some(event));
            })
        })
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
    let mut progress_open = true;
    let next = read_frame(&mut input);
    tokio::pin!(next);
    let bytes = loop {
        tokio::select! {
            bytes = &mut next => break bytes?,
            changed = received.changed(), if progress_open => {
                if changed.is_err() { progress_open = false; }
                let event = received.borrow_and_update().clone();
                if let Some(event) = event { write_frame(&mut output, &owner, StreamPayload::LspInstallProgress { event }).await?; }
            }
            result = &mut job => {
                let outcome = match result.map_err(|e| e.to_string())? {
                    Ok(info) => Outcome::Ok { value: serde_json::to_value(info).map_err(|e| e.to_string())? },
                    Err(message) => Outcome::Error { code: "lsp-install-failed".into(), message },
                };
                write_frame(&mut output, &owner, StreamPayload::LspInstalled { outcome }).await?;
                return Ok(());
            }
        }
    };
    let Some(bytes) = bytes else {
        return Ok(());
    };
    let request: StreamRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    request.validate(&owner)?;
    if !matches!(request.operation, StreamCommand::Close) {
        return Err("invalid-install-command".into());
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
