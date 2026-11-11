use std::{
    collections::{HashMap, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
};

#[cfg(feature = "test-support")]
use std::sync::Mutex;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{
    bytecode::VerifiedFunction,
    code_cache::{ArtifactKey, ArtifactVersionIdentity, CodeCache, CompiledArtifact, ExecutionPin},
    compiler::CompileFailure,
    JitMetrics,
};

use super::{
    install, invalidate, BoundedSpecializationSignature, CallSpecializationKey, DependencyGraph,
    DependencyKey, FeedbackRepresentation, FeedbackSnapshot, ObservedType,
};

fn call_specialization_fingerprint(key: &CallSpecializationKey) -> u64 {
    fn mix(state: u64, value: u64) -> u64 {
        (state ^ value).wrapping_mul(0x100_0000_01b3)
    }
    fn representation_tag(representation: FeedbackRepresentation) -> u64 {
        match representation {
            FeedbackRepresentation::Int32 => 1,
            FeedbackRepresentation::Float64 => 2,
            FeedbackRepresentation::HeapRef => 3,
            FeedbackRepresentation::Bool => 4,
        }
    }
    let mut state = 0xcbf2_9ce4_8422_2325;
    state = mix(state, key.caller().id);
    state = mix(state, key.caller().generation);
    state = mix(state, key.callee().id);
    state = mix(state, key.callee().generation);
    state = mix(state, key.callee_identity());
    state = mix(state, key.callee_bytecode_identity());
    state = mix(state, key.arity() as u64);
    for argument in key.arguments() {
        state = mix(state, representation_tag(*argument));
    }
    state = mix(state, representation_tag(key.result()));
    mix(state, key.feedback_epoch())
}

pub fn compile_and_send<C: crate::compiler::Compiler + ?Sized>(
    compiler: &C,
    request: CompileRequest,
    sender: &CompletionSender,
) -> Result<(), Box<CompileCompletion>> {
    let key = request.key();
    let requested_tier = request.tier();
    let artifact_key = request.artifact_key();
    let attempt_id = request.attempt_id();
    let result =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compiler.compile(request)))
            .unwrap_or(Err(CompileFailure::CompilerPanicked));
    sender.send(CompileCompletion {
        key,
        requested_tier,
        artifact_key,
        attempt_id,
        result,
    })
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct FunctionKey {
    pub id: u64,
    pub generation: u64,
}

/// A version-pinned callee selected for a monomorphic compiled call edge.
///
/// Holding this value keeps the callee's code alive across concurrent
/// invalidation. New resolutions still fail as soon as the generation is
/// retired, so stale callers cannot acquire another target.
#[derive(Debug)]
pub struct CompiledCallTarget {
    identity: ArtifactVersionIdentity,
    call: CallSpecializationKey,
    pin: ExecutionPin,
}

