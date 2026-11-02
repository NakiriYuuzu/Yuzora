//! Deterministic compiler used by coordinator tests.

use std::sync::{
    mpsc::{self, Receiver, SyncSender},
    Mutex,
};

use super::{CompileControl, CompileFailure, Compiler};
use crate::{code_cache::CompiledArtifact, runtime::CompileRequest};

pub struct FakeCompiler {
    requests: mpsc::Sender<CompileRequest>,
    releases: Mutex<Receiver<Result<CompiledArtifact, CompileFailure>>>,
}

pub struct FakeCompilerControl {
    requests: Mutex<Receiver<CompileRequest>>,
    releases: SyncSender<Result<CompiledArtifact, CompileFailure>>,
}

impl FakeCompiler {
    pub fn new(capacity: usize) -> (Self, FakeCompilerControl) {
        let (request_sender, request_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::sync_channel(capacity);
        (
            Self {
                requests: request_sender,
                releases: Mutex::new(release_receiver),
            },
            FakeCompilerControl {
                requests: Mutex::new(request_receiver),
                releases: release_sender,
            },
        )
    }
}

impl Compiler for FakeCompiler {
    fn compile(&self, request: CompileRequest) -> Result<CompiledArtifact, CompileFailure> {
        if self.requests.send(request.clone()).is_err() {
            return Err(CompileFailure::Cancelled);
        }
        let release = self
            .releases
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv()
            .unwrap_or(Err(CompileFailure::Cancelled));
        release.map(|artifact| artifact.bind_fake(request.artifact_key()))
    }

    fn compile_controlled(
        &self,
        request: CompileRequest,
        control: &CompileControl,
    ) -> Result<CompiledArtifact, CompileFailure> {
        if self.requests.send(request.clone()).is_err() {
            return Err(CompileFailure::Cancelled);
        }
        loop {
            control.check()?;
            match self
                .releases
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .recv_timeout(std::time::Duration::from_millis(1))
            {
                Ok(result) => {
                    return result.map(|artifact| artifact.bind_fake(request.artifact_key()))
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(CompileFailure::Cancelled)
                }
            }
        }
    }
}

impl FakeCompilerControl {
    pub fn next_request(&self) -> Option<CompileRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv()
            .ok()
    }

    pub fn request_within(&self, timeout: std::time::Duration) -> Option<CompileRequest> {
        self.requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .recv_timeout(timeout)
            .ok()
    }

    pub fn complete(&self, artifact: CompiledArtifact) {
        let _ = self.releases.send(Ok(artifact));
    }

    pub fn fail(&self, failure: CompileFailure) {
        let _ = self.releases.send(Err(failure));
    }
}
