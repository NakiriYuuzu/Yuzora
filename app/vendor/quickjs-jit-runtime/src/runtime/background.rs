use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use crate::compiler::Compiler;

use super::{CompileRequest, Coordinator};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackgroundCompilerError {
    InvalidLimit,
    Shutdown,
}

/// Bounded compiler workers. Jobs and completions contain owned, verified data
/// only; the owning runtime thread remains solely responsible for installation.
pub struct BackgroundCompiler {
    sender: Option<mpsc::SyncSender<CompileRequest>>,
    startup: Option<WorkerStartup>,
    workers: Vec<JoinHandle<()>>,
    completion_slot: Arc<Mutex<Option<super::CompletionSender>>>,
    cancelled: Arc<AtomicBool>,
    usage: Arc<WorkerUsage>,
    max_snapshot_bytes: usize,
    max_ir_bytes: usize,
    overflow: Arc<Mutex<VecDeque<super::CompileCompletion>>>,
    /// Jobs handed to the channel never exceed the workers that can run them:
    /// the backlog stays in the coordinator queue, where a Tier2 request can
    /// still be taken ahead of first Baseline compiles.
    worker_count: usize,
}

struct WorkerStartup {
    receiver: Arc<Mutex<mpsc::Receiver<CompileRequest>>>,
    compiler: Arc<dyn Compiler>,
    worker_count: usize,
    compile_budget: Duration,
}

#[derive(Debug, Default)]
struct WorkerUsage {
    jobs: AtomicUsize,
    snapshots: AtomicUsize,
    ir: AtomicUsize,
    peak_compiler: AtomicUsize,
}

struct UsageGuard {
    usage: Arc<WorkerUsage>,
    snapshot_bytes: usize,
    ir_bytes: usize,
}

impl Drop for UsageGuard {
    fn drop(&mut self) {
        self.usage.jobs.fetch_sub(1, Ordering::AcqRel);
        self.usage
            .snapshots
            .fetch_sub(self.snapshot_bytes, Ordering::AcqRel);
        self.usage.ir.fetch_sub(self.ir_bytes, Ordering::AcqRel);
    }
}

impl BackgroundCompiler {
    pub fn new(
        compiler: Arc<dyn Compiler>,
        worker_count: usize,
        max_pending_jobs: usize,
    ) -> Result<Self, BackgroundCompilerError> {
        Self::new_with_resource_limits(
            compiler,
            worker_count,
            max_pending_jobs,
            Duration::from_secs(30),
            usize::MAX,
            usize::MAX,
        )
    }

    pub fn new_with_resource_limits(
        compiler: Arc<dyn Compiler>,
        worker_count: usize,
        max_pending_jobs: usize,
        compile_budget: Duration,
        max_snapshot_bytes: usize,
        max_ir_bytes: usize,
    ) -> Result<Self, BackgroundCompilerError> {
        if worker_count == 0 || max_pending_jobs == 0 {
            return Err(BackgroundCompilerError::InvalidLimit);
        }
        let (sender, receiver) = mpsc::sync_channel::<CompileRequest>(max_pending_jobs);
        let receiver = Arc::new(Mutex::new(receiver));
        let completion_slot = Arc::new(Mutex::new(None));
        let cancelled = Arc::new(AtomicBool::new(false));
        let usage = Arc::new(WorkerUsage::default());
        let overflow = Arc::new(Mutex::new(VecDeque::with_capacity(max_pending_jobs)));
        Ok(Self {
            sender: Some(sender),
            startup: Some(WorkerStartup {
                receiver,
                compiler,
                worker_count,
                compile_budget,
            }),
            workers: Vec::with_capacity(worker_count),
            completion_slot,
            cancelled,
            usage,
            max_snapshot_bytes,
            max_ir_bytes,
            overflow,
            worker_count,
        })
    }