/// Executable scalar entry retained by a caller compilation request.  The
/// cloned publication owns the executable allocation, closing the race where
/// a callee is retired while its caller is still compiling.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[derive(Clone, Debug)]
pub struct DirectCallTarget {
    pc: u32,
    call: CallSpecializationKey,
    signature: BoundedSpecializationSignature,
    published: crate::compiler::baseline::PublishedBaselineCode,
    artifact_key: ArtifactKey,
    inline_snapshot: Option<crate::bytecode::CompileSnapshot>,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
impl DirectCallTarget {
    pub const fn artifact_key(&self) -> ArtifactKey {
        self.artifact_key
    }
    pub fn inline_snapshot(&self) -> Option<&crate::bytecode::CompileSnapshot> {
        self.inline_snapshot.as_ref()
    }
    pub const fn pc(&self) -> u32 {
        self.pc
    }
    pub const fn call(&self) -> &CallSpecializationKey {
        &self.call
    }
    pub const fn signature(&self) -> &BoundedSpecializationSignature {
        &self.signature
    }
    pub fn entry(&self) -> *const u8 {
        self.published.as_ptr()
    }
    pub(crate) fn publication(&self) -> crate::compiler::baseline::PublishedBaselineCode {
        self.published.clone()
    }
}

/// A monomorphic call edge whose callee artifact publishes a P3a native call
/// entry. The caller guards the callee identity and proves its global self
/// binding at run time; the publication pin keeps the entry executable.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[derive(Clone, Debug)]
pub struct NativeCallTarget {
    link: super::CallLinkStatus,
    plan: crate::compiler::native_call::NativeCallPlan,
    published: crate::compiler::baseline::PublishedBaselineCode,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
impl NativeCallTarget {
    pub const fn link(&self) -> &super::CallLinkStatus {
        &self.link
    }
    pub const fn plan(&self) -> &crate::compiler::native_call::NativeCallPlan {
        &self.plan
    }
    pub fn entry(&self) -> *const u8 {
        self.published.as_ptr()
    }
    pub(crate) fn publication(&self) -> crate::compiler::baseline::PublishedBaselineCode {
        self.published.clone()
    }
}

/// Copied callee body retained for tagged frame inlining, independently of a
/// scalar direct-call entry. Target guards and dependency registration remain
/// the consuming compiler's responsibility.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[derive(Clone, Debug)]
pub struct FrameInlineTarget {
    target: super::CallLinkStatus,
    artifact_key: ArtifactKey,
    snapshot: crate::bytecode::CompileSnapshot,
    children: Arc<[FrameInlineTarget]>,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
impl FrameInlineTarget {
    pub const fn target(&self) -> &super::CallLinkStatus {
        &self.target
    }
    pub const fn artifact_key(&self) -> ArtifactKey {
        self.artifact_key
    }
    pub const fn snapshot(&self) -> &crate::bytecode::CompileSnapshot {
        &self.snapshot
    }
    pub const fn pc(&self) -> u32 {
        self.target.pc()
    }
    pub fn children(&self) -> &[FrameInlineTarget] {
        &self.children
    }
    /// Pre-order traversal including this target. The iterator owns only a
    /// bounded traversal stack; snapshots remain shared by the request tree.
    pub fn descendants(&self) -> impl Iterator<Item = &FrameInlineTarget> {
        FrameInlineTargets {
            pending: vec![self],
        }
    }
    pub fn dependencies(&self) -> impl Iterator<Item = FunctionKey> + '_ {
        self.descendants().map(|target| target.target.callee())
    }
    pub fn retained_bytes(&self) -> usize {
        self.descendants().fold(0usize, |bytes, target| {
            bytes.saturating_add(target.snapshot.retained_bytes())
        })
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
struct FrameInlineTargets<'a> {
    pending: Vec<&'a FrameInlineTarget>,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
impl<'a> Iterator for FrameInlineTargets<'a> {
    type Item = &'a FrameInlineTarget;

    fn next(&mut self) -> Option<Self::Item> {
        let target = self.pending.pop()?;
        self.pending.extend(target.children.iter().rev());
        Some(target)
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const FRAME_INLINE_MAX_DEPTH: usize = 4;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const FRAME_INLINE_MAX_SITES: usize = 16;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const FRAME_INLINE_MAX_INSTRUCTIONS: usize = 256;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const FRAME_INLINE_MAX_SLOTS: usize = 128;
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
const FRAME_INLINE_MAX_RETAINED_BYTES: usize = 64 * 1024;

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
#[derive(Debug)]
struct FrameInlineBudget {
    // Candidate sites are charged before resolution/verification so rejected
    // targets cannot turn the successful-retention limit into unbounded work.
    sites_left: usize,
    instructions_left: usize,
    slots_left: usize,
    bytes_left: usize,
    cancelled: bool,
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
impl FrameInlineBudget {
    fn new(bytes_left: usize) -> Self {
        Self {
            sites_left: FRAME_INLINE_MAX_SITES,
            instructions_left: FRAME_INLINE_MAX_INSTRUCTIONS,
            slots_left: FRAME_INLINE_MAX_SLOTS,
            bytes_left: bytes_left.min(FRAME_INLINE_MAX_RETAINED_BYTES),
            cancelled: false,
        }
    }

    fn exhausted(&self) -> bool {
        self.cancelled
            || self.sites_left == 0
            || self.instructions_left == 0
            || self.bytes_left == 0
    }

    fn cancel(&mut self) {
        self.cancelled = true;
    }

    fn begin_candidate(&mut self) -> bool {
        if self.exhausted() {
            self.cancel();
            return false;
        }
        self.sites_left -= 1;
        true
    }

    fn verification_limits(
        &mut self,
        snapshot: &crate::bytecode::CompileSnapshot,
    ) -> Option<crate::bytecode::VerifyLimits> {
        let slots = usize::from(snapshot.arg_count())
            .checked_add(usize::from(snapshot.local_count()))
            .and_then(|slots| slots.checked_add(usize::from(snapshot.stack_size())));
        let Some(slots) = slots else {
            self.cancel();
            return None;
        };
        if slots > self.slots_left || snapshot.retained_bytes() > self.bytes_left {
            self.cancel();
            return None;
        }

        let mut limits = crate::bytecode::VerifyLimits::default();
        limits.max_snapshot_bytes = limits.max_snapshot_bytes.min(self.bytes_left);
        limits.max_instructions = limits.max_instructions.min(self.instructions_left);
        limits.max_metadata_bytes = limits.max_metadata_bytes.min(self.bytes_left);
        // The verifier charges initial locals once. A block is processed once
        // initially and at most once per local widening; stack merges either
        // agree or fail. Each processing charges instruction state, optional
        // before/after metadata, the block entry, and at most two successors.
        // Blocks are non-empty, so 1 + 6*slots per instruction and slots+1
        // visits bound all of those charges. The verifier's default remains
        // the outer cap because retained artifacts already passed that limit.
        let visits_per_block = self.slots_left.saturating_add(1);
        let work_per_instruction = self.slots_left.saturating_mul(6).saturating_add(1);
        let work_limit = self.slots_left.saturating_add(
            self.instructions_left
                .saturating_mul(visits_per_block)
                .saturating_mul(work_per_instruction),
        );
        limits.max_work_units = limits.max_work_units.min(work_limit);
        Some(limits)
    }

    fn verify(
        &mut self,
        snapshot: &crate::bytecode::CompileSnapshot,
    ) -> Option<crate::bytecode::VerifiedFunction> {
        let limits = self.verification_limits(snapshot)?;
        match snapshot.clone().verify(limits) {
            Ok(body) => Some(body),
            Err(_) => {
                // A bounded verifier rejection may already have consumed its
                // whole allowance. Stop the search instead of paying it again
                // for later or recursively discovered candidates.
                self.cancel();
                None
            }
        }
    }

    fn claim(&mut self, body: &crate::bytecode::VerifiedFunction) -> bool {
        let instructions = body.instructions().len();
        let snapshot = body.snapshot();
        let slots = usize::from(snapshot.arg_count())
            .saturating_add(usize::from(snapshot.local_count()))
            .saturating_add(usize::from(snapshot.stack_size()));
        let bytes = snapshot.retained_bytes();
        let Some(instructions_left) = self.instructions_left.checked_sub(instructions) else {
            self.cancel();
            return false;
        };
        let Some(slots_left) = self.slots_left.checked_sub(slots) else {
            self.cancel();
            return false;
        };
        let Some(bytes_left) = self.bytes_left.checked_sub(bytes) else {
            self.cancel();
            return false;
        };
        self.instructions_left = instructions_left;
        self.slots_left = slots_left;
        self.bytes_left = bytes_left;
        true
    }
}

#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
fn artifact_matches_direct_call(artifact: &CompiledArtifact, call: &CallSpecializationKey) -> bool {
    artifact.direct_call_published().is_some()
        && artifact
            .optimized_metadata()
            .and_then(|metadata| metadata.direct_call_signature())
            .is_some_and(|signature| {
                signature.function() == call.callee()
                    && signature.arguments() == call.arguments()
                    && signature.result() == call.result()
            })
}

/// Conservatively predicts whether a Tier 2 caller can take the linked direct
/// entry at `pc`. `emit_opt_specialized_call` uses it only for a receiver-free
/// call whose function operand and every HeapRef argument are frame reads
/// (`get_arg*` / `get_loc*` / `get_loc_check`). Any other site (callee by
/// global name, closure variable, method receiver, or a HeapRef argument from a
/// global or an expression) falls back to the generic CALL there, so the
/// planner must keep frame inlining it.
///
/// The prediction replays only the call's own basic block: values from
/// earlier blocks, rearranged by stack shuffles, or read before any later
/// frame write or nested call are unknown. It may under-approximate (the site
/// then stays frame-inlined, the pre-linking behavior) but never admits a site
/// whose operands the block does not visibly read from frame slots.
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
pub(crate) fn tier2_direct_call_site_usable(
    body: &VerifiedFunction,
    pc: u32,
    call: &CallSpecializationKey,
) -> bool {
    let instructions = body.instructions();
    let Some(index) = instructions
        .iter()
        .position(|instruction| instruction.pc() == pc)
    else {
        return false;
    };
    let instruction = &instructions[index];
    if !matches!(
        instruction.opcode().name(),
        "call" | "call0" | "call1" | "call2" | "call3"
    ) {
        return false;
    }
    let argc = crate::bytecode::effective_pop(instruction).saturating_sub(1);
    if argc != call.arguments().len() {
        return false;
    }
    let Some(block) = body
        .control_flow_graph()
        .blocks()
        .iter()
        .find(|block| block.instruction_range().contains(&index))
    else {
        return false;
    };
    // `true` marks a value pushed by a frame read that is still current.
    let mut stack: Vec<bool> = Vec::new();
    for instruction in &instructions[block.instruction_range().start..index] {
        let name = instruction.opcode().name();
        let frame_read = name != "get_loc0_loc1"
            && (name.starts_with("get_arg")
                || name.starts_with("get_loc")
                    && (name == "get_loc_check"
                        || name
                            .strip_prefix("get_loc")
                            .is_some_and(|rest| rest.bytes().all(|byte| byte.is_ascii_digit()))));
        if frame_read {
            stack.push(true);
            continue;
        }
        // A frame write, closure-variable write, or nested call may change a
        // slot after it was read; forget every earlier frame read.
        if name.contains("_loc")
            || name.contains("_arg")
            || name.contains("var_ref")
            || name.starts_with("call")
            || name.starts_with("tail_call")
            || name.starts_with("apply")
            || name.starts_with("eval")
        {
            stack.iter_mut().for_each(|known| *known = false);
        }
        let pop = crate::bytecode::effective_pop(instruction);
        stack.truncate(stack.len().saturating_sub(pop));
        stack.extend(std::iter::repeat_n(
            false,
            usize::from(instruction.opcode().n_push()),
        ));
    }
    let Some(function_index) = stack.len().checked_sub(argc + 1) else {
        return false;
    };
    stack[function_index]
        && call
            .arguments()
            .iter()
            .enumerate()
            .all(|(argument, representation)| {
                *representation != FeedbackRepresentation::HeapRef
                    || stack[function_index + 1 + argument]
            })
}

impl CompiledCallTarget {
    pub const fn identity(&self) -> ArtifactVersionIdentity {
        self.identity
    }

    pub const fn call(&self) -> &CallSpecializationKey {
        &self.call
    }

    pub fn artifact(&self) -> &CompiledArtifact {
        self.pin.artifact()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompiledCallTargetError {
    InvalidSpecialization(QueueError),
    StaleCallee,
    CalleeNotInstalled,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GuardId(u32);

impl GuardId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }
    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SidePathProfile {
    function: FunctionKey,
    guard: GuardId,
    pc: u32,
    observed: ObservedType,
    feedback_epoch: u64,
}

impl SidePathProfile {
    pub const fn new(
        function: FunctionKey,
        guard: GuardId,
        pc: u32,
        observed: ObservedType,
        feedback_epoch: u64,
    ) -> Self {
        Self {
            function,
            guard,
            pc,
            observed,
            feedback_epoch,
        }
    }
    pub const fn function(self) -> FunctionKey {
        self.function
    }
    pub const fn guard(self) -> GuardId {
        self.guard
    }
    pub const fn pc(self) -> u32 {
        self.pc
    }
    pub const fn observed(self) -> ObservedType {
        self.observed
    }
    pub const fn feedback_epoch(self) -> u64 {
        self.feedback_epoch
    }
    fn fingerprint(self) -> u64 {
        let observed = match self.observed {
            ObservedType::Int32 => 1,
            ObservedType::Float64 => 2,
            ObservedType::Bool => 3,
            ObservedType::Null => 4,
            ObservedType::Undefined => 5,
            ObservedType::String => 6,
            ObservedType::Object => 7,
            ObservedType::Function(key) => 8 ^ key.id ^ key.generation.rotate_left(17),
            ObservedType::BigInt => 9,
            ObservedType::Symbol => 10,
        };
        self.function.id
            ^ self.function.generation.rotate_left(7)
            ^ u64::from(self.guard.0).rotate_left(13)
            ^ u64::from(self.pc).rotate_left(29)
            ^ observed
            ^ self.feedback_epoch.rotate_left(41)
    }

    fn signature_fingerprint(self) -> u64 {
        let mut signature = self;
        signature.feedback_epoch = 0;
        signature.fingerprint()
    }
}

impl FunctionKey {
    pub const fn new(id: u64, generation: u64) -> Self {
        Self { id, generation }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Tier {
    Baseline,
    Optimizing,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactEnvironment {
    pub runtime_id: u64,
    pub target_isa: u64,
    pub cpu_features: u64,
    pub abi_fingerprint: u64,
    pub config_fingerprint: u64,
}

impl Default for ArtifactEnvironment {
    fn default() -> Self {
        Self {
            runtime_id: 1,
            target_isa: 0,
            cpu_features: 0,
            abi_fingerprint: 0,
            config_fingerprint: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompileState {
    Cold,
    Queued(Tier),
    Compiling(Tier),
    Ready(Tier),
    Installed(Tier),
    Backoff { attempts: u8, retry_after: u64 },
    Blacklisted,
    Retired,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AttemptId(u64);

impl AttemptId {
    pub const fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug)]
pub struct CompileRequest {
    key: FunctionKey,
    tier: Tier,
    snapshot: VerifiedFunction,
    artifact_key: ArtifactKey,
    attempt_id: AttemptId,
    feedback_epoch: u64,
    feedback: Arc<FeedbackSnapshot>,
    side_path_profile: Option<SidePathProfile>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    direct_call_targets: Arc<[DirectCallTarget]>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    frame_inline_targets: Arc<[FrameInlineTarget]>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    native_call_targets: Arc<[NativeCallTarget]>,
    /// Set when the function generation retires after dispatch, so a worker
    /// skips the compile instead of spending the bounded worker queue on it.
    retired: Arc<AtomicBool>,
}

impl CompileRequest {
    /// Whether the requested generation retired after this request was
    /// dispatched. Its completion can never be installed.
    pub fn is_retired(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }

    pub(super) fn discard_inline_snapshots(&mut self) {
        #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
        {
            self.frame_inline_targets = Arc::from([]);
        }
        #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
        if self
            .direct_call_targets
            .iter()
            .any(|target| target.inline_snapshot.is_some())
        {
            for target in Arc::make_mut(&mut self.direct_call_targets) {
                target.inline_snapshot = None;
            }
        }
    }

    /// Budget charge for the caller snapshot and every retained callee body.
    /// Shared callee storage is conservatively charged once per call site.
    pub fn snapshot_bytes(&self) -> usize {
        let bytes = self
            .snapshot
            .snapshot()
            .owned_bytes()
            .saturating_add(self.feedback.array_feedback_bytes());
        #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
        let bytes = self
            .direct_call_targets
            .iter()
            .fold(bytes, |total, target| {
                total.saturating_add(
                    target
                        .inline_snapshot()
                        .map_or(0, crate::bytecode::CompileSnapshot::retained_bytes),
                )
            });
        #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
        let bytes = self
            .frame_inline_targets
            .iter()
            .fold(bytes, |total, target| {
                total.saturating_add(target.retained_bytes())
            });
        bytes
    }
    pub const fn key(&self) -> FunctionKey {
        self.key
    }

    pub const fn tier(&self) -> Tier {
        self.tier
    }

    pub const fn artifact_key(&self) -> ArtifactKey {
        self.artifact_key
    }

    pub const fn snapshot(&self) -> &VerifiedFunction {
        &self.snapshot
    }

    pub const fn attempt_id(&self) -> AttemptId {
        self.attempt_id
    }

    pub const fn feedback_epoch(&self) -> u64 {
        self.feedback_epoch
    }

    pub fn feedback(&self) -> &FeedbackSnapshot {
        &self.feedback
    }
    pub const fn side_path_profile(&self) -> Option<SidePathProfile> {
        self.side_path_profile
    }
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn direct_call_target(&self, pc: u32) -> Option<&DirectCallTarget> {
        self.direct_call_targets.iter().find(|target| {
            self.feedback.call_specialization_at(self.key, pc).as_ref() == Some(target.call())
        })
    }
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub(crate) fn direct_call_targets(&self) -> &[DirectCallTarget] {
        &self.direct_call_targets
    }
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn frame_inline_targets(&self) -> &[FrameInlineTarget] {
        &self.frame_inline_targets
    }
    /// The native-call target of a monomorphic call site, if its callee
    /// published a P3a native entry when this request was queued.
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn native_call_target(&self, pc: u32) -> Option<&NativeCallTarget> {
        let status = self.feedback.call_link_at(self.key, pc)?;
        self.native_call_targets
            .iter()
            .find(|target| target.link == status)
    }
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn frame_inline_target(&self, pc: u32) -> Option<&FrameInlineTarget> {
        let status = self.feedback.call_link_at(self.key, pc)?;
        self.frame_inline_targets
            .iter()
            .find(|target| target.target == status)
    }
}

#[derive(Debug)]
pub struct CompileCompletion {
    pub key: FunctionKey,
    pub requested_tier: Tier,
    pub artifact_key: ArtifactKey,
    pub attempt_id: AttemptId,
    pub result: Result<CompiledArtifact, CompileFailure>,
}

/// Test-only record of the exact boundary at which a compiler completion was
/// accepted or rejected. This deliberately stays out of production metrics:
/// it is keyed by VM-assigned function identity and exists only to make
/// asynchronous publication failures deterministic in integration tests.
#[cfg(feature = "test-support")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionDisposition {
    Installed,
    MissingInFlight,
    StaleEnvelope,
    FeedbackEpochMismatch {
        expected: u64,
        artifact: u64,
        latest: Option<u64>,
    },
    DependencyGenerationMismatch {
        dependency: FunctionKey,
        current_generation: Option<u64>,
    },
    DependencyInstallFailed,
    InstallFailed,
    ArtifactKeyMismatch,
    CompileFailed(CompileFailure),
}

#[derive(Debug)]
pub enum CompletionSendError {
    Full(Box<CompileCompletion>),
    Closed(Box<CompileCompletion>),
}

#[derive(Clone, Debug)]
pub struct CompletionSender {
    sender: Option<SyncSender<CompileCompletion>>,
    signals: Arc<CompletionQueueSignals>,
}

#[derive(Debug, Default)]
struct CompletionQueueSignals {
    pending: AtomicUsize,
    saturated: AtomicU64,
}

impl CompletionQueueSignals {
    // `Atomic::try_update` would avoid this deprecation, but it is unavailable
    // on the crate's Rust 1.87 MSRV.
    #[allow(deprecated)]
    fn increment_pending(&self) {
        let _ = self
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                Some(pending.saturating_add(1))
            });
    }

    // See `increment_pending` for the MSRV compatibility rationale.
    #[allow(deprecated)]
    fn decrement_pending(&self) {
        let _ = self
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |pending| {
                Some(pending.saturating_sub(1))
            });
    }

    // See `increment_pending` for the MSRV compatibility rationale.
    #[allow(deprecated)]
    fn record_saturation(&self) {
        let _ = self
            .saturated
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                Some(count.saturating_add(1))
            });
    }
}

impl CompletionSender {
    pub fn try_send(&self, completion: CompileCompletion) -> Result<(), CompletionSendError> {
        let Some(sender) = &self.sender else {
            return Err(CompletionSendError::Closed(Box::new(completion)));
        };
        self.signals.increment_pending();
        sender.try_send(completion).map_err(|error| match error {
            TrySendError::Full(completion) => {
                self.signals.decrement_pending();
                self.signals.record_saturation();
                CompletionSendError::Full(Box::new(completion))
            }
            TrySendError::Disconnected(completion) => {
                self.signals.decrement_pending();
                CompletionSendError::Closed(Box::new(completion))
            }
        })
    }

    pub fn send(&self, completion: CompileCompletion) -> Result<(), Box<CompileCompletion>> {
        let Some(sender) = &self.sender else {
            return Err(Box::new(completion));
        };
        self.signals.increment_pending();
        sender.send(completion).map_err(|error| {
            self.signals.decrement_pending();
            Box::new(error.0)
        })
    }
}

pub const DEFAULT_COMPLETION_DRAIN_BUDGET: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompletionDrain {
    drained: usize,
    reclaimed: usize,
    may_have_remaining: bool,
}

impl CompletionDrain {
    pub const fn drained(self) -> usize {
        self.drained
    }

    pub const fn reclaimed(self) -> usize {
        self.reclaimed
    }

    /// Conservatively reports whether completion or reclamation work may remain.
    pub const fn may_have_remaining(self) -> bool {
        self.may_have_remaining
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueError {
    Full,
    NotReady,
    Retired,
    Blacklisted,
    Shutdown,
    SnapshotIdentity,
    AttemptIdsExhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct InFlight {
    attempt_id: AttemptId,
    artifact_key: ArtifactKey,
    tier: Tier,
    feedback_epoch: u64,
    side_path: bool,
}

#[derive(Debug)]
struct TierRecord {
    state: CompileState,
    attempts: u8,
}

impl Default for TierRecord {
    fn default() -> Self {
        Self {
            state: CompileState::Cold,
            attempts: 0,
        }
    }
}

#[derive(Debug, Default)]
struct FunctionState {
    baseline: TierRecord,
    optimizing: TierRecord,
    published: Option<Tier>,
    retired: bool,
    instability_attempts: u8,
}

impl FunctionState {
    fn tier(&self, tier: Tier) -> &TierRecord {
        match tier {
            Tier::Baseline => &self.baseline,
            Tier::Optimizing => &self.optimizing,
        }
    }

    fn tier_mut(&mut self, tier: Tier) -> &mut TierRecord {
        match tier {
            Tier::Baseline => &mut self.baseline,
            Tier::Optimizing => &mut self.optimizing,
        }
    }

    fn active_state(&self) -> Option<CompileState> {
        [self.baseline.state, self.optimizing.state]
            .into_iter()
            .find(|state| {
                matches!(
                    state,
                    CompileState::Queued(_) | CompileState::Compiling(_) | CompileState::Ready(_)
                )
            })
    }

    fn visible_state(&self) -> CompileState {
        if self.retired {
            return CompileState::Retired;
        }
        if let Some(active) = self.active_state() {
            return active;
        }
        if let Some(published) = self.published {
            return CompileState::Installed(published);
        }
        [self.baseline.state, self.optimizing.state]
            .into_iter()
            .find(|state| {
                matches!(
                    state,
                    CompileState::Backoff { .. } | CompileState::Blacklisted
                )
            })
            .unwrap_or(CompileState::Cold)
    }
}

/// Tracks mutations to compilation counters. Native-entry counters bypass the
/// dirty bit because publication refreshes those fields on every native exit.
#[derive(Debug)]
struct CoordinatorMetrics {
    value: JitMetrics,
    dirty: bool,
}

impl std::ops::Deref for CoordinatorMetrics {
    type Target = JitMetrics;
    fn deref(&self) -> &Self::Target {
        &self.value
    }
}

impl std::ops::DerefMut for CoordinatorMetrics {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // All normal field updates invalidate publication, including future
        // counters added to the coordinator.
        self.dirty = true;
        &mut self.value
    }
}

#[derive(Debug)]
pub struct Coordinator {
    max_queue_len: usize,
    max_attempts: u8,
    clock: u64,
    queue: VecDeque<CompileRequest>,
    // Keys are VM-assigned identities, not guest-controlled strings.
    functions: FxHashMap<FunctionKey, FunctionState>,
    current_generations: HashMap<u64, u64>,
    in_flight: HashMap<FunctionKey, InFlight>,
    next_attempt_id: u64,
    metrics: CoordinatorMetrics,
    completion_sender: Option<SyncSender<CompileCompletion>>,
    completion_receiver: Option<Receiver<CompileCompletion>>,
    completion_signals: Arc<CompletionQueueSignals>,
    shutdown: bool,
    cache: CodeCache,
    installed_keys: HashMap<(FunctionKey, Tier), ArtifactKey>,
    // Non-owning lookup cache. Every installed-key mutation clears it, so
    // repeated native exits can credit the same exact artifact without hashing
    // its function and tier again. Cache residency is still checked below.
    last_benefit_target: Option<(FunctionKey, Tier, ArtifactKey)>,
    environment: ArtifactEnvironment,
    dependencies: DependencyGraph,
    latest_feedback_epochs: HashMap<FunctionKey, u64>,
    /// Retirement flags shared with dispatched requests (see
    /// `CompileRequest::is_retired`).
    in_flight_retired: FxHashMap<FunctionKey, Arc<AtomicBool>>,
    side_exits: HashMap<FunctionKey, HashMap<u32, u8>>,
    side_exit_observations: HashMap<(FunctionKey, u32), Option<ObservedType>>,
    specialization_versions: HashMap<(FunctionKey, u64), u8>,
    call_specialization_versions: HashMap<FunctionKey, HashMap<u64, u8>>,
    profitability_demotions: FxHashSet<FunctionKey>,
    #[cfg(feature = "test-support")]
    completion_dispositions: Arc<Mutex<HashMap<FunctionKey, CompletionDisposition>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SideExitAction {
    Counted,
    StablePathThreshold,
    Demote { retry_after: u64 },
}

impl Coordinator {
    pub fn with_limits(
        max_queue_len: usize,
        max_completion_len: usize,
        max_attempts: u8,
        max_code_bytes: usize,
    ) -> Self {
        Self::with_environment(
            max_queue_len,
            max_completion_len,
            max_attempts,
            max_code_bytes,
            ArtifactEnvironment::default(),
        )
    }

    pub fn with_environment(
        max_queue_len: usize,
        max_completion_len: usize,
        max_attempts: u8,
        max_code_bytes: usize,
        environment: ArtifactEnvironment,
    ) -> Self {
        Self::with_cache(
            max_queue_len,
            max_completion_len,
            max_attempts,
            CodeCache::new(max_code_bytes),
            environment,
        )
    }

    pub fn with_environment_and_metadata_limit(
        max_queue_len: usize,
        max_completion_len: usize,
        max_attempts: u8,
        max_code_bytes: usize,
        max_metadata_bytes: usize,
        environment: ArtifactEnvironment,
    ) -> Self {
        Self::with_cache(
            max_queue_len,
            max_completion_len,
            max_attempts,
            CodeCache::new_with_separate_limits(max_code_bytes, max_metadata_bytes),
            environment,
        )
    }

    fn with_cache(
        max_queue_len: usize,
        max_completion_len: usize,
        max_attempts: u8,
        cache: CodeCache,
        environment: ArtifactEnvironment,
    ) -> Self {
        let (completion_sender, completion_receiver) = mpsc::sync_channel(max_completion_len);
        let completion_signals = Arc::new(CompletionQueueSignals::default());
        Self {
            max_queue_len,
            max_attempts,
            clock: 0,
            queue: VecDeque::new(),
            functions: FxHashMap::default(),
            current_generations: HashMap::new(),
            in_flight: HashMap::new(),
            next_attempt_id: 0,
            metrics: CoordinatorMetrics {
                value: JitMetrics::disabled(),
                dirty: true,
            },
            completion_sender: Some(completion_sender),
            completion_receiver: Some(completion_receiver),
            completion_signals,
            shutdown: false,
            cache,
            installed_keys: HashMap::new(),
            last_benefit_target: None,
            environment,
            dependencies: DependencyGraph::default(),
            latest_feedback_epochs: HashMap::new(),
            in_flight_retired: FxHashMap::default(),
            side_exits: HashMap::new(),
            side_exit_observations: HashMap::new(),
            specialization_versions: HashMap::new(),
            call_specialization_versions: HashMap::new(),
            profitability_demotions: FxHashSet::default(),
            #[cfg(feature = "test-support")]
            completion_dispositions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn test_completion_disposition(&self, key: FunctionKey) -> Option<CompletionDisposition> {
        self.completion_dispositions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&key)
            .copied()
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn test_completion_dispositions(
        &self,
    ) -> Arc<Mutex<HashMap<FunctionKey, CompletionDisposition>>> {
        Arc::clone(&self.completion_dispositions)
    }

    #[cfg(feature = "test-support")]
    fn record_completion_disposition(
        &mut self,
        key: FunctionKey,
        disposition: CompletionDisposition,
    ) {
        self.completion_dispositions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(key, disposition);
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub(crate) fn initialize_environment(&mut self, environment: ArtifactEnvironment) {
        debug_assert_eq!(self.metrics.queued, 0);
        debug_assert_eq!(self.metrics.compiling, 0);
        debug_assert_eq!(self.metrics.installed, 0);
        debug_assert!(self.queue.is_empty());
        debug_assert!(self.in_flight.is_empty());
        self.environment = environment;
    }

    /// Registers bounded version identity without claiming a direct-call path.
    pub fn register_call_specialization(
        &mut self,
        primary: &BoundedSpecializationSignature,
        call: &CallSpecializationKey,
    ) -> Result<ArtifactVersionIdentity, QueueError> {
        if primary.function() != call.caller()
            || primary.feedback_epoch() == 0
            || primary.feedback_epoch() != call.feedback_epoch()
        {
            return Err(QueueError::SnapshotIdentity);
        }
        let identity = ArtifactVersionIdentity::new(
            primary.fingerprint(),
            call_specialization_fingerprint(call),
        );
        let versions = self
            .call_specialization_versions
            .entry(call.caller())
            .or_default();
        if let Some(attempts) = versions.get_mut(&identity.fingerprint()) {
            if *attempts >= self.max_attempts {
                return Err(QueueError::Blacklisted);
            }
            *attempts = attempts.saturating_add(1);
            return Ok(identity);
        }
        if versions.len() >= usize::from(self.max_attempts) {
            return Err(QueueError::Blacklisted);
        }
        versions.insert(identity.fingerprint(), 1);
        Ok(identity)
    }

    /// Resolves a stable call specialization to a pinned optimizing artifact.
    ///
    /// This is the safety boundary needed by compiled-to-compiled lowering:
    /// identity validation and bounded versioning happen before the callee is
    /// looked up, and the returned execution pin prevents reclamation while a
    /// caller is using the target. The current compiler still uses the generic
    /// CALL helper until its native frame ABI can consume this target.
    pub fn resolve_compiled_call_target(
        &mut self,
        primary: &BoundedSpecializationSignature,
        call: &CallSpecializationKey,
    ) -> Result<CompiledCallTarget, CompiledCallTargetError> {
        if primary.function() != call.caller()
            || primary.feedback_epoch() == 0
            || primary.feedback_epoch() != call.feedback_epoch()
        {
            return Err(CompiledCallTargetError::InvalidSpecialization(
                QueueError::SnapshotIdentity,
            ));
        }
        if self.current_generations.get(&call.callee().id) != Some(&call.callee().generation)
            || self
                .functions
                .get(&call.callee())
                .is_some_and(|function| function.retired)
        {
            return Err(CompiledCallTargetError::StaleCallee);
        }
        let pin = self
            .pin(call.callee(), Tier::Optimizing)
            .ok_or(CompiledCallTargetError::CalleeNotInstalled)?;
        // Missing or stale callees are availability failures, not unstable
        // specialization attempts, so consume the bounded version budget only
        // once an executable target has actually been acquired.
        let identity = self
            .register_call_specialization(primary, call)
            .map_err(CompiledCallTargetError::InvalidSpecialization)?;
        Ok(CompiledCallTarget {
            identity,
            call: call.clone(),
            pin,
        })
    }

    pub fn queue(
        &mut self,
        key: FunctionKey,
        tier: Tier,
        snapshot: VerifiedFunction,
    ) -> Result<(), QueueError> {
        self.queue_with_feedback(key, tier, snapshot, FeedbackSnapshot::empty(self.clock))
    }

    pub fn queue_with_feedback(
        &mut self,
        key: FunctionKey,
        tier: Tier,
        snapshot: VerifiedFunction,
        feedback: FeedbackSnapshot,
    ) -> Result<(), QueueError> {
        self.queue_request(key, tier, snapshot, feedback, None)
    }

    pub fn queue_side_path(
        &mut self,
        key: FunctionKey,
        snapshot: VerifiedFunction,
        feedback: FeedbackSnapshot,
        profile: SidePathProfile,
    ) -> Result<(), QueueError> {
        if profile.function != key
            || profile.feedback_epoch == 0
            || profile.feedback_epoch != feedback.epoch()
            || !feedback.contains_stable_observation(key, profile.pc, profile.observed)
            // Side-path lowering only specializes numeric exits and rejects any
            // other profile as an invalid artifact; queueing one would spend a
            // compile and record a failure against the installed Tier 2 code.
            || !matches!(profile.observed, ObservedType::Int32 | ObservedType::Float64)
            || !self.installed_keys.contains_key(&(key, Tier::Baseline))
            || !self.installed_keys.contains_key(&(key, Tier::Optimizing))
        {
            return Err(QueueError::SnapshotIdentity);
        }
        self.queue_request(key, Tier::Optimizing, snapshot, feedback, Some(profile))
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    fn retain_frame_inline_target(
        &mut self,
        caller: FunctionKey,
        pc: u32,
        feedback: &FeedbackSnapshot,
        budget: &mut FrameInlineBudget,
        ancestors: &mut Vec<FunctionKey>,
        depth: usize,
    ) -> Option<FrameInlineTarget> {
        if depth > FRAME_INLINE_MAX_DEPTH {
            return None;
        }
        let target = feedback.call_link_at(caller, pc)?;
        if !budget.begin_candidate() {
            return None;
        }
        if ancestors.contains(&target.callee()) {
            return None;
        }
        let pin = self
            .pin(target.callee(), Tier::Optimizing)
            .or_else(|| self.pin(target.callee(), Tier::Baseline))?;
        let artifact = pin.artifact();
        let snapshot = artifact.inline_snapshot()?;
        let artifact_key = artifact.key();
        if snapshot.function_id() != target.callee().id
            || snapshot.generation() != target.callee().generation
            || snapshot.function_id() != artifact_key.function_id
            || snapshot.generation() != artifact_key.generation
            || snapshot.source_revision() != artifact_key.source_revision
            || snapshot.opcode_fingerprint() != artifact_key.opcode_fingerprint
        {
            return None;
        }
        let body = budget.verify(snapshot)?;
        let snapshot = body.snapshot().clone();
        if !budget.claim(&body) {
            return None;
        }

        ancestors.push(target.callee());
        let children = if depth == FRAME_INLINE_MAX_DEPTH {
            Vec::new()
        } else {
            let mut children = Vec::new();
            for instruction in body.instructions() {
                if budget.exhausted() {
                    break;
                }
                if let Some(child) = self.retain_frame_inline_target(
                    target.callee(),
                    instruction.pc(),
                    feedback,
                    budget,
                    ancestors,
                    depth + 1,
                ) {
                    children.push(child);
                }
            }
            children
        };
        let popped = ancestors.pop();
        debug_assert_eq!(popped, Some(target.callee()));
        Some(FrameInlineTarget {
            target,
            artifact_key,
            snapshot,
            children: children.into(),
        })
    }

    fn queue_request(
        &mut self,
        key: FunctionKey,
        tier: Tier,
        snapshot: VerifiedFunction,
        feedback: FeedbackSnapshot,
        side_path_profile: Option<SidePathProfile>,
    ) -> Result<(), QueueError> {
        if self.shutdown {
            return Err(QueueError::Shutdown);
        }
        let side_path_signature = side_path_profile.map(SidePathProfile::signature_fingerprint);
        if side_path_signature.is_some_and(|signature| {
            self.specialization_versions
                .get(&(key, signature))
                .copied()
                .unwrap_or(0)
                >= self.max_attempts
        }) {
            return Err(QueueError::Blacklisted);
        }
        if self
            .current_generations
            .get(&key.id)
            .is_some_and(|generation| key.generation < *generation)
        {
            return Err(QueueError::Retired);
        }
        if self
            .functions
            .get(&key)
            .is_some_and(|function| function.retired)
        {
            return Err(QueueError::Retired);
        }
        if self
            .functions
            .get(&key)
            .and_then(FunctionState::active_state)
            .is_some()
        {
            return Err(QueueError::NotReady);
        }
        let installed_baseline = self.installed_keys.contains_key(&(key, Tier::Baseline));
        let interpreter_deopt_target = self
            .functions
            .get(&key)
            .is_some_and(|function| function.baseline.state == CompileState::Blacklisted);
        let installed_optimizing = self.installed_keys.contains_key(&(key, Tier::Optimizing));
        match tier {
            Tier::Baseline if installed_baseline || installed_optimizing => {
                return Err(QueueError::NotReady)
            }
            Tier::Optimizing
                if (!installed_baseline && !interpreter_deopt_target)
                    || (installed_optimizing && side_path_profile.is_none()) =>
            {
                return Err(QueueError::NotReady)
            }
            _ => {}
        }
        let tier_state = self
            .functions
            .get(&key)
            .map_or(CompileState::Cold, |function| function.tier(tier).state);
        match tier_state {
            CompileState::Cold => {}
            CompileState::Installed(Tier::Optimizing) if side_path_profile.is_some() => {}
            CompileState::Backoff { retry_after, .. } if self.clock >= retry_after => {}
            CompileState::Blacklisted => return Err(QueueError::Blacklisted),
            _ => return Err(QueueError::NotReady),
        }
        let source = snapshot.snapshot();
        if (source.function_id() != 0 && source.function_id() != key.id)
            || (source.generation() != 0 && source.generation() != key.generation)
        {
            return Err(QueueError::SnapshotIdentity);
        }
        let primary_feedback_signature = (tier == Tier::Optimizing && side_path_profile.is_none())
            .then(|| feedback.bounded_specialization(key))
            .flatten()
            .map(|signature| signature.fingerprint());
        let call_feedback_signature = (tier == Tier::Optimizing && side_path_profile.is_none())
            .then(|| {
                snapshot
                    .instructions()
                    .iter()
                    .filter_map(|instruction| {
                        feedback.call_specialization_at(key, instruction.pc())
                    })
                    .map(|call| call_specialization_fingerprint(&call))
                    .reduce(|prior, fingerprint| prior.rotate_left(17) ^ fingerprint)
            })
            .flatten();
        let feedback_signature = match (primary_feedback_signature, call_feedback_signature) {
            (Some(primary), Some(call)) => {
                Some(ArtifactVersionIdentity::new(primary, call).fingerprint())
            }
            (Some(primary), None) => Some(primary),
            (None, Some(call)) => Some(ArtifactVersionIdentity::new(0, call).fingerprint()),
            (None, None) => None,
        };
        let artifact_key = ArtifactKey {
            runtime_id: self.environment.runtime_id,
            function_id: key.id,
            generation: key.generation,
            tier,
            target_isa: self.environment.target_isa,
            cpu_features: self.environment.cpu_features,
            abi_fingerprint: self.environment.abi_fingerprint,
            source_revision: source.source_revision(),
            opcode_fingerprint: source.opcode_fingerprint(),
            config_fingerprint: self.environment.config_fingerprint,
            specialization_fingerprint: side_path_profile
                .map(SidePathProfile::fingerprint)
                .or(feedback_signature)
                .unwrap_or(0),
        };
        if self
            .current_generations
            .get(&key.id)
            .is_none_or(|generation| key.generation > *generation)
        {
            self.retire_older_generations(key);
            self.current_generations.insert(key.id, key.generation);
        }
        if self.queue.len() >= self.max_queue_len {
            self.metrics.queue_saturated = self.metrics.queue_saturated.saturating_add(1);
            return Err(QueueError::Full);
        }
        let Some(next_attempt_id) = self.next_attempt_id.checked_add(1) else {
            return Err(QueueError::AttemptIdsExhausted);
        };
        self.next_attempt_id = next_attempt_id;
        let feedback_epoch = feedback.epoch();
        if tier == Tier::Optimizing && side_path_profile.is_none() {
            self.latest_feedback_epochs
                .entry(key)
                .and_modify(|epoch| *epoch = (*epoch).max(feedback_epoch))
                .or_insert(feedback_epoch);
        }
        #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
        let direct_call_targets: Arc<[DirectCallTarget]> =
            if matches!(tier, Tier::Baseline | Tier::Optimizing) && side_path_profile.is_none() {
                // This also bounds snapshot retention while requests wait in
                // the coordinator's count-bounded foreground queue.
                let mut inline_bytes_left = 64 * 1024usize;
                snapshot
                    .instructions()
                    .iter()
                    .filter_map(|instruction| {
                        let call = feedback.call_specialization_at(key, instruction.pc())?;
                        let pin = self
                            .pin(call.callee(), Tier::Optimizing)
                            .or_else(|| self.pin(call.callee(), Tier::Baseline))?;
                        let artifact = pin.artifact();
                        let signature = artifact
                            .optimized_metadata()?
                            .direct_call_signature()?
                            .clone();
                        if signature.function() != call.callee()
                            || signature.arguments() != call.arguments()
                            || signature.result() != call.result()
                        {
                            return None;
                        }
                        let published = artifact.direct_call_published()?.clone();
                        let inline_snapshot = artifact.inline_snapshot().and_then(|body| {
                            let remaining = inline_bytes_left.checked_sub(body.retained_bytes())?;
                            inline_bytes_left = remaining;
                            Some(body.clone())
                        });
                        Some(DirectCallTarget {
                            pc: instruction.pc(),
                            call,
                            signature,
                            published,
                            artifact_key: artifact.key(),
                            inline_snapshot,
                        })
                    })
                    .collect::<Vec<_>>()
                    .into()
            } else {
                Arc::from([])
            };
        #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
        let frame_inline_targets: Arc<[FrameInlineTarget]> =
            if tier == Tier::Optimizing && side_path_profile.is_none() {
                // Pure inlining keeps first claim on the shared body budget.
                // Bounded sites also cap candidate/IR planning overhead.
                let bytes_left = direct_call_targets
                    .iter()
                    .fold(64 * 1024usize, |left, target| {
                        left.saturating_sub(
                            target
                                .inline_snapshot()
                                .map_or(0, crate::bytecode::CompileSnapshot::retained_bytes),
                        )
                    });
                let mut budget = FrameInlineBudget::new(bytes_left);
                let mut ancestors = vec![key];
                let mut targets = Vec::new();
                for instruction in snapshot.instructions() {
                    if budget.exhausted() {
                        break;
                    }
                    // Pure inlining owns its site. A linked direct entry is a
                    // transaction with an exact pre-effect miss to the generic
                    // CALL and needs no shadow frame, so it is preferred over
                    // frame inlining, but only where the Tier 2 caller can
                    // take it. Elsewhere the site would reach the generic CALL.
                    if direct_call_targets.iter().any(|target| {
                        target.pc() == instruction.pc()
                            && (target.inline_snapshot().is_some()
                                || tier2_direct_call_site_usable(
                                    &snapshot,
                                    instruction.pc(),
                                    target.call(),
                                ))
                    }) {
                        continue;
                    }
                    if let Some(target) = self.retain_frame_inline_target(
                        key,
                        instruction.pc(),
                        &feedback,
                        &mut budget,
                        &mut ancestors,
                        1,
                    ) {
                        targets.push(target);
                    }
                }
                targets.into()
            } else {
                Arc::from([])
            };
        #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
        let native_call_targets: Arc<[NativeCallTarget]> =
            if tier == Tier::Optimizing && side_path_profile.is_none() {
                snapshot
                    .instructions()
                    .iter()
                    .filter_map(|instruction| {
                        let link = feedback.call_link_at(key, instruction.pc())?;
                        if link.callee() == key {
                            return None;
                        }
                        let pin = self.pin(link.callee(), Tier::Optimizing)?;
                        let artifact = pin.artifact();
                        let plan = artifact.optimized_metadata()?.native_call_plan()?.clone();
                        let published = artifact.native_call_published()?.clone();
                        Some(NativeCallTarget {
                            link,
                            plan,
                            published,
                        })
                    })
                    .collect::<Vec<_>>()
                    .into()
            } else {
                Arc::from([])
            };
        #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
        let artifact_key = if frame_inline_targets.is_empty() {
            artifact_key
        } else {
            use std::hash::{Hash, Hasher};
            let mut hash = rustc_hash::FxHasher::default();
            for target in frame_inline_targets.iter() {
                for descendant in target.descendants() {
                    descendant.target.hash(&mut hash);
                    descendant.artifact_key.hash(&mut hash);
                }
            }
            artifact_key.with_version_identity(ArtifactVersionIdentity::new(
                artifact_key.specialization_fingerprint,
                hash.finish(),
            ))
        };
        self.queue.push_back(CompileRequest {
            key,
            tier,
            snapshot,
            artifact_key,
            attempt_id: AttemptId(next_attempt_id),
            feedback_epoch,
            feedback: Arc::new(feedback),
            side_path_profile,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            direct_call_targets,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            frame_inline_targets,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            native_call_targets,
            retired: Arc::new(AtomicBool::new(false)),
        });
        if let Some(signature) = side_path_signature {
            let versions = self
                .specialization_versions
                .entry((key, signature))
                .or_default();
            *versions = versions.saturating_add(1);
        }
        let function = self.functions.entry(key).or_default();
        function.tier_mut(tier).state = CompileState::Queued(tier);
        self.metrics.queued = self.metrics.queued.saturating_add(1);
        Ok(())
    }

    /// Moves a queued request to the front of the compile queue. Returns
    /// false when no such request is waiting.
    pub fn prioritize(&mut self, key: FunctionKey, tier: Tier) -> bool {
        let Some(index) = self
            .queue
            .iter()
            .position(|request| request.key == key && request.tier == tier)
        else {
            return false;
        };
        if let Some(request) = self.queue.remove(index) {
            self.queue.push_front(request);
        }
        true
    }

    pub fn begin_next(&mut self) -> Option<CompileRequest> {
        loop {
            let request = self.queue.pop_front()?;
            let Some(record) = self.functions.get_mut(&request.key) else {
                continue;
            };
            if record.tier(request.tier).state != CompileState::Queued(request.tier) {
                continue;
            }
            record.tier_mut(request.tier).state = CompileState::Compiling(request.tier);
            self.in_flight.insert(
                request.key,
                InFlight {
                    attempt_id: request.attempt_id,
                    artifact_key: request.artifact_key,
                    tier: request.tier,
                    feedback_epoch: request.feedback_epoch,
                    side_path: request.side_path_profile.is_some(),
                },
            );
            self.in_flight_retired
                .insert(request.key, Arc::clone(&request.retired));
            self.metrics.compiling = self.metrics.compiling.saturating_add(1);
            return Some(request);
        }
    }

    pub(super) fn rollback_dispatch(&mut self, request: CompileRequest) {
        self.rollback(request, true);
    }

    pub(super) fn rollback_resource_limit(&mut self, request: CompileRequest) {
        self.rollback(request, false);
    }

    fn rollback(&mut self, request: CompileRequest, saturated: bool) {
        if self.in_flight.get(&request.key).is_some_and(|flight| {
            flight.attempt_id == request.attempt_id && flight.tier == request.tier
        }) {
            self.in_flight.remove(&request.key);
            if let Some(record) = self.functions.get_mut(&request.key) {
                record.tier_mut(request.tier).state = CompileState::Queued(request.tier);
            }
            self.queue.push_front(request);
            if saturated {
                self.metrics.worker_queue_saturated =
                    self.metrics.worker_queue_saturated.saturating_add(1);
            }
        }
    }

    pub fn complete(&mut self, completion: CompileCompletion) {
        let key = completion.key;
        self.complete_in_flight(completion);
        if !self.in_flight.contains_key(&key) {
            self.in_flight_retired.remove(&key);
        }
    }

    fn complete_in_flight(&mut self, completion: CompileCompletion) {
        let Some(expected) = self.in_flight.get(&completion.key).copied() else {
            #[cfg(feature = "test-support")]
            self.record_completion_disposition(
                completion.key,
                CompletionDisposition::MissingInFlight,
            );
            self.metrics.stale_results = self.metrics.stale_results.saturating_add(1);
            return;
        };
        if !invalidate::is_current_generation(&self.current_generations, completion.key)
            || self
                .functions
                .get(&completion.key)
                .map(|function| function.tier(completion.requested_tier).state)
                != Some(CompileState::Compiling(completion.requested_tier))
            || expected.attempt_id != completion.attempt_id
            || expected.tier != completion.requested_tier
            || expected.artifact_key != completion.artifact_key
        {
            #[cfg(feature = "test-support")]
            self.record_completion_disposition(
                completion.key,
                CompletionDisposition::StaleEnvelope,
            );
            self.metrics.stale_results = self.metrics.stale_results.saturating_add(1);
            return;
        }
        match completion.result {
            Ok(artifact) if completion.artifact_key == artifact.key() => {
                #[cfg(feature = "compiler")]
                if completion.requested_tier == Tier::Optimizing {
                    if let Some(metadata) = artifact.optimized_metadata() {
                        let artifact_epoch = metadata.feedback_epoch();
                        let latest = self.latest_feedback_epochs.get(&completion.key).copied();
                        if artifact_epoch != expected.feedback_epoch
                            || (!expected.side_path && latest != Some(expected.feedback_epoch))
                        {
                            #[cfg(feature = "test-support")]
                            self.record_completion_disposition(
                                completion.key,
                                CompletionDisposition::FeedbackEpochMismatch {
                                    expected: expected.feedback_epoch,
                                    artifact: artifact_epoch,
                                    latest,
                                },
                            );
                            self.in_flight.remove(&completion.key);
                            self.metrics.stale_results =
                                self.metrics.stale_results.saturating_add(1);
                            self.record_invalid_artifact(completion.key, completion.requested_tier);
                            return;
                        }
                    }
                }
                self.in_flight.remove(&completion.key);
                if let Some(record) = self.functions.get_mut(&completion.key) {
                    record.tier_mut(completion.requested_tier).state =
                        CompileState::Ready(completion.requested_tier);
                }
                if !invalidate::is_current_generation(&self.current_generations, completion.key) {
                    self.metrics.stale_results = self.metrics.stale_results.saturating_add(1);
                    self.retire_state(completion.key);
                    return;
                }
                let artifact_key = artifact.key();
                #[cfg(feature = "compiler")]
                let optimization_metrics = artifact.optimized_metadata().map(|metadata| {
                    (
                        metadata.boxes_elided(),
                        metadata.cse_eliminated(),
                        metadata.dead_nodes_eliminated(),
                    )
                });
                let dependency_versions = artifact
                    .dependencies()
                    .iter()
                    .map(|dependency| {
                        (
                            DependencyKey::function(dependency.function),
                            dependency.function.generation,
                        )
                    })
                    .collect::<Vec<_>>();
                if let Some((_dependency, _)) =
                    dependency_versions
                        .iter()
                        .find(|(dependency, generation)| match *dependency {
                            DependencyKey::Function(function) => {
                                function.generation != *generation
                                    || self.current_generations.get(&function.id)
                                        != Some(generation)
                            }
                            DependencyKey::Shape(_) | DependencyKey::Prototype(_) => false,
                        })
                {
                    #[cfg(feature = "test-support")]
                    if let DependencyKey::Function(function) = *_dependency {
                        let current_generation =
                            self.current_generations.get(&function.id).copied();
                        self.record_completion_disposition(
                            completion.key,
                            CompletionDisposition::DependencyGenerationMismatch {
                                dependency: function,
                                current_generation,
                            },
                        );
                    }
                    self.record_invalid_artifact(completion.key, completion.requested_tier);
                    return;
                }
                let mut staged_dependencies = self.dependencies.clone();
                if staged_dependencies
                    .install(
                        DependencyKey::function(completion.key),
                        completion.key.generation,
                        dependency_versions
                            .iter()
                            .map(|(dependency, _)| *dependency),
                    )
                    .is_err()
                {
                    #[cfg(feature = "test-support")]
                    self.record_completion_disposition(
                        completion.key,
                        CompletionDisposition::DependencyInstallFailed,
                    );
                    self.record_install_failure(completion.key, completion.requested_tier);
                    return;
                }
                match install::publish(&mut self.cache, artifact) {
                    Ok(insert) => {
                        #[cfg(feature = "test-support")]
                        self.record_completion_disposition(
                            completion.key,
                            CompletionDisposition::Installed,
                        );
                        self.dependencies = staged_dependencies;
                        for evicted in insert.evictions() {
                            self.record_eviction(*evicted);
                        }
                        self.last_benefit_target = None;
                        self.installed_keys
                            .insert((completion.key, completion.requested_tier), artifact_key);
                        if completion.requested_tier == Tier::Optimizing {
                            // Reset the per-artifact retry counts, but preserve
                            // guard/type instability history across recompiles.
                            if let Some(exits) = self.side_exits.get_mut(&completion.key) {
                                for count in exits.values_mut() {
                                    *count = 0;
                                }
                            }
                        }
                        if expected.side_path {
                            self.latest_feedback_epochs
                                .insert(completion.key, expected.feedback_epoch);
                        }
                        if let Some(record) = self.functions.get_mut(&completion.key) {
                            let tier_record = record.tier_mut(completion.requested_tier);
                            tier_record.state = CompileState::Cold;
                            tier_record.attempts = 0;
                            record.published = Some(completion.requested_tier);
                        }
                        self.metrics.installed = self.metrics.installed.saturating_add(1);
                        #[cfg(feature = "compiler")]
                        if let Some((boxes, cse, dead)) = optimization_metrics {
                            self.metrics.boxes_elided =
                                self.metrics.boxes_elided.saturating_add(boxes);
                            self.metrics.cse_eliminated =
                                self.metrics.cse_eliminated.saturating_add(cse);
                            self.metrics.dead_nodes_eliminated =
                                self.metrics.dead_nodes_eliminated.saturating_add(dead);
                        }
                    }
                    Err(_) => {
                        #[cfg(feature = "test-support")]
                        self.record_completion_disposition(
                            completion.key,
                            CompletionDisposition::InstallFailed,
                        );
                        self.record_install_failure(completion.key, completion.requested_tier)
                    }
                }
            }
            Ok(_) => {
                #[cfg(feature = "test-support")]
                self.record_completion_disposition(
                    completion.key,
                    CompletionDisposition::ArtifactKeyMismatch,
                );
                self.metrics.invalid_artifacts = self.metrics.invalid_artifacts.saturating_add(1);
                self.metrics.stale_results = self.metrics.stale_results.saturating_add(1);
            }
            Err(failure) => {
                #[cfg(feature = "test-support")]
                self.record_completion_disposition(
                    completion.key,
                    CompletionDisposition::CompileFailed(failure),
                );
                self.in_flight.remove(&completion.key);
                self.record_compile_failure(completion.key, completion.requested_tier, failure);
            }
        }
    }

    fn record_compile_failure(&mut self, key: FunctionKey, tier: Tier, failure: CompileFailure) {
        let category = match failure {
            CompileFailure::UnsupportedOpcode => &mut self.metrics.unsupported_opcode_failures,
            CompileFailure::Tier1Rejected(_) => &mut self.metrics.tier1_rejections,
            CompileFailure::ResourceLimit => &mut self.metrics.resource_limit_failures,
            CompileFailure::TimedOut => &mut self.metrics.compile_timeouts,
            CompileFailure::Cancelled => &mut self.metrics.cancelled_compilations,
            CompileFailure::CompilerPanicked => &mut self.metrics.compiler_panics,
            CompileFailure::InvalidArtifact => &mut self.metrics.invalid_artifacts,
        };
        *category = category.saturating_add(1);
        self.record_failure(key, tier);
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub(crate) fn reject_tier1(
        &mut self,
        key: FunctionKey,
        tier: Tier,
        reason: crate::bytecode::FallbackReason,
    ) {
        if self
            .current_generations
            .get(&key.id)
            .is_none_or(|generation| key.generation > *generation)
        {
            self.retire_older_generations(key);
            self.current_generations.insert(key.id, key.generation);
        }
        self.functions.entry(key).or_default();
        self.record_compile_failure(key, tier, CompileFailure::Tier1Rejected(reason));
        let tier_record = self
            .functions
            .get_mut(&key)
            .expect("Tier 1 rejection registered the function")
            .tier_mut(tier);
        if tier_record.state != CompileState::Blacklisted {
            tier_record.state = CompileState::Blacklisted;
            self.metrics.blacklisted = self.metrics.blacklisted.saturating_add(1);
        }
    }

    fn record_invalid_artifact(&mut self, key: FunctionKey, tier: Tier) {
        self.metrics.invalid_artifacts = self.metrics.invalid_artifacts.saturating_add(1);
        self.record_failure(key, tier);
    }

    fn record_install_failure(&mut self, key: FunctionKey, tier: Tier) {
        self.metrics.install_failures = self.metrics.install_failures.saturating_add(1);
        self.record_failure(key, tier);
    }

    fn record_failure(&mut self, key: FunctionKey, tier: Tier) {
        self.metrics.compile_failures = self.metrics.compile_failures.saturating_add(1);
        let Some(record) = self.functions.get_mut(&key) else {
            return;
        };
        let tier_record = record.tier_mut(tier);
        tier_record.attempts = tier_record.attempts.saturating_add(1);
        if tier_record.attempts >= self.max_attempts {
            tier_record.state = CompileState::Blacklisted;
            self.metrics.blacklisted = self.metrics.blacklisted.saturating_add(1);
        } else {
            let retry_after = self.clock.saturating_add(u64::from(tier_record.attempts));
            tier_record.state = CompileState::Backoff {
                attempts: tier_record.attempts,
                retry_after,
            };
        }
    }

    fn retire_older_generations(&mut self, current: FunctionKey) {
        let older = self
            .functions
            .keys()
            .copied()
            .filter(|key| key.id == current.id && key.generation < current.generation)
            .collect::<Vec<_>>();
        for key in older {
            self.retire_state(key);
        }
    }

    fn retire_state(&mut self, key: FunctionKey) {
        self.last_benefit_target = None;
        self.queue.retain(|request| request.key != key);
        self.in_flight.remove(&key);
        if let Some(retired) = self.in_flight_retired.remove(&key) {
            retired.store(true, Ordering::Release);
        }
        let function = self.functions.entry(key).or_default();
        let was_retired = function.retired;
        function.retired = true;
        function.published = None;
        if !was_retired {
            self.metrics.retired = self.metrics.retired.saturating_add(1);
        }
        self.installed_keys
            .retain(|(installed_key, _), _| *installed_key != key);
        self.cache.invalidate_deferred(key);
    }

    fn record_eviction(&mut self, evicted: ArtifactKey) {
        self.last_benefit_target = None;
        let key = FunctionKey::new(evicted.function_id, evicted.generation);
        self.installed_keys.remove(&(key, evicted.tier));
        if let Some(record) = self.functions.get_mut(&key) {
            if record.published == Some(evicted.tier) {
                record.published = (evicted.tier == Tier::Optimizing
                    && self.installed_keys.contains_key(&(key, Tier::Baseline)))
                .then_some(Tier::Baseline);
            }
        }
        self.metrics.evicted = self.metrics.evicted.saturating_add(1);
    }

    pub fn retire(&mut self, key: FunctionKey) {
        let advances_watermark = self
            .current_generations
            .get(&key.id)
            .is_none_or(|generation| key.generation > *generation);
        if advances_watermark {
            self.retire_older_generations(key);
            self.current_generations.insert(key.id, key.generation);
        }
        let mut invalidated = self.dependencies.invalidate(DependencyKey::function(key));
        // A queued or compiling function has no dependency node yet. Retirement
        // must still retire the identity itself so a late worker completion can
        // never publish it.
        let own_dependency = DependencyKey::function(key);
        if !invalidated.contains(&own_dependency) {
            invalidated.push(own_dependency);
        }
        self.metrics.dependency_invalidations = self
            .metrics
            .dependency_invalidations
            .saturating_add(invalidated.len() as u64);
        for dependency in invalidated {
            if let DependencyKey::Function(function) = dependency {
                self.retire_state(function);
            }
        }
    }

    /// Unpublishes a harmful baseline version so future entries stay in the
    /// interpreter. Automatic tiering uses this only after its bounded
    /// profitability retries are exhausted; BaselineOnly never calls it.
    pub fn demote_baseline_to_interpreter(&mut self, key: FunctionKey) -> bool {
        self.last_benefit_target = None;
        let Some(function) = self.functions.get_mut(&key) else {
            return false;
        };
        if self.installed_keys.remove(&(key, Tier::Baseline)).is_none() {
            return false;
        }
        function.baseline.state = CompileState::Blacklisted;
        self.profitability_demotions.insert(key);
        if function.published == Some(Tier::Baseline) {
            function.published = None;
        }
        /* Keep the unpublished Baseline artifact resident as the cache's
         * lifetime pin while a bounded Tier2 trial is compiled. It is no
         * longer addressable through `installed_keys` or `published`, so it
         * cannot execute; optimized side exits resume the interpreter. */
        self.metrics.interpreter_demotions = self.metrics.interpreter_demotions.saturating_add(1);
        self.metrics.blacklisted = self.metrics.blacklisted.saturating_add(1);
        true
    }

    /// Returns true when an installed baseline caller has a stable call edge
    /// whose scalar callee entry is now published, but the caller artifact was
    /// compiled before that dependency became available.
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn direct_call_ready(&mut self, call: &CallSpecializationKey) -> bool {
        self.pin(call.callee(), Tier::Optimizing)
            .or_else(|| self.pin(call.callee(), Tier::Baseline))
            .is_some_and(|pin| artifact_matches_direct_call(pin.artifact(), call))
    }

    /// Returns whether the immutable monomorphic target has a retained body
    /// that the frame-inline planner can actually consume. Merely finding an
    /// inline snapshot is insufficient: unsupported CFG/exception/closure
    /// shapes must take the call-IC backoff path rather than stall forever.
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn frame_inline_candidate_ready(
        &mut self,
        caller: FunctionKey,
        caller_body: &crate::bytecode::VerifiedFunction,
        pc: u32,
        feedback: &FeedbackSnapshot,
    ) -> bool {
        let Some(target) = feedback.call_link_at(caller, pc) else {
            return false;
        };
        if target.callee() == caller {
            return false;
        }
        let Some(pin) = self
            .pin(target.callee(), Tier::Optimizing)
            .or_else(|| self.pin(target.callee(), Tier::Baseline))
        else {
            return false;
        };
        let artifact = pin.artifact();
        let Some(snapshot) = artifact.inline_snapshot() else {
            return false;
        };
        let artifact_key = artifact.key();
        let Ok(body) = snapshot.verify(crate::bytecode::VerifyLimits::default()) else {
            return false;
        };
        let Some(instruction) = caller_body
            .instructions()
            .iter()
            .find(|instruction| instruction.pc() == pc)
        else {
            return false;
        };
        let kind = match instruction.opcode().name() {
            "call" | "call0" | "call1" | "call2" | "call3" => crate::ir::InlineCallKind::Call,
            "call_method" => crate::ir::InlineCallKind::Method,
            _ => return false,
        };
        let raw = caller_body.snapshot();
        let caller_shape = crate::ir::OptimizedFrameShape::new(
            raw.arg_count(),
            raw.local_count(),
            raw.stack_size(),
        );
        crate::ir::FrameInlineCallee {
            artifact: artifact_key,
            target,
            body,
            children: Default::default(),
        }
        .admission_probe(kind, caller_shape)
    }

    /// True when the callee's installed optimizing artifact publishes a P3a
    /// native call entry that a monomorphic caller can link to.
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn native_call_ready(&mut self, callee: FunctionKey) -> bool {
        self.pin(callee, Tier::Optimizing).is_some_and(|pin| {
            pin.artifact().native_call_published().is_some()
                && pin
                    .artifact()
                    .optimized_metadata()
                    .is_some_and(|metadata| metadata.native_call_plan().is_some())
        })
    }

    /// A resolved target cannot become frame-inlineable without a generation
    /// or artifact replacement, both of which change the normal readiness
    /// inputs and retry the caller independently.
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn call_target_resolved(&mut self, callee: FunctionKey) -> bool {
        self.pin(callee, Tier::Optimizing)
            .or_else(|| self.pin(callee, Tier::Baseline))
            .is_some()
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn baseline_direct_refresh_ready(
        &mut self,
        key: FunctionKey,
        feedback: &FeedbackSnapshot,
    ) -> bool {
        let Some(caller_pin) = self.pin(key, Tier::Baseline) else {
            return false;
        };
        let installed_dependencies = caller_pin
            .artifact()
            .dependencies()
            .iter()
            .map(|dependency| dependency.function)
            .collect::<std::collections::HashSet<_>>();
        drop(caller_pin);
        feedback.call_specializations_for(key).any(|call| {
            if installed_dependencies.contains(&call.callee()) {
                return false;
            }
            let callee_pin = self
                .pin(call.callee(), Tier::Optimizing)
                .or_else(|| self.pin(call.callee(), Tier::Baseline));
            callee_pin.is_some_and(|pin| artifact_matches_direct_call(pin.artifact(), &call))
        })
    }

    /// Atomically makes an installed baseline caller queueable again. Existing
    /// executions retain their publication pin; new entries use the
    /// interpreter until the refreshed artifact is installed.
    pub fn prepare_baseline_direct_refresh(&mut self, key: FunctionKey) -> bool {
        self.last_benefit_target = None;
        let Some(function) = self.functions.get_mut(&key) else {
            return false;
        };
        if function.published != Some(Tier::Baseline)
            || !self.installed_keys.contains_key(&(key, Tier::Baseline))
        {
            return false;
        }
        self.installed_keys.remove(&(key, Tier::Baseline));
        function.baseline.state = CompileState::Cold;
        function.published = None;
        true
    }

    pub fn state(&self, key: FunctionKey) -> CompileState {
        self.functions
            .get(&key)
            .map_or(CompileState::Cold, FunctionState::visible_state)
    }

    pub fn tier_state(&self, key: FunctionKey, tier: Tier) -> CompileState {
        let Some(function) = self.functions.get(&key) else {
            return CompileState::Cold;
        };
        if function.retired {
            return CompileState::Retired;
        }
        let state = function.tier(tier).state;
        if state != CompileState::Cold {
            return state;
        }
        // Publication already proves residency for the active tier. A lower
        // installed tier may coexist with it, so retain the exact-key lookup
        // only when asking about that non-published tier.
        if function.published == Some(tier) || self.installed_keys.contains_key(&(key, tier)) {
            CompileState::Installed(tier)
        } else {
            CompileState::Cold
        }
    }

    /// Reports immutable generations that cannot publish either native tier.
    /// Profitability demotions remain probeable for their bounded Tier2 trial.
    pub fn is_terminally_blacklisted(&self, key: FunctionKey) -> bool {
        let Some(function) = self.functions.get(&key) else {
            return false;
        };
        !function.retired
            && (function.optimizing.state == CompileState::Blacklisted
                || (function.baseline.state == CompileState::Blacklisted
                    && !self.profitability_demotions.contains(&key)))
    }

    pub fn advance_clock(&mut self, now: u64) {
        self.clock = self.clock.max(now);
    }

    pub fn metrics(&self) -> JitMetrics {
        let mut metrics = self.metrics.value.clone();
        metrics.completion_queue_saturated =
            self.completion_signals.saturated.load(Ordering::Acquire);
        metrics.code_bytes = self.cache.charged_code_bytes();
        metrics.metadata_bytes = self.cache.charged_metadata_bytes();
        metrics
    }

    /// Refreshes the backend's single persistent publication snapshot. The
    /// public `metrics()` accessor remains an independent complete snapshot.
    /// Returns whether the whole snapshot was replaced, including caller-owned fields.
    #[cfg(any(test, feature = "compiler"))]
    pub(crate) fn refresh_published_metrics(&mut self, snapshot: &mut JitMetrics) -> bool {
        let replaced = self.metrics.dirty;
        if replaced {
            snapshot.clone_from(&self.metrics.value);
            self.metrics.dirty = false;
        }
        snapshot.completion_queue_saturated =
            self.completion_signals.saturated.load(Ordering::Acquire);
        snapshot.code_bytes = self.cache.charged_code_bytes();
        snapshot.metadata_bytes = self.cache.charged_metadata_bytes();
        snapshot.tier2_entries = self.metrics.value.tier2_entries;
        snapshot.side_path_entries = self.metrics.value.side_path_entries;
        replaced
    }

    pub fn set_native_enabled(&mut self, enabled: bool) {
        self.metrics.set_native_enabled(enabled);
    }

    pub fn set_worker_usage(&mut self, jobs: usize, snapshots: usize, ir: usize) {
        self.metrics.pending_worker_jobs = jobs;
        self.metrics.pending_snapshot_bytes = snapshots;
        self.metrics.active_ir_bytes = ir;
    }

    pub fn record_resource_limit_rejection(&mut self) {
        self.metrics.resource_limit_rejections =
            self.metrics.resource_limit_rejections.saturating_add(1);
    }

    pub fn record_tier2_entry(&mut self) {
        self.metrics.value.tier2_entries = self.metrics.value.tier2_entries.saturating_add(1);
    }

    /// Feeds observed time saved by an installed artifact into cache eviction.
    pub fn record_benefit(&mut self, key: FunctionKey, tier: Tier, saved_ns: u64) -> bool {
        let artifact = match self.last_benefit_target {
            Some((function, cached_tier, artifact)) if function == key && cached_tier == tier => {
                artifact
            }
            _ => {
                let Some(artifact) = self.installed_keys.get(&(key, tier)).copied() else {
                    return false;
                };
                self.last_benefit_target = Some((key, tier, artifact));
                artifact
            }
        };
        self.cache.record_benefit(artifact, saved_ns).is_ok()
    }

    /// Unpublishes an optimizing artifact whose bounded production trial was
    /// measurably slower than the installed baseline it falls back to.
    ///
    /// This mirrors the bounded tier-down/backoff used by production JITs: the
    /// usable baseline remains the retry target while the optimizing tier
    /// backs off. A function whose baseline was already demoted keeps its
    /// optimizing version instead: the interpreter is its only fallback, so
    /// the baseline timing it lost to is not what it would run next, and a
    /// wall-clock comparison across two separate windows (core migration,
    /// frequency scaling, a busy compile worker, sanitizer hosts) is too weak
    /// evidence for a verdict this generation could never revisit. Deopt
    /// accounting still bounds that trial. Active pins remain valid until
    /// their invocation returns; new acquisitions see the baseline
    /// immediately.
    pub fn demote_unprofitable_optimized(&mut self, key: FunctionKey) -> bool {
        self.last_benefit_target = None;
        if !self.installed_keys.contains_key(&(key, Tier::Baseline))
            || self
                .installed_keys
                .remove(&(key, Tier::Optimizing))
                .is_none()
        {
            return false;
        }

        let function = self.functions.entry(key).or_default();
        function.instability_attempts = function.instability_attempts.saturating_add(1);
        let attempts = function.instability_attempts;
        let delay = 1u64
            .checked_shl(u32::from(attempts.min(20)))
            .unwrap_or(u64::MAX);
        function.optimizing.state = CompileState::Backoff {
            attempts,
            retry_after: self.clock.saturating_add(delay),
        };
        function.published = Some(Tier::Baseline);
        self.metrics.optimized_demotions = self.metrics.optimized_demotions.saturating_add(1);
        true
    }

    pub fn record_side_path_entries(&mut self, count: u64) {
        self.metrics.value.side_path_entries =
            self.metrics.value.side_path_entries.saturating_add(count);
    }
    pub fn record_deopt(&mut self, guard_failure: bool) {
        self.metrics.deopts = self.metrics.deopts.saturating_add(1);
        self.metrics.side_exits = self.metrics.side_exits.saturating_add(1);
        if guard_failure {
            self.metrics.tier2_guard_failures = self.metrics.tier2_guard_failures.saturating_add(1);
        }
    }

    pub fn record_optimized_side_exit(&mut self, key: FunctionKey, guard: u32) -> SideExitAction {
        self.record_optimized_side_exit_profile(key, guard, None)
    }

    pub fn record_optimized_side_exit_profile(
        &mut self,
        key: FunctionKey,
        guard: u32,
        observed: Option<ObservedType>,
    ) -> SideExitAction {
        self.record_deopt(true);
        let observation_key = (key, guard);
        let observation_changed = self
            .side_exit_observations
            .get(&observation_key)
            .is_some_and(|prior| *prior != observed);
        self.side_exit_observations
            .entry(observation_key)
            .or_insert(observed);
        let side_path_pending = self.functions.get(&key).is_some_and(|function| {
            matches!(
                function.optimizing.state,
                CompileState::Queued(_) | CompileState::Compiling(_)
            )
        });
        let exits = self.side_exits.entry(key).or_default();
        let count = exits.entry(guard).or_default();
        *count = count.saturating_add(1);
        let count = *count;
        // A stable guard gets one opportunity to compile a side path at ten
        // exits. If no replacement is pending, stability alone must not keep
        // failing code published forever (e.g. repeated Object shape misses).
        // Let authorized work finish: ten fast calls must not cancel a valid
        // side path just because its compiler worker has not been scheduled.
        if exits.len() > 1 || observation_changed || (count >= 20 && !side_path_pending) {
            let function = self.functions.entry(key).or_default();
            function.instability_attempts = function.instability_attempts.saturating_add(1);
            let attempts = function.instability_attempts;
            let delay = 1u64
                .checked_shl(u32::from(attempts.min(20)))
                .unwrap_or(u64::MAX);
            let retry_after = self.clock.saturating_add(delay);
            function.optimizing.state = if attempts >= self.max_attempts {
                self.metrics.blacklisted = self.metrics.blacklisted.saturating_add(1);
                CompileState::Blacklisted
            } else {
                CompileState::Backoff {
                    attempts,
                    retry_after,
                }
            };
            self.last_benefit_target = None;
            self.installed_keys.remove(&(key, Tier::Optimizing));
            if let Some(function) = self.functions.get_mut(&key) {
                function.published = self
                    .installed_keys
                    .contains_key(&(key, Tier::Baseline))
                    .then_some(Tier::Baseline);
            }
            self.metrics.optimized_demotions = self.metrics.optimized_demotions.saturating_add(1);
            SideExitAction::Demote { retry_after }
        } else if count == 10 {
            SideExitAction::StablePathThreshold
        } else {
            SideExitAction::Counted
        }
    }

    /// Atomically returns an installed optimizing tier to a queueable state
    /// while preserving its baseline deopt target.
    pub fn prepare_stable_path_recompile(&mut self, key: FunctionKey) -> bool {
        self.last_benefit_target = None;
        if !self.installed_keys.contains_key(&(key, Tier::Baseline))
            || self
                .installed_keys
                .remove(&(key, Tier::Optimizing))
                .is_none()
        {
            return false;
        }
        if let Some(function) = self.functions.get_mut(&key) {
            function.optimizing.state = CompileState::Cold;
            function.published = Some(Tier::Baseline);
        }
        true
    }

    pub fn pin(&mut self, key: FunctionKey, tier: Tier) -> Option<ExecutionPin> {
        let artifact_key = *self.installed_keys.get(&(key, tier))?;
        let pin = self.cache.pin(artifact_key)?;
        #[cfg(feature = "compiler")]
        if tier == Tier::Optimizing
            && pin.artifact().optimized_metadata().is_some_and(|metadata| {
                self.latest_feedback_epochs.get(&key) != Some(&metadata.feedback_epoch())
            })
        {
            return None;
        }
        Some(pin)
    }

    pub fn cache_len(&self) -> usize {
        self.cache.len()
    }

    pub fn cache_bytes(&self) -> usize {
        self.cache.charged_bytes()
    }

    pub fn poll_cache_reclamation(&mut self) -> usize {
        self.cache.poll_reclamation()
    }

    pub fn completion_sender(&self) -> CompletionSender {
        CompletionSender {
            sender: self.completion_sender.clone(),
            signals: Arc::clone(&self.completion_signals),
        }
    }

    /// Applies worker completions on the caller's runtime-locked coordinator.
    /// True when a worker completion or a code-cache reclamation is waiting
    /// to be applied. This is a pair of relaxed atomic loads, cheap enough
    /// for the per-entry native path to decide whether maintenance is due.
    pub fn has_pending_work(&self) -> bool {
        self.completion_signals.pending.load(Ordering::Acquire) != 0
            || self.cache.reclamation_requested()
    }

    /// Number of installed artifacts without cloning the metrics snapshot.
    pub fn installed_count(&self) -> u64 {
        self.metrics.installed
    }

    pub fn drain_completions(&mut self) -> CompletionDrain {
        self.drain_completions_with_budget(DEFAULT_COMPLETION_DRAIN_BUDGET)
    }

    /// Applies at most `budget` worker completions without allocating an intermediate queue.
    pub fn drain_completions_with_budget(&mut self, budget: usize) -> CompletionDrain {
        let reclamation = self.cache.poll_reclamation_with_budget(budget);
        let reclaimed = reclamation.reclaimed();
        let completion_budget = budget.saturating_sub(reclaimed);
        let mut drained = 0;
        while drained < completion_budget {
            let completion = self
                .completion_receiver
                .as_ref()
                .and_then(|receiver| receiver.try_recv().ok());
            let Some(completion) = completion else {
                break;
            };
            self.completion_signals.decrement_pending();
            self.complete(completion);
            drained += 1;
        }
        CompletionDrain {
            drained,
            reclaimed,
            may_have_remaining: reclamation.may_have_remaining()
                || self.cache.reclamation_requested()
                || self.completion_signals.pending.load(Ordering::Acquire) != 0,
        }
    }

    pub fn shutdown(&mut self) {
        if self.shutdown {
            return;
        }
        self.shutdown = true;
        self.queue.clear();
        self.in_flight.clear();
        self.completion_receiver.take();
        self.completion_sender.take();
        self.completion_signals.pending.store(0, Ordering::Release);
        let functions = self.functions.keys().copied().collect::<Vec<_>>();
        for key in functions {
            self.retire_state(key);
        }
        self.cache.collect_invalidated();
    }
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::{
        bytecode::{opcode, CompileSnapshot, VerifyLimits},
        compiler::{mock::FakeCompiler, Compiler},
    };

    fn snapshot() -> VerifiedFunction {
        CompileSnapshot::from_untrusted_bytecode(vec![opcode::RETURN_UNDEF], 0, 0, 0, 0)
            .verify(VerifyLimits::default())
            .unwrap()
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    fn frame_inline_fixture(
        calls: usize,
        conflicting_target: bool,
    ) -> (
        Coordinator,
        CompileRequest,
        FunctionKey,
        ArtifactKey,
        CompileSnapshot,
    ) {
        use super::super::FeedbackTable;
        use crate::compiler::baseline::BaselineCompiler;
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        let ((callee_snapshot, _callee_roots), (caller_snapshot, _caller_roots)) =
            context.with(|ctx| {
                let callee: rquickjs::Value = ctx.eval("(function(a){return a})").unwrap();
                let source = format!("(function(f,a){{{}return a}})", "f(a);".repeat(calls));
                let caller: rquickjs::Value = ctx.eval(source).unwrap();
                unsafe {
                    (
                        CompileSnapshot::capture_with_runtime_constants(
                            &runtime,
                            ctx.as_raw().as_ptr(),
                            callee.as_raw(),
                        )
                        .unwrap(),
                        CompileSnapshot::capture_with_runtime_constants(
                            &runtime,
                            ctx.as_raw().as_ptr(),
                            caller.as_raw(),
                        )
                        .unwrap(),
                    )
                }
            });
        let callee = FunctionKey::new(callee_snapshot.function_id(), callee_snapshot.generation());
        let caller = FunctionKey::new(caller_snapshot.function_id(), caller_snapshot.generation());
        let callee_body = callee_snapshot.verify(VerifyLimits::default()).unwrap();
        let caller_body = caller_snapshot.verify(VerifyLimits::default()).unwrap();
        let mut coordinator = Coordinator::with_limits(8, 8, 4, 1 << 20);
        let mut callee_artifact = None;
        for (key, body) in [(callee, callee_body), (caller, caller_body.clone())] {
            coordinator.queue(key, Tier::Baseline, body).unwrap();
            let request = coordinator.begin_next().unwrap();
            let artifact_key = request.artifact_key();
            let attempt_id = request.attempt_id();
            let artifact = Compiler::compile(&BaselineCompiler::host(), request).unwrap();
            if key == callee {
                assert!(artifact.inline_snapshot().is_some());
                assert!(artifact.direct_call_published().is_none());
                callee_artifact = Some(artifact_key);
            }
            coordinator.complete(CompileCompletion {
                key,
                requested_tier: Tier::Baseline,
                artifact_key,
                attempt_id,
                result: Ok(artifact),
            });
            assert_eq!(
                coordinator.state(key),
                CompileState::Installed(Tier::Baseline)
            );
        }
        let mut feedback = FeedbackTable::new(128, 2);
        for instruction in caller_body
            .instructions()
            .iter()
            .filter(|i| i.opcode().name() == "call1")
        {
            for (arg, result) in [
                (ObservedType::Object, ObservedType::Object),
                (ObservedType::Bool, ObservedType::Int32),
            ] {
                feedback.observe_call_signature_with_identity(
                    caller,
                    instruction.pc(),
                    callee,
                    0x1234,
                    0x5678,
                    &[arg],
                    result,
                );
            }
            if conflicting_target {
                feedback.observe_call_signature_with_identity(
                    caller,
                    instruction.pc(),
                    callee,
                    0x4321,
                    0x5678,
                    &[ObservedType::Object],
                    ObservedType::Object,
                );
            }
        }
        coordinator
            .queue_with_feedback(caller, Tier::Optimizing, caller_body, feedback.snapshot(31))
            .unwrap();
        let request = coordinator.begin_next().unwrap();
        (
            coordinator,
            request,
            callee,
            callee_artifact.unwrap(),
            callee_snapshot,
        )
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    #[test]
    fn frame_inline_retains_actual_tagged_callee_without_scalar_entry() {
        let (mut coordinator, request, callee, artifact, body) = frame_inline_fixture(1, false);
        assert!(request.direct_call_targets().is_empty());
        assert_eq!(request.frame_inline_targets().len(), 1);
        let retained = request.frame_inline_targets()[0].clone();
        assert_eq!(retained.target().callee(), callee);
        assert_eq!(retained.artifact_key(), artifact);
        assert_eq!(retained.snapshot().bytecode(), body.bytecode());
        assert_eq!(
            request.snapshot_bytes(),
            request.snapshot().snapshot().owned_bytes() + body.retained_bytes()
        );
        coordinator.retire(callee);
        drop(coordinator);
        drop(request);
        retained.snapshot().verify(VerifyLimits::default()).unwrap();
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    #[test]
    fn frame_inline_conflicting_closure_keeps_generic_call() {
        let (_, request, _, _, _) = frame_inline_fixture(1, true);
        assert!(request.frame_inline_targets().is_empty());
        assert!(request.direct_call_targets().is_empty());
        assert_eq!(
            request.snapshot_bytes(),
            request.snapshot().snapshot().owned_bytes()
        );
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    #[test]
    fn frame_inline_budget_cancels_oversized_candidates_before_unbounded_verification() {
        let oversized_slots = CompileSnapshot::from_untrusted_bytecode(
            vec![opcode::RETURN_UNDEF],
            (FRAME_INLINE_MAX_SLOTS + 1) as u16,
            0,
            0,
            0,
        );
        let oversized_snapshot = CompileSnapshot::from_untrusted_bytecode(
            vec![opcode::RETURN_UNDEF; FRAME_INLINE_MAX_RETAINED_BYTES],
            0,
            0,
            0,
            0,
        );
        let mut oversized_instructions = vec![opcode::NOP; FRAME_INLINE_MAX_INSTRUCTIONS];
        oversized_instructions.push(opcode::RETURN_UNDEF);
        let oversized_work =
            CompileSnapshot::from_untrusted_bytecode(oversized_instructions, 0, 0, 0, 0);

        for snapshot in [oversized_slots, oversized_snapshot, oversized_work] {
            let mut budget = FrameInlineBudget::new(FRAME_INLINE_MAX_RETAINED_BYTES);
            assert!(budget.begin_candidate());
            assert!(budget.verify(&snapshot).is_none());
            assert!(budget.exhausted());
            assert_eq!(budget.sites_left, FRAME_INLINE_MAX_SITES - 1);
        }
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    #[test]
    fn frame_inline_budget_accepts_two_legal_half_budget_bodies() {
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        let parameters = (0..21)
            .map(|index| format!("a{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let locals = (0..42)
            .map(|index| format!("let x{index};"))
            .collect::<String>();
        let snapshot = context.with(|ctx| {
            let function: rquickjs::Value = ctx
                .eval(format!("(function({parameters}){{{locals}return 0;}})"))
                .unwrap();
            unsafe {
                CompileSnapshot::capture_with_runtime_constants(
                    &runtime,
                    ctx.as_raw().as_ptr(),
                    function.as_raw(),
                )
                .unwrap()
                .0
            }
        });
        let slots = usize::from(snapshot.arg_count())
            + usize::from(snapshot.local_count())
            + usize::from(snapshot.stack_size());
        assert_eq!(snapshot.decode().unwrap().len(), 128);
        assert_eq!(slots, 64);
        let mut budget = FrameInlineBudget::new(FRAME_INLINE_MAX_RETAINED_BYTES);

        assert!(budget.begin_candidate());
        let parent = budget.verify(&snapshot).unwrap();
        assert!(budget.claim(&parent));
        assert_eq!(budget.instructions_left, 128);
        assert_eq!(budget.slots_left, 64);

        assert!(budget.begin_candidate());
        let limits = budget.verification_limits(&snapshot).unwrap();
        // A legal body may place all 64 slots in locals. Even one block then
        // charges 64 initial locals, a 64-cell entry, and 128 instruction
        // states of 65 units each.
        assert!(limits.max_work_units >= 8_448);
        let child = budget
            .verify(&snapshot)
            .expect("the second 128-instruction/64-slot body remains legal");
        assert!(budget.claim(&child));
        assert_eq!(budget.instructions_left, 0);
        assert_eq!(budget.slots_left, 0);
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    #[test]
    fn frame_inline_budget_allows_zero_slot_child_after_slots_are_spent() {
        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        let parameters = (0..FRAME_INLINE_MAX_SLOTS)
            .map(|index| format!("a{index}"))
            .collect::<Vec<_>>()
            .join(",");
        let (full_slots, zero_slots) = context.with(|ctx| {
            let parent: rquickjs::Value =
                ctx.eval(format!("(function({parameters}){{}})")).unwrap();
            let child: rquickjs::Value = ctx.eval("(function(){})").unwrap();
            unsafe {
                (
                    CompileSnapshot::capture_with_runtime_constants(
                        &runtime,
                        ctx.as_raw().as_ptr(),
                        parent.as_raw(),
                    )
                    .unwrap()
                    .0,
                    CompileSnapshot::capture_with_runtime_constants(
                        &runtime,
                        ctx.as_raw().as_ptr(),
                        child.as_raw(),
                    )
                    .unwrap()
                    .0,
                )
            }
        });
        assert_eq!(full_slots.arg_count(), FRAME_INLINE_MAX_SLOTS as u16);
        assert_eq!(full_slots.local_count(), 0);
        assert_eq!(full_slots.stack_size(), 0);
        assert_eq!(zero_slots.arg_count(), 0);
        assert_eq!(zero_slots.local_count(), 0);
        assert_eq!(zero_slots.stack_size(), 0);
        assert_eq!(
            zero_slots.decode().unwrap()[0].opcode().name(),
            "return_undef"
        );
        let mut budget = FrameInlineBudget::new(FRAME_INLINE_MAX_RETAINED_BYTES);

        assert!(budget.begin_candidate());
        let parent = budget.verify(&full_slots).unwrap();
        assert!(budget.claim(&parent));
        assert_eq!(budget.slots_left, 0);

        assert!(budget.begin_candidate());
        let child = budget
            .verify(&zero_slots)
            .expect("a zero-slot child does not exceed an exhausted slot budget");
        assert!(budget.claim(&child));
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    #[test]
    fn frame_inline_retention_is_bounded_and_discardable() {
        let (_, mut request, _, _, body) = frame_inline_fixture(20, false);
        assert_eq!(request.frame_inline_targets().len(), 16);
        assert_eq!(
            request.snapshot_bytes(),
            request.snapshot().snapshot().owned_bytes() + 16 * body.retained_bytes()
        );
        request.discard_inline_snapshots();
        assert!(request.frame_inline_targets().is_empty());
        assert_eq!(
            request.snapshot_bytes(),
            request.snapshot().snapshot().owned_bytes()
        );
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    #[test]
    fn frame_inline_target_tree_accounts_and_discards_descendants() {
        let (_, request, _, artifact, body) = frame_inline_fixture(1, false);
        let leaf = request.frame_inline_targets()[0].clone();
        let parent = FrameInlineTarget {
            target: *leaf.target(),
            artifact_key: artifact,
            snapshot: body.clone(),
            children: vec![leaf].into(),
        };
        let root = FrameInlineTarget {
            target: *parent.target(),
            artifact_key: artifact,
            snapshot: body.clone(),
            children: vec![parent].into(),
        };
        assert_eq!(root.descendants().count(), 3);
        assert_eq!(root.retained_bytes(), 3 * body.retained_bytes());
        assert_eq!(root.dependencies().count(), 3);

        let mut request = request;
        request.frame_inline_targets = vec![root].into();
        assert_eq!(
            request.snapshot_bytes(),
            request.snapshot().snapshot().owned_bytes() + 3 * body.retained_bytes()
        );
        request.discard_inline_snapshots();
        assert!(request.frame_inline_targets().is_empty());
        assert_eq!(
            request.snapshot_bytes(),
            request.snapshot().snapshot().owned_bytes()
        );
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    #[test]
    fn frame_inline_enqueue_builds_nested_tree_and_rejects_ancestor_cycle() {
        use super::super::FeedbackTable;
        use crate::compiler::baseline::BaselineCompiler;

        let runtime = rquickjs::Runtime::new().unwrap();
        let context = rquickjs::Context::full(&runtime).unwrap();
        let snapshots = context.with(|ctx| {
            [
                "(function(a){return a})",
                "(function(g,a){let x=g(a);return x})",
                "(function(f,a){let x=f(a);return x})",
            ]
            .map(|source| {
                let function: rquickjs::Value = ctx.eval(source).unwrap();
                unsafe {
                    CompileSnapshot::capture_with_runtime_constants(
                        &runtime,
                        ctx.as_raw().as_ptr(),
                        function.as_raw(),
                    )
                    .unwrap()
                }
            })
        });
        let keys: [FunctionKey; 3] = std::array::from_fn(|index| {
            FunctionKey::new(
                snapshots[index].0.function_id(),
                snapshots[index].0.generation(),
            )
        });
        let bodies =
            snapshots.map(|(snapshot, _roots)| snapshot.verify(VerifyLimits::default()).unwrap());
        let mut coordinator = Coordinator::with_limits(8, 8, 4, 1 << 20);
        for index in 0..3 {
            coordinator
                .queue(keys[index], Tier::Baseline, bodies[index].clone())
                .unwrap();
            let request = coordinator.begin_next().unwrap();
            let artifact_key = request.artifact_key();
            let attempt_id = request.attempt_id();
            let artifact = Compiler::compile(&BaselineCompiler::host(), request).unwrap();
            coordinator.complete(CompileCompletion {
                key: keys[index],
                requested_tier: Tier::Baseline,
                artifact_key,
                attempt_id,
                result: Ok(artifact),
            });
        }

        let call_pc = |body: &VerifiedFunction| {
            body.instructions()
                .iter()
                .find(|instruction| {
                    matches!(
                        instruction.opcode().name(),
                        "call" | "call0" | "call1" | "call2" | "call3"
                    )
                })
                .unwrap()
                .pc()
        };
        let mut feedback = FeedbackTable::new(128, 2);
        let mut observe_link = |caller: FunctionKey, pc: u32, callee: FunctionKey| {
            for (argument, result) in [
                (ObservedType::Object, ObservedType::Object),
                (ObservedType::Bool, ObservedType::Int32),
            ] {
                feedback.observe_call_signature_with_identity(
                    caller,
                    pc,
                    callee,
                    0x1234,
                    0x5678,
                    &[argument],
                    result,
                );
            }
        };
        observe_link(keys[2], call_pc(&bodies[2]), keys[1]);
        observe_link(keys[1], call_pc(&bodies[1]), keys[0]);
        coordinator
            .queue_with_feedback(
                keys[2],
                Tier::Optimizing,
                bodies[2].clone(),
                feedback.snapshot(41),
            )
            .unwrap();
        let request = coordinator.begin_next().unwrap();
        assert_eq!(request.frame_inline_targets().len(), 1);
        let root = &request.frame_inline_targets()[0];
        assert_eq!(root.target().callee(), keys[1]);
        assert_eq!(root.children().len(), 1);
        assert_eq!(root.children()[0].target().callee(), keys[0]);
        assert_eq!(root.descendants().count(), 2);

        // A directly constructed cycle exercises the same ancestor rule used
        // by queue construction without retaining the root body a second time.
        let mut budget = FrameInlineBudget::new(FRAME_INLINE_MAX_RETAINED_BYTES);
        let mut ancestors = vec![keys[1], keys[0]];
        let cycle = coordinator.retain_frame_inline_target(
            keys[1],
            call_pc(&bodies[1]),
            request.feedback(),
            &mut budget,
            &mut ancestors,
            2,
        );
        assert!(cycle.is_none());
        assert_eq!(budget.sites_left, FRAME_INLINE_MAX_SITES - 1);
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    #[test]
    fn frame_inline_retired_dependency_cannot_publish_in_flight_caller() {
        for retired in [false, true] {
            let (mut coordinator, request, callee, _, _) = frame_inline_fixture(1, false);
            let key = request.key();
            let artifact = CompiledArtifact::fake(Tier::Optimizing)
                .bind_fake(request.artifact_key())
                .with_dependencies(vec![crate::code_cache::ArtifactDependency::new(callee)]);
            if retired {
                coordinator.retire(callee);
            }
            coordinator.complete(CompileCompletion {
                key,
                requested_tier: request.tier(),
                artifact_key: request.artifact_key(),
                attempt_id: request.attempt_id(),
                result: Ok(artifact),
            });
            assert_eq!(
                coordinator.pin(key, Tier::Optimizing).is_none(),
                retired,
                "retired={retired}, state={:?}, metrics={:?}",
                coordinator.state(key),
                coordinator.metrics()
            );
        }
    }

    #[test]
    fn retiring_a_dispatched_generation_marks_its_request_retired() {
        let mut coordinator = Coordinator::with_limits(4, 4, 4, 1 << 20);
        let retired = FunctionKey::new(51, 1);
        let live = FunctionKey::new(52, 1);
        coordinator
            .queue(retired, Tier::Baseline, snapshot())
            .unwrap();
        coordinator.queue(live, Tier::Baseline, snapshot()).unwrap();
        let retired_request = coordinator.begin_next().unwrap();
        let live_request = coordinator.begin_next().unwrap();
        assert!(!retired_request.is_retired());
        coordinator.retire(retired);
        assert!(retired_request.is_retired());
        assert!(!live_request.is_retired());
        coordinator.complete(CompileCompletion {
            key: retired,
            requested_tier: Tier::Baseline,
            artifact_key: retired_request.artifact_key(),
            attempt_id: retired_request.attempt_id(),
            result: Err(CompileFailure::Cancelled),
        });
        assert_eq!(coordinator.metrics().cancelled_compilations, 0);
        assert!(coordinator.in_flight_retired.contains_key(&live));
        assert!(!coordinator.in_flight_retired.contains_key(&retired));
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn completion_disposition_records_the_exact_publication_boundary_per_function() {
        let mut coordinator = Coordinator::with_limits(4, 4, 4, 1 << 20);
        let dependency = FunctionKey::new(41, 1);
        let caller = FunctionKey::new(42, 1);

        coordinator
            .queue(dependency, Tier::Baseline, snapshot())
            .unwrap();
        let dependency_request = coordinator.begin_next().unwrap();
        let dependency_artifact =
            CompiledArtifact::fake(Tier::Baseline).bind_fake(dependency_request.artifact_key());
        coordinator.complete(CompileCompletion {
            key: dependency,
            requested_tier: Tier::Baseline,
            artifact_key: dependency_request.artifact_key(),
            attempt_id: dependency_request.attempt_id(),
            result: Ok(dependency_artifact),
        });
        assert_eq!(
            coordinator.test_completion_disposition(dependency),
            Some(CompletionDisposition::Installed)
        );

        coordinator
            .queue(caller, Tier::Baseline, snapshot())
            .unwrap();
        let caller_request = coordinator.begin_next().unwrap();
        let caller_artifact = CompiledArtifact::fake(Tier::Baseline)
            .bind_fake(caller_request.artifact_key())
            .with_dependencies(vec![crate::code_cache::ArtifactDependency::new(dependency)]);
        let replacement = FunctionKey::new(dependency.id, 2);
        coordinator
            .queue(replacement, Tier::Baseline, snapshot())
            .unwrap();
        coordinator.complete(CompileCompletion {
            key: caller,
            requested_tier: Tier::Baseline,
            artifact_key: caller_request.artifact_key(),
            attempt_id: caller_request.attempt_id(),
            result: Ok(caller_artifact),
        });
        assert_eq!(
            coordinator.test_completion_disposition(caller),
            Some(CompletionDisposition::DependencyGenerationMismatch {
                dependency,
                current_generation: Some(2),
            })
        );
    }

    #[test]
    fn prioritized_request_is_taken_ahead_of_earlier_requests() {
        let mut coordinator = Coordinator::with_limits(4, 4, 4, 1 << 20);
        let first = FunctionKey::new(1, 1);
        let second = FunctionKey::new(2, 1);
        let kernel = FunctionKey::new(3, 1);
        // Tier2 requires an installed Baseline.
        coordinator
            .queue(kernel, Tier::Baseline, snapshot())
            .unwrap();
        let request = coordinator.begin_next().unwrap();
        coordinator.complete(CompileCompletion {
            key: kernel,
            requested_tier: Tier::Baseline,
            artifact_key: request.artifact_key(),
            attempt_id: request.attempt_id(),
            result: Ok(CompiledArtifact::fake(Tier::Baseline).bind_fake(request.artifact_key())),
        });
        coordinator
            .queue(first, Tier::Baseline, snapshot())
            .unwrap();
        coordinator
            .queue(second, Tier::Baseline, snapshot())
            .unwrap();
        coordinator
            .queue(kernel, Tier::Optimizing, snapshot())
            .unwrap();
        assert!(coordinator.prioritize(kernel, Tier::Optimizing));
        assert!(!coordinator.prioritize(FunctionKey::new(9, 1), Tier::Optimizing));

        let order: Vec<_> = std::iter::from_fn(|| coordinator.begin_next())
            .map(|request| (request.key(), request.tier()))
            .collect();
        assert_eq!(
            order,
            [
                (kernel, Tier::Optimizing),
                (first, Tier::Baseline),
                (second, Tier::Baseline),
            ]
        );
    }

    #[test]
    fn measured_tier2_loss_only_demotes_onto_an_installed_baseline() {
        fn install(coordinator: &mut Coordinator, key: FunctionKey, tier: Tier) {
            coordinator.queue(key, tier, snapshot()).unwrap();
            let request = coordinator.begin_next().unwrap();
            coordinator.complete(CompileCompletion {
                key,
                requested_tier: tier,
                artifact_key: request.artifact_key(),
                attempt_id: request.attempt_id(),
                result: Ok(CompiledArtifact::fake(tier).bind_fake(request.artifact_key())),
            });
        }

        let mut coordinator = Coordinator::with_limits(4, 4, 4, 1 << 20);
        let backed = FunctionKey::new(7, 1);
        install(&mut coordinator, backed, Tier::Baseline);
        install(&mut coordinator, backed, Tier::Optimizing);
        assert!(coordinator.demote_unprofitable_optimized(backed));
        assert!(coordinator.pin(backed, Tier::Optimizing).is_none());
        assert!(coordinator.pin(backed, Tier::Baseline).is_some());
        assert!(matches!(
            coordinator.tier_state(backed, Tier::Optimizing),
            CompileState::Backoff { attempts: 1, .. }
        ));
        assert_eq!(coordinator.metrics().optimized_demotions, 1);
        assert_eq!(coordinator.metrics().blacklisted, 0);

        // The interpreter-only function keeps its bounded Tier-2 trial: the
        // timing it lost to is not the tier it would fall back to.
        let demoted = FunctionKey::new(8, 1);
        install(&mut coordinator, demoted, Tier::Baseline);
        assert!(coordinator.demote_baseline_to_interpreter(demoted));
        install(&mut coordinator, demoted, Tier::Optimizing);
        assert!(coordinator.pin(demoted, Tier::Optimizing).is_some());
        assert!(!coordinator.demote_unprofitable_optimized(demoted));
        assert!(coordinator.pin(demoted, Tier::Optimizing).is_some());
        assert_eq!(
            coordinator.tier_state(demoted, Tier::Optimizing),
            CompileState::Installed(Tier::Optimizing)
        );
        assert_eq!(coordinator.metrics().optimized_demotions, 1);
        assert_eq!(coordinator.metrics().blacklisted, 1);
        assert_eq!(coordinator.metrics().interpreter_demotions, 1);
    }

    #[test]
    fn publication_reports_when_backend_owned_fields_need_restoring() {
        let mut coordinator = Coordinator::with_limits(1, 1, 4, 3);
        let mut published = JitMetrics::disabled();
        assert!(coordinator.refresh_published_metrics(&mut published));
        published.snapshot_requests = 17;
        coordinator.record_tier2_entry();
        assert!(!coordinator.refresh_published_metrics(&mut published));
        assert_eq!(published.snapshot_requests, 17);
        assert_eq!(published.tier2_entries, 1);
        coordinator.record_deopt(true);
        assert!(coordinator.refresh_published_metrics(&mut published));
        assert_eq!(published.snapshot_requests, 0);
        assert_eq!(published.deopts, 1);
    }

    #[test]
    fn published_metrics_remain_exact_across_compilation_and_native_exits() {
        fn check(coordinator: &mut Coordinator, published: &mut JitMetrics) {
            coordinator.refresh_published_metrics(published);
            assert_eq!(*published, coordinator.metrics());
        }
        let mut coordinator = Coordinator::with_limits(1, 1, 4, 3);
        let mut published = JitMetrics::disabled();
        check(&mut coordinator, &mut published);
        let key = FunctionKey::new(1, 1);
        coordinator.queue(key, Tier::Baseline, snapshot()).unwrap();
        check(&mut coordinator, &mut published);
        let request = coordinator.begin_next().unwrap();
        check(&mut coordinator, &mut published);
        coordinator.complete(CompileCompletion {
            key,
            requested_tier: request.tier,
            artifact_key: request.artifact_key,
            attempt_id: request.attempt_id,
            result: Err(CompileFailure::UnsupportedOpcode),
        });
        check(&mut coordinator, &mut published);
        for index in 0..128 {
            coordinator.record_tier2_entry();
            coordinator.record_side_path_entries(index % 3);
            check(&mut coordinator, &mut published);
        }
        coordinator.record_deopt(true);
        coordinator.set_worker_usage(2, 128, 256);
        check(&mut coordinator, &mut published);
        coordinator.record_tier2_entry();
        check(&mut coordinator, &mut published);
        assert_eq!(published.tier2_entries, 129);
        assert_eq!(published.deopts, 1);
        assert_eq!(published.compile_failures, 1);
    }

    #[test]
    fn mock_compiler_waits_for_release_before_submitting_completion() {
        let mut coordinator = Coordinator::with_limits(1, 1, 4, 3);
        let key = FunctionKey::new(1, 1);
        coordinator.queue(key, Tier::Baseline, snapshot()).unwrap();
        let request = coordinator.begin_next().unwrap();
        let sender = coordinator.completion_sender();
        let (compiler, control) = FakeCompiler::new(1);
        let compiler = Arc::new(compiler);
        let worker_compiler = Arc::clone(&compiler);
        let worker = std::thread::spawn(move || {
            compile_and_send(worker_compiler.as_ref(), request, &sender).unwrap()
        });

        let observed = control.next_request().expect("worker reached compiler");
        assert_eq!(observed.key(), key);
        assert_eq!(coordinator.drain_completions().drained(), 0);
        control.complete(CompiledArtifact::fake(Tier::Baseline));
        worker.join().unwrap();

        assert_eq!(coordinator.drain_completions().drained(), 1);
        assert_eq!(
            coordinator.state(key),
            CompileState::Installed(Tier::Baseline)
        );
    }

    struct PanickingCompiler;

    impl Compiler for PanickingCompiler {
        fn compile(&self, _request: CompileRequest) -> Result<CompiledArtifact, CompileFailure> {
            panic!("deterministic compiler panic")
        }
    }

    #[test]
    fn compiler_panic_becomes_failure_completion_without_unwinding() {
        let mut coordinator = Coordinator::with_limits(1, 1, 4, 3);
        let key = FunctionKey::new(1, 1);
        coordinator.queue(key, Tier::Baseline, snapshot()).unwrap();
        let request = coordinator.begin_next().unwrap();

        compile_and_send(
            &PanickingCompiler,
            request,
            &coordinator.completion_sender(),
        )
        .unwrap();
        assert_eq!(coordinator.drain_completions().drained(), 1);
        assert!(matches!(
            coordinator.state(key),
            CompileState::Backoff { attempts: 1, .. }
        ));
        assert_eq!(coordinator.metrics().compile_failures, 1);
    }
}
