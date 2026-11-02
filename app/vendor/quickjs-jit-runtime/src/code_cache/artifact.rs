use std::sync::atomic::{AtomicU64, Ordering};

use crate::runtime::{FunctionKey, Tier};

/// Full compatibility identity for one compiled artifact.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtifactKey {
    pub runtime_id: u64,
    pub function_id: u64,
    pub generation: u64,
    pub tier: Tier,
    pub target_isa: u64,
    pub cpu_features: u64,
    pub abi_fingerprint: u64,
    pub source_revision: u64,
    pub opcode_fingerprint: u64,
    pub config_fingerprint: u64,
    pub specialization_fingerprint: u64,
}

/// Identity for an optimized primary version plus one specialized call edge.
/// Registration does not imply that a direct compiled call path exists.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ArtifactVersionIdentity {
    primary_fingerprint: u64,
    call_fingerprint: u64,
    fingerprint: u64,
}

impl ArtifactVersionIdentity {
    pub(crate) fn new(primary_fingerprint: u64, call_fingerprint: u64) -> Self {
        const SUBSTITUTE: u64 = 0x9e37_79b9_7f4a_7c15;
        let primary_fingerprint = if primary_fingerprint == 0 {
            SUBSTITUTE
        } else {
            primary_fingerprint
        };
        let call_fingerprint = if call_fingerprint == 0 {
            SUBSTITUTE
        } else {
            call_fingerprint
        };
        let combined =
            (primary_fingerprint ^ call_fingerprint.rotate_left(29)).wrapping_mul(0x100_0000_01b3);
        Self {
            primary_fingerprint,
            call_fingerprint,
            fingerprint: if combined == 0 { SUBSTITUTE } else { combined },
        }
    }
    pub const fn primary_fingerprint(self) -> u64 {
        self.primary_fingerprint
    }
    pub const fn call_fingerprint(self) -> u64 {
        self.call_fingerprint
    }
    pub const fn fingerprint(self) -> u64 {
        self.fingerprint
    }
}

impl ArtifactKey {
    pub const fn with_tier(mut self, tier: Tier) -> Self {
        self.tier = tier;
        if matches!(tier, Tier::Baseline) {
            self.specialization_fingerprint = 0;
        }
        self
    }

    pub const fn with_version_identity(mut self, identity: ArtifactVersionIdentity) -> Self {
        self.specialization_fingerprint = identity.fingerprint();
        self
    }
}

#[derive(Debug, Eq, PartialEq)]
pub struct CodeAllocation {
    bytes: Box<[u8]>,
}

impl CodeAllocation {
    pub fn inert(bytes: Vec<u8>) -> Self {
        Self {
            bytes: bytes.into_boxed_slice(),
        }
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub const fn is_executable(&self) -> bool {
        false
    }
}

/// Exact relocation encodings emitted by Cranelift 0.116.
///
/// Keeping the encoding in the artifact prevents the publisher from silently
/// treating target-relative relocations as absolute addresses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RelocationKind {
    Abs4,
    Abs8,
    X86PCRel4,
    X86CallPCRel4,
    X86CallPLTRel4,
    X86GOTPCRel4,
    X86SecRel,
    Arm32Call,
    Arm64Call,
    S390xPCRel32Dbl,
    S390xPLTRel32Dbl,
    ElfX86_64TlsGd,
    MachOX86_64Tlv,
    MachOAarch64TlsAdrPage21,
    MachOAarch64TlsAdrPageOff12,
    Aarch64TlsDescAdrPage21,
    Aarch64TlsDescLd64Lo12,
    Aarch64TlsDescAddLo12,
    Aarch64TlsDescCall,
    Aarch64AdrGotPage21,
    Aarch64Ld64GotLo12Nc,
    RiscvCallPlt,
    RiscvTlsGdHi20,
    RiscvPCRelLo12I,
    RiscvGotHi20,
    S390xTlsGd64,
    S390xTlsGdCall,
    PulleyCallIndirectHost,
}