    fn start_workers(&mut self) -> Result<(), BackgroundCompilerError> {
        let Some(startup) = self.startup.take() else {
            return Ok(());
        };
        for index in 0..startup.worker_count {
            let receiver = Arc::clone(&startup.receiver);
            let compiler = Arc::clone(&startup.compiler);
            let completion_slot: Arc<Mutex<Option<super::CompletionSender>>> =
                Arc::clone(&self.completion_slot);
            let cancelled = Arc::clone(&self.cancelled);
            let worker_budget = startup.compile_budget;
            let max_ir_bytes = self.max_ir_bytes;
            let usage = Arc::clone(&self.usage);
            let overflow = Arc::clone(&self.overflow);
            let worker = thread::Builder::new()
                .name(format!("rquickjs-jit-{index}"))
                .spawn(move || loop {
                    let request = { receiver.lock().unwrap_or_else(|p| p.into_inner()).recv() };
                    let Ok(request) = request else { break };
                    let snapshot_bytes = request.snapshot_bytes();
                    let ir_bytes = snapshot_bytes.saturating_mul(32);
                    let _usage = UsageGuard {
                        usage: Arc::clone(&usage),
                        snapshot_bytes,
                        ir_bytes,
                    };
                    let sender = completion_slot
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .clone();
                    if let Some(sender) = sender {
                        let key = request.key();
                        let tier = request.tier();
                        let artifact_key = request.artifact_key();
                        let attempt_id = request.attempt_id();
                        let control = crate::compiler::CompileControl::with_ir_limit(
                            Arc::clone(&cancelled),
                            worker_budget,
                            max_ir_bytes,
                        );
                        // One-shot scripts retire right after their only
                        // call; compiling them would only delay live jobs
                        // queued behind them.
                        let result = if request.is_retired() {
                            Err(crate::compiler::CompileFailure::Cancelled)
                        } else {
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                compiler.compile_controlled(request, &control)
                            }))
                            .unwrap_or(Err(crate::compiler::CompileFailure::CompilerPanicked))
                        };
                        let completion = super::CompileCompletion {
                            key,
                            requested_tier: tier,
                            artifact_key,
                            attempt_id,
                            result,
                        };
                        if let Err(super::CompletionSendError::Full(completion)) =
                            sender.try_send(completion)
                        {
                            overflow
                                .lock()
                                .unwrap_or_else(|p| p.into_inner())
                                .push_back(*completion);
                        }
                    }
                });
            match worker {
                Ok(worker) => self.workers.push(worker),
                Err(_) => {
                    self.sender.take();
                    for worker in self.workers.drain(..) {
                        let _ = worker.join();
                    }
                    return Err(BackgroundCompilerError::Shutdown);
                }
            }
        }
        Ok(())
    }

    pub fn dispatch_next(
        &mut self,
        coordinator: &mut Coordinator,
    ) -> Result<bool, BackgroundCompilerError> {
        self.drain_overflow(coordinator, super::DEFAULT_COMPLETION_DRAIN_BUDGET);
        if self.sender.is_none() {
            return Err(BackgroundCompilerError::Shutdown);
        }
        if self.usage.jobs.load(Ordering::Acquire) >= self.worker_count {
            return Ok(false);
        }
        *self
            .completion_slot
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            Some(coordinator.completion_sender());
        let Some(mut request) = coordinator.begin_next() else {
            return Ok(false);
        };
        if let Err(error) = self.start_workers() {
            coordinator.rollback_dispatch(request);
            return Err(error);
        }
        let sender = self
            .sender
            .as_ref()
            .ok_or(BackgroundCompilerError::Shutdown)?;
        let within_limits = |snapshot_bytes: usize| {
            self.usage
                .snapshots
                .load(Ordering::Acquire)
                .saturating_add(snapshot_bytes)
                <= self.max_snapshot_bytes
                && self
                    .usage
                    .ir
                    .load(Ordering::Acquire)
                    .saturating_add(snapshot_bytes.saturating_mul(32))
                    <= self.max_ir_bytes
        };
        if !within_limits(request.snapshot_bytes()) {
            // Inlining is optional. Retain ordinary direct-call publications
            // while releasing bodies before rejecting an affordable caller.
            request.discard_inline_snapshots();
        }
        let snapshot_bytes = request.snapshot_bytes();
        let ir_bytes = snapshot_bytes.saturating_mul(32);
        if !within_limits(snapshot_bytes) {
            coordinator.rollback_resource_limit(request);
            coordinator.record_resource_limit_rejection();
            return Ok(false);
        }
        self.usage.jobs.fetch_add(1, Ordering::AcqRel);
        self.usage
            .snapshots
            .fetch_add(snapshot_bytes, Ordering::AcqRel);
        self.usage.ir.fetch_add(ir_bytes, Ordering::AcqRel);
        self.usage
            .peak_compiler
            .fetch_max(snapshot_bytes.saturating_add(ir_bytes), Ordering::AcqRel);
        match sender.try_send(request) {
            Ok(()) => Ok(true),
            Err(mpsc::TrySendError::Full(request)) => {
                self.usage.jobs.fetch_sub(1, Ordering::AcqRel);
                self.usage
                    .snapshots
                    .fetch_sub(snapshot_bytes, Ordering::AcqRel);
                self.usage.ir.fetch_sub(ir_bytes, Ordering::AcqRel);
                coordinator.rollback_dispatch(request);
                Ok(false)
            }
            Err(mpsc::TrySendError::Disconnected(request)) => {
                self.usage.jobs.fetch_sub(1, Ordering::AcqRel);
                self.usage
                    .snapshots
                    .fetch_sub(snapshot_bytes, Ordering::AcqRel);
                self.usage.ir.fetch_sub(ir_bytes, Ordering::AcqRel);
                coordinator.rollback_dispatch(request);
                Err(BackgroundCompilerError::Shutdown)
            }
        }
    }

    pub fn drain_overflow(&mut self, coordinator: &mut Coordinator, budget: usize) -> usize {
        let mut drained = 0;
        while drained < budget {
            let completion = self
                .overflow
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pop_front();
            let Some(completion) = completion else { break };
            coordinator.complete(completion);
            drained += 1;
        }
        drained
    }

    pub fn live_usage(&self) -> (usize, usize, usize) {
        (
            self.usage.jobs.load(Ordering::Acquire),
            self.usage.snapshots.load(Ordering::Acquire),
            self.usage.ir.load(Ordering::Acquire),
        )
    }

    pub fn peak_compiler_bytes(&self) -> usize {
        self.usage.peak_compiler.load(Ordering::Acquire)
    }

    #[cfg(test)]
    fn started_worker_count(&self) -> usize {
        self.workers.len()
    }

    pub fn shutdown(&mut self, coordinator: &mut Coordinator) {
        self.cancelled.store(true, Ordering::Release);
        self.sender.take();
        self.startup.take();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
        self.drain_overflow(coordinator, usize::MAX);
        loop {
            let drain = coordinator.drain_completions();
            if drain.drained() == 0 {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bytecode::{opcode, CompileSnapshot, VerifyLimits},
        code_cache::CompiledArtifact,
        compiler::CompileFailure,
        runtime::{FunctionKey, Tier},
    };

    struct ImmediateCompiler;

    impl Compiler for ImmediateCompiler {
        fn compile(&self, request: CompileRequest) -> Result<CompiledArtifact, CompileFailure> {
            Ok(CompiledArtifact::fake(request.tier()))
        }
    }

    fn snapshot() -> crate::bytecode::VerifiedFunction {
        CompileSnapshot::from_untrusted_bytecode(vec![opcode::RETURN_UNDEF], 0, 0, 0, 0)
            .verify(VerifyLimits::default())
            .unwrap()
    }

    #[test]
    fn construction_does_not_start_workers_before_eligible_work_exists() {
        let mut background = BackgroundCompiler::new(Arc::new(ImmediateCompiler), 2, 1).unwrap();
        assert_eq!(background.started_worker_count(), 0);

        let mut coordinator = Coordinator::with_limits(1, 1, 1, 1);
        background.shutdown(&mut coordinator);
        assert_eq!(coordinator.metrics().queued, 0);
    }

    struct BlockingCompiler(std::sync::Mutex<std::sync::mpsc::Receiver<()>>);

    impl Compiler for BlockingCompiler {
        fn compile(&self, request: CompileRequest) -> Result<CompiledArtifact, CompileFailure> {
            let _ = self.0.lock().unwrap().recv();
            Ok(CompiledArtifact::fake(request.tier()))
        }
    }

    #[test]
    fn dispatch_keeps_the_backlog_in_the_coordinator_while_workers_are_busy() {
        let (release, blocked) = std::sync::mpsc::channel();
        let mut background = BackgroundCompiler::new(
            Arc::new(BlockingCompiler(std::sync::Mutex::new(blocked))),
            1,
            8,
        )
        .unwrap();
        let mut coordinator = Coordinator::with_limits(8, 8, 4, 1 << 20);
        for id in 1..=3 {
            coordinator
                .queue(FunctionKey::new(id, 1), Tier::Baseline, snapshot())
                .unwrap();
        }

        assert!(background.dispatch_next(&mut coordinator).unwrap());
        // The single worker is busy: later requests stay reorderable.
        assert!(!background.dispatch_next(&mut coordinator).unwrap());
        assert_eq!(coordinator.metrics().compiling, 1);

        for _ in 0..3 {
            release.send(()).unwrap();
        }
        background.shutdown(&mut coordinator);
    }

    #[test]
    fn first_queued_request_starts_the_configured_workers() {
        let mut background = BackgroundCompiler::new(Arc::new(ImmediateCompiler), 2, 1).unwrap();
        let mut coordinator = Coordinator::with_limits(1, 1, 1, 1);
        coordinator
            .queue(FunctionKey::new(1, 1), Tier::Baseline, snapshot())
            .unwrap();

        assert!(background.dispatch_next(&mut coordinator).unwrap());
        assert_eq!(background.started_worker_count(), 2);
        background.shutdown(&mut coordinator);
    }
}
