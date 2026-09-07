//! One owned LSP process/stdio channel, with no transparent process restart.
use crate::lsp_service::{LspManager, ResolvedServer};
use crate::protocol::{ConnectionOwner, Outcome};
use crate::stream_protocol::*;
use crate::{lsp_adapters, lsp_config, streams::write_frame, wire::read_frame};
use serde_json::Value;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite, BufReader};

struct OwnedLsp(Arc<LspManager>);
impl Drop for OwnedLsp {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}
type Job = tokio::task::JoinHandle<Result<Value, String>>;

pub(crate) async fn serve<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    mut input: BufReader<R>,
    mut output: W,
    owner: ConnectionOwner,
    id: String,
    path: String,
    language: String,
) -> Result<(), String> {
    let (sender, mut receiver) = tokio::sync::mpsc::channel(STREAM_QUEUE_CAPACITY);
    let (overflow, mut overflowed) = tokio::sync::watch::channel(false);
    let emit = Arc::new(move |payload| {
        if sender.try_send(payload).is_err() {
            overflow.send_replace(true);
        }
    });
    let status = emit.clone();
    let logs = dirs::home_dir()
        .ok_or("host-home-unavailable")?
        .join(".yuzora/logs");
    let manager = OwnedLsp(Arc::new(
        LspManager::with_parts(
            logs,
            0,
            Box::new(|_| {}),
            Box::new(move |info| status(StreamPayload::LspStatus { info })),
        )
        .without_restarts()
        .with_host_executables(),
    ));
    let runtime = manager.0.clone();
    let workspace = path.clone();
    let lang = language.clone();
    let host = owner.host_id.clone();
    let mut pending: Option<(String, Job)> = Some((
        id,
        tokio::task::spawn_blocking(move || {
            let identity = crate::trust_command::host_trust(&host)?.require_trusted(&workspace)?;
            if identity.canonical_path != workspace {
                return Err("workspace-not-canonical".into());
            }
            let config = lsp_config::load_from(&lsp_config::config_path());
            let id = lsp_config::resolve_server(&config, &workspace, &lang)
                .or_else(|| lsp_adapters::adapters_for(&lang).map(|a| a.default_id.to_owned()))
                .ok_or("lsp-language-unsupported")?;
            let adapter = lsp_adapters::adapter(&lang, &id).ok_or("lsp-server-unsupported")?;
            let resolved = ResolvedServer {
                server_id: id,
                command: adapter.command.into(),
                args: adapter.args.iter().map(|s| (*s).into()).collect(),
            };
            let info = runtime.start(
                &workspace,
                &lang,
                resolved,
                Arc::new(move |message| emit(StreamPayload::Lsp { message })),
            );
            serde_json::to_value(info).map_err(|e| e.to_string())
        }),
    ));
    loop {
        // A single frame read survives every output event and job completion.
        let next = read_frame(&mut input);
        tokio::pin!(next);
        let bytes = loop {
            tokio::select! {
                bytes = &mut next => break bytes?,
                _ = overflowed.changed() => return Err("lsp-output-backpressure".into()),
                event = receiver.recv() => {
                    let Some(event) = event else { return Ok(()); };
                    write_frame(&mut output, &owner, event).await?;
                }
                result = async { (&mut pending.as_mut().unwrap().1).await }, if pending.is_some() => {
                    let (id, _) = pending.take().unwrap();
                    let outcome = match result.map_err(|e| e.to_string())? {
                        Ok(value) => Outcome::Ok { value },
                        Err(message) => Outcome::Error { code: "lsp-error".into(), message },
                    };
                    write_frame(&mut output, &owner, StreamPayload::Reply { id, outcome }).await?;
                }
            }
        };
        let Some(bytes) = bytes else {
            return Ok(());
        };
        let request: StreamRequest = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        request.validate(&owner)?;
        match request.operation {
            StreamCommand::LspTrace { enabled } if pending.is_none() => {
                let outcome = match manager.0.set_trace(enabled) {
                    Ok(()) => Outcome::Ok { value: Value::Null },
                    Err(message) => Outcome::Error {
                        code: "lsp-trace-failed".into(),
                        message,
                    },
                };
                write_frame(
                    &mut output,
                    &owner,
                    StreamPayload::Reply {
                        id: request.id,
                        outcome,
                    },
                )
                .await?;
            }
            StreamCommand::Close => {
                write_frame(
                    &mut output,
                    &owner,
                    StreamPayload::Reply {
                        id: request.id,
                        outcome: Outcome::Ok { value: Value::Null },
                    },
                )
                .await?;
                return Ok(());
            }
            StreamCommand::LspMessage { message } if pending.is_none() => {
                let runtime = manager.0.clone();
                let workspace = path.clone();
                let lang = language.clone();
                pending = Some((
                    request.id,
                    tokio::task::spawn_blocking(move || {
                        runtime
                            .send(&workspace, &lang, message)
                            .map(|_| Value::Null)
                    }),
                ));
            }
            _ => return Err("invalid-or-pipelined-lsp-command".into()),
        }
    }
}