/// A relocation target retained symbolically until a writable allocation has
/// a stable address and the runtime has resolved external symbols.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum RelocationTarget {
    Absolute(u64),
    FunctionOffset(u32),
    Symbol(Box<str>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Relocation {
    pub offset: u32,
    pub kind: RelocationKind,
    pub target: RelocationTarget,
    pub addend: i64,
}

impl Relocation {
    /// Backwards-compatible constructor for an absolute 64-bit relocation.
    pub const fn new(offset: u32, target: u64, addend: i64) -> Self {
        Self {
            offset,
            kind: RelocationKind::Abs8,
            target: RelocationTarget::Absolute(target),
            addend,
        }
    }

    pub const fn with_target(
        offset: u32,
        kind: RelocationKind,
        target: RelocationTarget,
        addend: i64,
    ) -> Self {
        Self {
            offset,
            kind,
            target,
            addend,
        }
    }

    /// Resolves the symbolic target without discarding the relocation kind.
    pub fn resolve_with(
        &self,
        mut resolve: impl FnMut(&RelocationTarget) -> Option<u64>,
    ) -> Result<ResolvedRelocation, RelocationResolveError> {
        let target = match self.target {
            RelocationTarget::Absolute(target) => target,
            _ => resolve(&self.target).ok_or(RelocationResolveError::UnresolvedTarget)?,
        };
        Ok(ResolvedRelocation::new(
            self.offset,
            self.kind,
            target,
            self.addend,
        ))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RelocationResolveError {
    UnresolvedTarget,
}

/// A relocation whose target address has been explicitly resolved and is
/// ready for validation by the W^X publisher.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedRelocation {
    pub offset: u32,
    pub kind: RelocationKind,
    pub target: u64,
    pub addend: i64,
}

impl ResolvedRelocation {
    pub const fn new(offset: u32, kind: RelocationKind, target: u64, addend: i64) -> Self {
        Self {
            offset,
            kind,
            target,
            addend,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackMap {
    pub code_offset: u32,
    pub live_slots: Box<[u16]>,
}

impl StackMap {
    pub fn new(code_offset: u32, live_slots: Vec<u16>) -> Self {
        Self {
            code_offset,
            live_slots: live_slots.into_boxed_slice(),
        }
    }
}

/// Native unwind format represented by serialized Cranelift 0.116 metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum UnwindKind {
    SystemV,
    WindowsX64,
    WindowsArm64,
}

/// Version-pinned unwind metadata retained with compiled code.
///
/// The encoding is Cranelift 0.116's serde representation. Keeping it opaque
/// here avoids making the cache depend on Cranelift when the compiler feature
/// is disabled while preserving the complete unwind plan for registration by
/// a later installation stage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnwindMetadata {
    kind: UnwindKind,
    frame_size: u32,
    encoding: Box<[u8]>,
}

impl UnwindMetadata {
    pub fn new(kind: UnwindKind, frame_size: u32, encoding: Vec<u8>) -> Self {
        Self {
            kind,
            frame_size,
            encoding: encoding.into_boxed_slice(),
        }
    }

    pub const fn kind(&self) -> UnwindKind {
        self.kind
    }

    pub const fn frame_size(&self) -> u32 {
        self.frame_size
    }

    pub fn encoding(&self) -> &[u8] {
        &self.encoding
    }
}

/// Exact native location represented by a retained logical frame state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameStateLocationKind {
    /// The code offset is the return address of one machine call site.
    CallReturn,
    /// The code offset is the start of one emitted non-call marker range.
    Marker,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameState {
    pub code_offset: u32,
    pub bytecode_pc: u32,
    pub slots: Box<[u16]>,
    pub location_kind: FrameStateLocationKind,
    pub source_location: u32,
    pub source_start: u32,
    pub source_end: u32,
}

impl FrameState {
    pub fn new(code_offset: u32, bytecode_pc: u32, slots: Vec<u16>) -> Self {
        Self::with_location(
            code_offset,
            bytecode_pc,
            slots,
            FrameStateLocationKind::Marker,
            0,
            code_offset,
            code_offset.saturating_add(1),
        )
    }

    pub fn with_location(
        code_offset: u32,
        bytecode_pc: u32,
        slots: Vec<u16>,
        location_kind: FrameStateLocationKind,
        source_location: u32,
        source_start: u32,
        source_end: u32,
    ) -> Self {
        Self {
            code_offset,
            bytecode_pc,
            slots: slots.into_boxed_slice(),
            location_kind,
            source_location,
            source_start,
            source_end,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactDependency {
    pub function: FunctionKey,
}

#[cfg(feature = "compiler")]
#[derive(Clone, Debug, PartialEq)]
pub struct OptimizedArtifactMetadata {
    feedback_epoch: u64,
    deopt_sites: Box<[(crate::ir::OptimizedFrameShape, crate::ir::DeoptMap)]>,
    boxes_elided: u64,
    cse_eliminated: u64,
    dead_nodes_eliminated: u64,
    inlined_calls: u64,
    side_path_profile: Option<crate::runtime::SidePathProfile>,
    direct_call_signature: Option<crate::runtime::BoundedSpecializationSignature>,
    #[cfg(not(target_family = "wasm"))]
    native_call_plan: Option<crate::compiler::native_call::NativeCallPlan>,
}

#[cfg(feature = "compiler")]
impl OptimizedArtifactMetadata {
    pub fn new(
        feedback_epoch: u64,
        deopt_sites: Vec<(crate::ir::OptimizedFrameShape, crate::ir::DeoptMap)>,
        boxes_elided: u64,
        cse_eliminated: u64,
        dead_nodes_eliminated: u64,
    ) -> Self {
        Self {
            feedback_epoch,
            deopt_sites: deopt_sites.into_boxed_slice(),
            boxes_elided,
            cse_eliminated,
            dead_nodes_eliminated,
            inlined_calls: 0,
            side_path_profile: None,
            direct_call_signature: None,
            #[cfg(not(target_family = "wasm"))]
            native_call_plan: None,
        }
    }
    /// Number of statically expanded call sites, not runtime call executions.
    pub const fn inlined_calls(&self) -> u64 {
        self.inlined_calls
    }
    pub fn with_inlined_calls(mut self, calls: u64) -> Self {
        self.inlined_calls = calls;
        self
    }
    pub const fn feedback_epoch(&self) -> u64 {
        self.feedback_epoch
    }
    pub fn deopt_sites(&self) -> &[(crate::ir::OptimizedFrameShape, crate::ir::DeoptMap)] {
        &self.deopt_sites
    }
    pub const fn boxes_elided(&self) -> u64 {
        self.boxes_elided
    }
    pub const fn cse_eliminated(&self) -> u64 {
        self.cse_eliminated
    }
    pub const fn dead_nodes_eliminated(&self) -> u64 {
        self.dead_nodes_eliminated
    }
    pub const fn side_path_profile(&self) -> Option<crate::runtime::SidePathProfile> {
        self.side_path_profile
    }
    pub fn with_side_path_profile(mut self, profile: crate::runtime::SidePathProfile) -> Self {
        self.side_path_profile = Some(profile);
        self
    }
    pub fn with_direct_call_signature(
        mut self,
        signature: crate::runtime::BoundedSpecializationSignature,
    ) -> Self {
        self.direct_call_signature = Some(signature);
        self
    }
    pub const fn direct_call_signature(
        &self,
    ) -> Option<&crate::runtime::BoundedSpecializationSignature> {
        self.direct_call_signature.as_ref()
    }
    #[cfg(not(target_family = "wasm"))]
    pub(crate) fn with_native_call_plan(
        mut self,
        plan: crate::compiler::native_call::NativeCallPlan,
    ) -> Self {
        self.native_call_plan = Some(plan);
        self
    }
    /// The pure self-recursive native entry published with this artifact.
    #[cfg(not(target_family = "wasm"))]
    pub const fn native_call_plan(&self) -> Option<&crate::compiler::native_call::NativeCallPlan> {
        self.native_call_plan.as_ref()
    }
}

impl ArtifactDependency {
    pub const fn new(function: FunctionKey) -> Self {
        Self { function }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BenefitSnapshot {
    pub executions: u64,
    pub score: u64,
}

#[derive(Debug, Default)]
struct BenefitCounters {
    executions: AtomicU64,
    score: AtomicU64,
}

impl BenefitCounters {
    // `Atomic::try_update` would avoid this deprecation, but it is unavailable
    // on the crate's Rust 1.87 MSRV.
    #[allow(deprecated)]
    fn record(&self, score: u64) {
        let _ = self
            .executions
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_add(1))
            });
        let _ = self
            .score
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_add(score))
            });
    }

    fn snapshot(&self) -> BenefitSnapshot {
        BenefitSnapshot {
            executions: self.executions.load(Ordering::Acquire),
            score: self.score.load(Ordering::Acquire),
        }
    }
}

/// Compiler output. Task 5 allocations contain inert owned bytes only.
#[derive(Debug)]
pub struct CompiledArtifact {
    key: ArtifactKey,
    code: CodeAllocation,
    relocations: Box<[Relocation]>,
    unwind_metadata: Option<UnwindMetadata>,
    stack_maps: Box<[StackMap]>,
    frame_states: Box<[FrameState]>,
    dependencies: Box<[ArtifactDependency]>,
    benefit: BenefitCounters,
    #[cfg(feature = "compiler")]
    optimized: Option<OptimizedArtifactMetadata>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    relocatable: Option<Box<crate::compiler::baseline::RelocatableCode>>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    published: Option<crate::compiler::baseline::PublishedBaselineCode>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    direct_call_relocatable: Option<Box<crate::compiler::baseline::RelocatableCode>>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    direct_call_published: Option<crate::compiler::baseline::PublishedBaselineCode>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    native_call_relocatable: Option<Box<crate::compiler::baseline::RelocatableCode>>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    native_call_published: Option<crate::compiler::baseline::PublishedBaselineCode>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    direct_call_dependencies: Box<[crate::compiler::baseline::PublishedBaselineCode]>,
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    inline_snapshot: Option<crate::bytecode::CompileSnapshot>,
    #[cfg(any(test, feature = "test-support"))]
    fake: bool,
}

impl CompiledArtifact {
    pub fn empty(key: ArtifactKey) -> Self {
        Self::from_parts(
            key,
            CodeAllocation::inert(Vec::new()),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        )
    }

    pub fn from_parts(
        key: ArtifactKey,
        code: CodeAllocation,
        relocations: Vec<Relocation>,
        stack_maps: Vec<StackMap>,
        frame_states: Vec<FrameState>,
        dependencies: Vec<ArtifactDependency>,
    ) -> Self {
        Self {
            key,
            code,
            relocations: relocations.into_boxed_slice(),
            unwind_metadata: None,
            stack_maps: stack_maps.into_boxed_slice(),
            frame_states: frame_states.into_boxed_slice(),
            dependencies: dependencies.into_boxed_slice(),
            benefit: BenefitCounters::default(),
            #[cfg(feature = "compiler")]
            optimized: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            relocatable: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            published: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            direct_call_relocatable: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            direct_call_published: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            native_call_relocatable: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            native_call_published: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            direct_call_dependencies: Box::new([]),
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            inline_snapshot: None,
            #[cfg(any(test, feature = "test-support"))]
            fake: false,
        }
    }

    pub const fn key(&self) -> ArtifactKey {
        self.key
    }

    pub const fn code(&self) -> &CodeAllocation {
        &self.code
    }

    pub fn relocations(&self) -> &[Relocation] {
        &self.relocations
    }

    pub fn unwind_metadata(&self) -> Option<&UnwindMetadata> {
        self.unwind_metadata.as_ref()
    }

    pub fn with_unwind_metadata(mut self, metadata: Option<UnwindMetadata>) -> Self {
        self.unwind_metadata = metadata;
        self
    }

    pub fn stack_maps(&self) -> &[StackMap] {
        &self.stack_maps
    }

    pub fn frame_states(&self) -> &[FrameState] {
        &self.frame_states
    }

    pub fn dependencies(&self) -> &[ArtifactDependency] {
        &self.dependencies
    }

    pub fn with_dependencies(mut self, dependencies: Vec<ArtifactDependency>) -> Self {
        self.dependencies = dependencies.into_boxed_slice();
        self
    }

    #[cfg(feature = "compiler")]
    pub fn optimized_metadata(&self) -> Option<&OptimizedArtifactMetadata> {
        self.optimized.as_ref()
    }

    #[cfg(feature = "compiler")]
    pub fn with_optimized_metadata(mut self, metadata: OptimizedArtifactMetadata) -> Self {
        self.optimized = Some(metadata);
        self
    }

    pub fn benefit(&self) -> BenefitSnapshot {
        self.benefit.snapshot()
    }

    pub fn charge_bytes(&self) -> Option<usize> {
        fn add_slice<T>(total: usize, len: usize) -> Option<usize> {
            total.checked_add(len.checked_mul(core::mem::size_of::<T>())?)
        }

        let mut total = self.code.bytes().len();
        #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
        if let Some(snapshot) = &self.inline_snapshot {
            total = total.checked_add(snapshot.retained_bytes())?;
        }
        total = add_slice::<Relocation>(total, self.relocations.len())?;
        for relocation in &self.relocations {
            if let RelocationTarget::Symbol(symbol) = &relocation.target {
                total = total.checked_add(symbol.len())?;
            }
        }
        if let Some(unwind) = &self.unwind_metadata {
            total = total.checked_add(core::mem::size_of::<UnwindMetadata>())?;
            total = total.checked_add(unwind.encoding.len())?;
        }
        total = add_slice::<StackMap>(total, self.stack_maps.len())?;
        for map in &self.stack_maps {
            total = add_slice::<u16>(total, map.live_slots.len())?;
        }
        total = add_slice::<FrameState>(total, self.frame_states.len())?;
        for state in &self.frame_states {
            total = add_slice::<u16>(total, state.slots.len())?;
        }
        total = add_slice::<ArtifactDependency>(total, self.dependencies.len())?;
        #[cfg(feature = "compiler")]
        if let Some(optimized) = &self.optimized {
            total = total.checked_add(core::mem::size_of::<OptimizedArtifactMetadata>())?;
            total = add_slice::<(crate::ir::OptimizedFrameShape, crate::ir::DeoptMap)>(
                total,
                optimized.deopt_sites.len(),
            )?;
            for (_, map) in &optimized.deopt_sites {
                total = total.checked_add(
                    map.materialization_count()
                        .checked_mul(core::mem::size_of::<crate::ir::Materialization>())?,
                )?;
            }
        }
        Some(total)
    }

    pub fn code_bytes(&self) -> usize {
        self.code.bytes().len()
    }

    pub fn metadata_bytes(&self) -> Option<usize> {
        self.charge_bytes()?.checked_sub(self.code_bytes())
    }

    /// Copied callee bytecode for a future graph-builder inline attempt.
    /// Retention does not establish inline eligibility or replace target guards.
    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn inline_snapshot(&self) -> Option<&crate::bytecode::CompileSnapshot> {
        self.inline_snapshot.as_ref()
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub(crate) fn with_inline_snapshot(
        mut self,
        function: &crate::bytecode::VerifiedFunction,
    ) -> Self {
        let snapshot = function.snapshot();
        // Optional retention has independent bounds; large callees keep their
        // normal executable entry and do not make caller compilation fail.
        if function.instructions().len() <= 128
            && snapshot.retained_bytes() <= 16 * 1024
            && snapshot.exception_map().is_empty()
            && !function.has_exception_regions()
            && snapshot.function_id() == self.key.function_id
            && snapshot.generation() == self.key.generation
            && snapshot.source_revision() == self.key.source_revision
            && snapshot.opcode_fingerprint() == self.key.opcode_fingerprint
        {
            self.inline_snapshot = Some(snapshot.clone());
        }
        self
    }

    pub(crate) fn record_benefit(&self, score: u64) {
        self.benefit.record(score);
    }

    pub(crate) fn benefit_score(&self) -> u64 {
        self.benefit.score.load(Ordering::Acquire)
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub(crate) fn with_relocatable(
        mut self,
        code: crate::compiler::baseline::RelocatableCode,
    ) -> Self {
        self.relocatable = Some(Box::new(code));
        self
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub(crate) fn with_direct_call_relocatable(
        mut self,
        code: crate::compiler::baseline::RelocatableCode,
    ) -> Self {
        self.direct_call_relocatable = Some(Box::new(code));
        self
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub(crate) fn with_native_call_relocatable(
        mut self,
        code: crate::compiler::baseline::RelocatableCode,
    ) -> Self {
        self.native_call_relocatable = Some(Box::new(code));
        self
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub(crate) fn publish_relocatable(&mut self) -> Result<(), crate::platform::CodeMemoryError> {
        // The native entry publishes first: the main body embeds its address
        // and retains its publication for the body's whole executable life.
        if let Some(code) = self.native_call_relocatable.take() {
            self.native_call_published = Some(code.publish()?);
        }
        if let Some(code) = self.relocatable.take() {
            self.published = Some(match &self.native_call_published {
                Some(native) => code.publish_with(
                    &[(
                        crate::compiler::native_call::NATIVE_ENTRY_SYMBOL,
                        native.as_ptr() as usize as u64,
                    )],
                    vec![native.clone()],
                )?,
                None => code.publish()?,
            });
        }
        if let Some(code) = self.direct_call_relocatable.take() {
            self.direct_call_published = Some(code.publish()?);
        }
        Ok(())
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn published(&self) -> Option<&crate::compiler::baseline::PublishedBaselineCode> {
        self.published.as_ref()
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn native_call_published(
        &self,
    ) -> Option<&crate::compiler::baseline::PublishedBaselineCode> {
        self.native_call_published.as_ref()
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub fn direct_call_published(
        &self,
    ) -> Option<&crate::compiler::baseline::PublishedBaselineCode> {
        self.direct_call_published.as_ref()
    }

    #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
    pub(crate) fn with_direct_call_dependencies(
        mut self,
        dependencies: Vec<crate::compiler::baseline::PublishedBaselineCode>,
    ) -> Self {
        self.direct_call_dependencies = dependencies.into_boxed_slice();
        self
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn fake(tier: Tier) -> Self {
        Self {
            key: ArtifactKey {
                runtime_id: 0,
                function_id: 0,
                generation: 0,
                tier,
                target_isa: 0,
                cpu_features: 0,
                abi_fingerprint: 0,
                source_revision: 0,
                opcode_fingerprint: 0,
                config_fingerprint: 0,
                specialization_fingerprint: 0,
            },
            code: CodeAllocation::inert(Vec::new()),
            relocations: Box::new([]),
            unwind_metadata: None,
            stack_maps: Box::new([]),
            frame_states: Box::new([]),
            dependencies: Box::new([]),
            benefit: BenefitCounters::default(),
            #[cfg(feature = "compiler")]
            optimized: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            relocatable: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            published: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            direct_call_relocatable: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            direct_call_published: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            native_call_relocatable: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            native_call_published: None,
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            direct_call_dependencies: Box::new([]),
            #[cfg(all(feature = "compiler", not(target_family = "wasm")))]
            inline_snapshot: None,
            fake: true,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn bind_fake(mut self, key: ArtifactKey) -> Self {
        if self.fake && self.key.tier == key.tier {
            self.key = key;
            self.fake = false;
        }
        self
    }
}

#[cfg(all(test, feature = "compiler", not(target_family = "wasm")))]
mod inline_snapshot_tests {
    use super::*;
    use crate::bytecode::{opcode, CompileSnapshot, VerifyLimits};

    fn function(bytes: Vec<u8>) -> crate::bytecode::VerifiedFunction {
        CompileSnapshot::from_untrusted_bytecode(bytes, 0, 0, 0, 0)
            .verify(VerifyLimits::default())
            .unwrap()
    }

    fn artifact(function: &crate::bytecode::VerifiedFunction) -> CompiledArtifact {
        let snapshot = function.snapshot();
        let mut key = CompiledArtifact::fake(Tier::Baseline).key();
        key.function_id = snapshot.function_id();
        key.generation = snapshot.generation();
        key.source_revision = snapshot.source_revision();
        key.opcode_fingerprint = snapshot.opcode_fingerprint();
        CompiledArtifact::empty(key)
    }

    #[test]
    fn retained_inline_body_is_charged_and_survives_its_artifact() {
        let function = function(vec![opcode::RETURN_UNDEF]);
        let bytes = function.snapshot().retained_bytes();
        let artifact = artifact(&function).with_inline_snapshot(&function);
        assert_eq!(artifact.metadata_bytes(), Some(bytes));
        let retained = artifact.inline_snapshot().unwrap().clone();
        drop(artifact);
        drop(function);
        assert_eq!(retained.bytecode(), &[opcode::RETURN_UNDEF]);
        assert!(retained.verify(VerifyLimits::default()).is_ok());
    }

    #[test]
    fn inline_retention_declines_large_bodies_and_spare_capacity() {
        let mut long = vec![opcode::NOP; 128];
        long.push(opcode::RETURN_UNDEF);
        let mut spare = Vec::with_capacity(32 * 1024);
        spare.push(opcode::RETURN_UNDEF);
        for bytes in [long, spare] {
            let function = function(bytes);
            let artifact = artifact(&function).with_inline_snapshot(&function);
            assert!(artifact.inline_snapshot().is_none());
            assert_eq!(artifact.metadata_bytes(), Some(0));
        }
    }

    #[test]
    fn inline_retention_requires_exact_source_identity() {
        let function = function(vec![opcode::RETURN_UNDEF]);
        for field in 0..4 {
            let mut artifact = artifact(&function);
            match field {
                0 => artifact.key.function_id ^= 1,
                1 => artifact.key.generation ^= 1,
                2 => artifact.key.source_revision ^= 1,
                _ => artifact.key.opcode_fingerprint ^= 1,
            }
            assert!(artifact
                .with_inline_snapshot(&function)
                .inline_snapshot()
                .is_none());
        }
    }
}
