//! Safe inspection and validation of the linked QuickJS JIT ABI.

use core::{fmt, mem};

use rquickjs_core::qjs;

pub const ABI_MAJOR: u16 = 1;
pub const ABI_MINOR: u16 = 25;

pub const SOURCE_REVISION: u64 = 0xfd0a_0210_b7be_0095;
pub const OPCODE_FINGERPRINT: u64 = qjs::QJSJIT_GENERATED_OPCODE_FINGERPRINT;
pub const HELPER_TABLE_FINGERPRINT: u64 = qjs::QJSJIT_GENERATED_HELPER_FINGERPRINT;
const BUILD_FEATURE_FLAGS: u64 = 0x0000_0000_0000_0001;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbiStructure {
    AbiInfo,
    FunctionId,
    HotEvent,
    FunctionSnapshot,
    EntryHandle,
    ExecFrame,
    Exit,
    RuntimeApi,
    HelperTable,
    ElementLayout,
    PropertyLayout,
    BackendVTable,
    InlineApi,
    ArrayApi,
    IteratorApi,
    ObjectApi,
}

/// Constructors for the authoritative native-to-interpreter exit contract.
pub trait JitExitExt: Sized {
    fn done() -> Self;
    fn exception() -> Self;
    fn interrupt() -> Self;
    fn resume(resume_pc: *const u8) -> Self;
    fn retry_interpreter() -> Self;
}

impl JitExitExt for qjs::JSJitExit {
    fn done() -> Self {
        new_exit(qjs::JSJitExitKind_JS_JIT_EXIT_DONE, core::ptr::null())
    }

    fn exception() -> Self {
        new_exit(qjs::JSJitExitKind_JS_JIT_EXIT_EXCEPTION, core::ptr::null())
    }

    fn interrupt() -> Self {
        new_exit(qjs::JSJitExitKind_JS_JIT_EXIT_INTERRUPT, core::ptr::null())
    }

    fn resume(resume_pc: *const u8) -> Self {
        new_exit(qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT, resume_pc)
    }

    fn retry_interpreter() -> Self {
        new_exit(
            qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER,
            core::ptr::null(),
        )
    }
}

fn new_exit(kind: u32, resume_pc: *const u8) -> qjs::JSJitExit {
    qjs::JSJitExit {
        kind,
        reserved: 0,
        resume_pc,
        resume_stack_top: core::ptr::null_mut(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbiMismatch {
    StructSize,
    Major,
    Minor,
    PointerWidth,
    Endianness,
    ValueSize,
    SourceRevision,
    OpcodeFingerprint,
    ValueLayout,
    FeatureFlags,
    StructureLayout(AbiStructure),
    BuildFingerprint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AbiError {
    QueryFailed(i32),
    Incompatible(AbiMismatch),
}

impl fmt::Display for AbiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QueryFailed(status) => write!(f, "QuickJS ABI query failed with status {status}"),
            Self::Incompatible(mismatch) => {
                write!(f, "incompatible QuickJS JIT ABI: {mismatch:?}")
            }
        }
    }
}

impl std::error::Error for AbiError {}

#[derive(Clone, Copy, Debug)]
pub struct AbiInfo {
    raw: qjs::JSJitABIInfo,
    inline_api: qjs::JSJitInlineAPI,
    array_api: qjs::JSJitArrayAPI,
    iterator_api: qjs::JSJitIteratorAPI,
    object_api: qjs::JSJitObjectAPI,
}

/// Runtime-exported, append-only offsets for element fast paths.  QuickJS
/// keeps these internals private; code generation must only use this
/// fingerprinted descriptor, never Rust-side assumptions about object layout.
#[cfg(feature = "compiler")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct ElementLayout {
    pub object_class_id_offset: i32,
    pub object_flags_offset: i32,
    pub object_fast_array_mask: i64,
    pub object_union_offset: i32,
    pub array_count_offset: i32,
    pub array_data_offset: i32,
    pub typed_array_ptr_offset: i32,
    pub typed_array_buffer_offset: i32,
    pub typed_array_track_rab_offset: i32,
    pub array_buffer_data_offset: i32,
    pub array_buffer_detached_offset: i32,
    pub array_buffer_immutable_offset: i32,
    pub array_class_id: i64,
    pub int32_array_class_id: i64,
    pub float64_array_class_id: i64,
}

/// Validated runtime offsets for native shape generation guards.
#[cfg(feature = "compiler")]
#[derive(Clone, Copy, Debug)]
pub(crate) struct PropertyLayout {
    pub object_shape_offset: i32,
    pub object_properties_offset: i32,
    pub shape_generation_offset: i32,
}

impl AbiInfo {
    pub fn linked() -> Result<Self, AbiError> {
        let info = Self::query_linked()?;
        info.validate()?;
        Ok(info)
    }

    pub const fn major(&self) -> u16 {
        self.raw.major
    }

    pub const fn minor(&self) -> u16 {
        self.raw.minor
    }

    pub const fn pointer_width(&self) -> u8 {
        self.raw.pointer_width
    }

    pub const fn little_endian(&self) -> bool {
        self.raw.little_endian != 0
    }

    pub const fn source_revision(&self) -> u64 {
        self.raw.source_revision
    }

    pub const fn opcode_fingerprint(&self) -> u64 {
        self.raw.opcode_fingerprint
    }

    pub const fn build_fingerprint(&self) -> u64 {
        self.raw.build_fingerprint
    }

    /// Native inline operations, copied only after the main ABI and this
    /// versioned table have been validated. Each operation may allocate,
    /// throw, finalize and reenter; callers must publish the required roots.
    pub(crate) const fn inline_api(&self) -> &qjs::JSJitInlineAPI {
        &self.inline_api
    }

    /// Read-only, non-allocating typed-array continuing-path guards. Runtime
    /// feedback chooses candidates; this table revalidates live state.
    #[cfg(feature = "compiler")]
    pub(crate) const fn array_api(&self) -> &qjs::JSJitArrayAPI {
        &self.array_api
    }

    /// Non-allocating continuing-path leaves for synchronous iteration. A
    /// miss never touches state; callers fall back to the exact helper.
    #[cfg(feature = "compiler")]
    pub(crate) const fn iterator_api(&self) -> &qjs::JSJitIteratorAPI {
        &self.iterator_api
    }

    /// Non-throwing, non-reentrant allocation and array-method fast paths.
    /// Each entry completes the exact interpreter effect or misses without an
    /// observable effect; callers keep their generic helper as the fallback.
    #[cfg(feature = "compiler")]
    pub(crate) const fn object_api(&self) -> &qjs::JSJitObjectAPI {
        &self.object_api
    }

    #[cfg(feature = "compiler")]
    pub(crate) fn element_layout(&self) -> ElementLayout {
        let raw = self.raw.element_layout;
        ElementLayout {
            object_class_id_offset: raw.object_class_id_offset as i32,
            object_flags_offset: raw.object_flags_offset as i32,
            object_fast_array_mask: raw.object_fast_array_mask as i64,
            object_union_offset: raw.object_union_offset as i32,
            array_count_offset: raw.array_count_offset as i32,
            array_data_offset: raw.array_data_offset as i32,
            typed_array_ptr_offset: raw.typed_array_ptr_offset as i32,
            typed_array_buffer_offset: raw.typed_array_buffer_offset as i32,
            typed_array_track_rab_offset: raw.typed_array_track_rab_offset as i32,
            array_buffer_data_offset: raw.array_buffer_data_offset as i32,
            array_buffer_detached_offset: raw.array_buffer_detached_offset as i32,
            array_buffer_immutable_offset: raw.array_buffer_immutable_offset as i32,
            array_class_id: raw.array_class_id as i64,
            int32_array_class_id: raw.int32_array_class_id as i64,
            float64_array_class_id: raw.float64_array_class_id as i64,
        }
    }

    #[cfg(feature = "compiler")]
    pub(crate) fn property_layout(&self) -> PropertyLayout {
        let raw = self.raw.property_layout;
        PropertyLayout {
            object_shape_offset: raw.object_shape_offset as i32,
            object_properties_offset: raw.object_properties_offset as i32,
            shape_generation_offset: raw.shape_generation_offset as i32,
        }
    }

    pub(crate) fn query_linked() -> Result<Self, AbiError> {
        let mut raw = unsafe { mem::zeroed::<qjs::JSJitABIInfo>() };
        raw.struct_size = mem::size_of::<qjs::JSJitABIInfo>() as u32;
        let status = unsafe { qjs::JS_GetJitABIInfo(&mut raw) };
        if status == qjs::JS_JIT_BACKEND_OK {
            let mut info = Self {
                raw,
                // All fields are integers or nullable function pointers.
                inline_api: unsafe { mem::zeroed() },
                array_api: unsafe { mem::zeroed() },
                iterator_api: unsafe { mem::zeroed() },
                object_api: unsafe { mem::zeroed() },
            };
            // Do not resolve native inline addresses against a mismatched
            // main ABI, even when the optional table has a familiar version.
            info.validate_main()?;
            info.inline_api = query_inline_api()?;
            info.array_api = query_array_api()?;
            info.iterator_api = query_iterator_api()?;
            info.object_api = query_object_api()?;
            Ok(info)
        } else {
            Err(AbiError::QueryFailed(status))
        }
    }

    pub(crate) fn validate(&self) -> Result<(), AbiError> {
        self.validate_main()?;
        validate_inline_api(self.inline_api())?;
        validate_array_api(&self.array_api)?;
        validate_iterator_api(&self.iterator_api)?;
        validate_object_api(&self.object_api)
    }

    fn validate_main(&self) -> Result<(), AbiError> {
        let raw = &self.raw;
        let mismatch = if raw.struct_size != mem::size_of::<qjs::JSJitABIInfo>() as u32 {
            Some(AbiMismatch::StructSize)
        } else if raw.major != ABI_MAJOR {
            Some(AbiMismatch::Major)
        } else if raw.minor.cmp(&ABI_MINOR).is_lt() {
            Some(AbiMismatch::Minor)
        } else if raw.pointer_width != usize::BITS as u8 {
            Some(AbiMismatch::PointerWidth)
        } else if (raw.little_endian != 0) != cfg!(target_endian = "little") {
            Some(AbiMismatch::Endianness)
        } else if raw.value_size != mem::size_of::<qjs::JSValue>() as u16 {
            Some(AbiMismatch::ValueSize)
        } else if raw.source_revision != SOURCE_REVISION {
            Some(AbiMismatch::SourceRevision)
        } else if raw.opcode_fingerprint != OPCODE_FINGERPRINT {
            Some(AbiMismatch::OpcodeFingerprint)
        } else if raw.value_layout_fingerprint != value_layout_fingerprint() {
            Some(AbiMismatch::ValueLayout)
        } else if raw.build_feature_flags != BUILD_FEATURE_FLAGS {
            Some(AbiMismatch::FeatureFlags)
        } else if raw.abi_info_layout_fingerprint != abi_info_layout_fingerprint() {
            Some(AbiMismatch::StructureLayout(AbiStructure::AbiInfo))
        } else if raw.function_id_layout_fingerprint != function_id_layout_fingerprint() {
            Some(AbiMismatch::StructureLayout(AbiStructure::FunctionId))
        } else if raw.hot_event_layout_fingerprint != hot_event_layout_fingerprint() {
            Some(AbiMismatch::StructureLayout(AbiStructure::HotEvent))
        } else if raw.function_snapshot_layout_fingerprint != function_snapshot_layout_fingerprint()
        {
            Some(AbiMismatch::StructureLayout(AbiStructure::FunctionSnapshot))
        } else if raw.entry_handle_layout_fingerprint != entry_handle_layout_fingerprint() {
            Some(AbiMismatch::StructureLayout(AbiStructure::EntryHandle))
        } else if raw.exec_frame_layout_fingerprint != exec_frame_layout_fingerprint() {
            Some(AbiMismatch::StructureLayout(AbiStructure::ExecFrame))
        } else if raw.exit_layout_fingerprint != exit_layout_fingerprint() {
            Some(AbiMismatch::StructureLayout(AbiStructure::Exit))
        } else if raw.runtime_api_layout_fingerprint != runtime_api_layout_fingerprint() {
            Some(AbiMismatch::StructureLayout(AbiStructure::RuntimeApi))
        } else if raw.helper_table_fingerprint != HELPER_TABLE_FINGERPRINT {
            Some(AbiMismatch::StructureLayout(AbiStructure::HelperTable))
        } else if raw.element_layout.struct_size != mem::size_of::<qjs::JSJitElementLayout>() as u32
            || raw.element_layout_fingerprint != element_layout_fingerprint()
        {
            Some(AbiMismatch::StructureLayout(AbiStructure::ElementLayout))
        } else if raw.property_layout.struct_size
            != mem::size_of::<qjs::JSJitPropertyLayout>() as u32
            || raw.property_layout_fingerprint != property_layout_fingerprint()
            || raw.property_layout.object_shape_offset > i32::MAX as u32
            || raw.property_layout.object_properties_offset > i32::MAX as u32
            || raw.property_layout.shape_generation_offset > i32::MAX as u32
        {
            Some(AbiMismatch::StructureLayout(AbiStructure::PropertyLayout))
        } else if raw.backend_vtable_layout_fingerprint != backend_vtable_layout_fingerprint() {
            Some(AbiMismatch::StructureLayout(AbiStructure::BackendVTable))
        } else if raw.build_fingerprint != expected_build_fingerprint() {
            Some(AbiMismatch::BuildFingerprint)
        } else {
            None
        };

        mismatch.map_or(Ok(()), |mismatch| Err(AbiError::Incompatible(mismatch)))
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn corrupt(&mut self, mismatch: AbiMismatch) {
        match mismatch {
            AbiMismatch::StructSize => self.raw.struct_size ^= 1,
            AbiMismatch::Major => self.raw.major ^= 1,
            AbiMismatch::Minor => self.raw.minor = ABI_MINOR.saturating_sub(1),
            AbiMismatch::PointerWidth => self.raw.pointer_width ^= 32,
            AbiMismatch::Endianness => self.raw.little_endian ^= 1,
            AbiMismatch::ValueSize => self.raw.value_size ^= 1,
            AbiMismatch::SourceRevision => self.raw.source_revision ^= 1,
            AbiMismatch::OpcodeFingerprint => self.raw.opcode_fingerprint ^= 1,
            AbiMismatch::ValueLayout => self.raw.value_layout_fingerprint ^= 1,
            AbiMismatch::FeatureFlags => self.raw.build_feature_flags ^= 1,
            AbiMismatch::StructureLayout(AbiStructure::AbiInfo) => {
                self.raw.abi_info_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::FunctionId) => {
                self.raw.function_id_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::HotEvent) => {
                self.raw.hot_event_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::FunctionSnapshot) => {
                self.raw.function_snapshot_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::EntryHandle) => {
                self.raw.entry_handle_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::ExecFrame) => {
                self.raw.exec_frame_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::Exit) => {
                self.raw.exit_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::RuntimeApi) => {
                self.raw.runtime_api_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::HelperTable) => {
                self.raw.helper_table_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::PropertyLayout) => {
                self.raw.property_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::ElementLayout) => {
                self.raw.element_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::BackendVTable) => {
                self.raw.backend_vtable_layout_fingerprint ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::InlineApi) => self.inline_api.version ^= 1,
            AbiMismatch::StructureLayout(AbiStructure::ArrayApi) => self.array_api.version ^= 1,
            AbiMismatch::StructureLayout(AbiStructure::IteratorApi) => {
                self.iterator_api.version ^= 1
            }
            AbiMismatch::StructureLayout(AbiStructure::ObjectApi) => self.object_api.version ^= 1,
            AbiMismatch::BuildFingerprint => self.raw.build_fingerprint ^= 1,
        }
    }
}

fn hash_u64(mut hash: u64, mut value: u64) -> u64 {
    for _ in 0..8 {
        hash ^= value & 0xff;
        hash = hash.wrapping_mul(FNV_PRIME);
        value >>= 8;
    }
    hash
}

fn layout_start<T>() -> u64 {
    hash_u64(
        hash_u64(FNV_OFFSET, mem::size_of::<T>() as u64),
        mem::align_of::<T>() as u64,
    )
}

fn layout_field(hash: u64, offset: usize, size: usize) -> u64 {
    hash_u64(hash_u64(hash, offset as u64), size as u64)
}

fn function_id_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitFunctionId>();
    hash = layout_field(hash, mem::offset_of!(qjs::JSJitFunctionId, struct_size), 4);
    hash = layout_field(hash, mem::offset_of!(qjs::JSJitFunctionId, reserved), 4);
    hash = layout_field(hash, mem::offset_of!(qjs::JSJitFunctionId, id), 8);
    layout_field(hash, mem::offset_of!(qjs::JSJitFunctionId, generation), 8)
}

fn hot_event_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitHotEvent>();
    hash = layout_field(hash, mem::offset_of!(qjs::JSJitHotEvent, struct_size), 4);
    hash = layout_field(hash, mem::offset_of!(qjs::JSJitHotEvent, kind), 4);
    hash = layout_field(
        hash,
        mem::offset_of!(qjs::JSJitHotEvent, function),
        mem::size_of::<qjs::JSJitFunctionId>(),
    );
    hash = layout_field(hash, mem::offset_of!(qjs::JSJitHotEvent, pc), 4);
    layout_field(hash, mem::offset_of!(qjs::JSJitHotEvent, count), 4)
}

fn function_snapshot_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitFunctionSnapshot>();
    hash = layout_field(
        hash,
        mem::offset_of!(qjs::JSJitFunctionSnapshot, struct_size),
        4,
    );
    hash = layout_field(hash, mem::offset_of!(qjs::JSJitFunctionSnapshot, flags), 4);
    hash = layout_field(
        hash,
        mem::offset_of!(qjs::JSJitFunctionSnapshot, function),
        mem::size_of::<qjs::JSJitFunctionId>(),
    );
    hash = layout_field(
        hash,
        mem::offset_of!(qjs::JSJitFunctionSnapshot, opaque),
        mem::size_of::<*mut core::ffi::c_void>(),
    );
    for (offset, size) in [
        (
            mem::offset_of!(qjs::JSJitFunctionSnapshot, source_revision),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitFunctionSnapshot, opcode_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitFunctionSnapshot, bytecode),
            mem::size_of::<*const u8>(),
        ),
        (mem::offset_of!(qjs::JSJitFunctionSnapshot, bytecode_len), 4),
        (mem::offset_of!(qjs::JSJitFunctionSnapshot, arg_count), 2),
        (mem::offset_of!(qjs::JSJitFunctionSnapshot, local_count), 2),
        (
            mem::offset_of!(qjs::JSJitFunctionSnapshot, closure_count),
            2,
        ),
        (mem::offset_of!(qjs::JSJitFunctionSnapshot, stack_size), 2),
        (
            mem::offset_of!(qjs::JSJitFunctionSnapshot, constants),
            mem::size_of::<*const qjs::JSJitConstantDescriptor>(),
        ),
        (
            mem::offset_of!(qjs::JSJitFunctionSnapshot, constant_count),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitFunctionSnapshot, exception_map),
            mem::size_of::<*const u8>(),
        ),
        (
            mem::offset_of!(qjs::JSJitFunctionSnapshot, exception_map_len),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitFunctionSnapshot, source_map),
            mem::size_of::<*const u8>(),
        ),
        (
            mem::offset_of!(qjs::JSJitFunctionSnapshot, source_map_len),
            4,
        ),
    ] {
        hash = layout_field(hash, offset, size);
    }
    hash
}

fn entry_handle_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitEntryHandle>();
    hash = layout_field(hash, mem::offset_of!(qjs::JSJitEntryHandle, struct_size), 4);
    hash = layout_field(hash, mem::offset_of!(qjs::JSJitEntryHandle, reserved), 4);
    hash = layout_field(
        hash,
        mem::offset_of!(qjs::JSJitEntryHandle, entry),
        mem::size_of::<qjs::JSJitEntryFn>(),
    );
    hash = layout_field(
        hash,
        mem::offset_of!(qjs::JSJitEntryHandle, pin),
        mem::size_of::<*mut core::ffi::c_void>(),
    );
    hash = layout_field(
        hash,
        mem::offset_of!(qjs::JSJitEntryHandle, stack_map_count),
        4,
    );
    layout_field(
        hash,
        mem::offset_of!(qjs::JSJitEntryHandle, helper_abi_version),
        4,
    )
}

fn exec_frame_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitExecFrame>();
    for (offset, size) in [
        (mem::offset_of!(qjs::JSJitExecFrame, struct_size), 4),
        (mem::offset_of!(qjs::JSJitExecFrame, flags), 4),
        (
            mem::offset_of!(qjs::JSJitExecFrame, rt),
            mem::size_of::<*mut qjs::JSRuntime>(),
        ),
        (
            mem::offset_of!(qjs::JSJitExecFrame, ctx),
            mem::size_of::<*mut qjs::JSContext>(),
        ),
        (mem::offset_of!(qjs::JSJitExecFrame, function_id), 8),
        (mem::offset_of!(qjs::JSJitExecFrame, generation), 8),
        (
            mem::offset_of!(qjs::JSJitExecFrame, arg_buf),
            mem::size_of::<*mut qjs::JSValue>(),
        ),
        (
            mem::offset_of!(qjs::JSJitExecFrame, var_buf),
            mem::size_of::<*mut qjs::JSValue>(),
        ),
        (
            mem::offset_of!(qjs::JSJitExecFrame, stack_base),
            mem::size_of::<*mut qjs::JSValue>(),
        ),
        (
            mem::offset_of!(qjs::JSJitExecFrame, stack_top),
            mem::size_of::<*mut qjs::JSValue>(),
        ),
        (
            mem::offset_of!(qjs::JSJitExecFrame, bytecode_start),
            mem::size_of::<*const u8>(),
        ),
        (
            mem::offset_of!(qjs::JSJitExecFrame, pc),
            mem::size_of::<*const u8>(),
        ),
        (
            mem::offset_of!(qjs::JSJitExecFrame, result),
            mem::size_of::<qjs::JSValue>(),
        ),
        (
            mem::offset_of!(qjs::JSJitExecFrame, entry),
            mem::size_of::<qjs::JSJitEntryHandle>(),
        ),
        (
            mem::offset_of!(qjs::JSJitExecFrame, runtime_api),
            mem::size_of::<*const qjs::JSJitRuntimeAPI>(),
        ),
        (mem::offset_of!(qjs::JSJitExecFrame, runtime_id), 8),
        (mem::offset_of!(qjs::JSJitExecFrame, frame_cookie), 8),
        (
            mem::offset_of!(qjs::JSJitExecFrame, stack_capacity),
            mem::size_of::<*mut qjs::JSValue>(),
        ),
    ] {
        hash = layout_field(hash, offset, size);
    }
    hash
}

fn exit_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitExit>();
    for (offset, size) in [
        (mem::offset_of!(qjs::JSJitExit, kind), 4),
        (mem::offset_of!(qjs::JSJitExit, reserved), 4),
        (
            mem::offset_of!(qjs::JSJitExit, resume_pc),
            mem::size_of::<*const u8>(),
        ),
        (
            mem::offset_of!(qjs::JSJitExit, resume_stack_top),
            mem::size_of::<*mut qjs::JSValue>(),
        ),
    ] {
        hash = layout_field(hash, offset, size);
    }
    hash
}

fn runtime_api_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitRuntimeAPI>();
    for (offset, size) in [
        (mem::offset_of!(qjs::JSJitRuntimeAPI, struct_size), 4),
        (mem::offset_of!(qjs::JSJitRuntimeAPI, major), 2),
        (mem::offset_of!(qjs::JSJitRuntimeAPI, minor), 2),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, interrupt_poll),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, dup),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, free),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, resolve_const),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, to_numeric),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, to_bool),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, add_slow),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, compare_slow),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, get_property),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, set_property),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, call),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, new_array),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, new_object),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, shape_guard),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, materialize_owner),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, get_element),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, set_element),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, to_propkey),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, get_global),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, call_constructor),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, regexp),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, atom_value),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, binary_arith_slow),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, unary_arith_slow),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, fclosure),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, get_var_ref),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, put_var_ref),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, close_loc),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, set_name),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, generic_op),
            mem::size_of::<usize>(),
        ),
        (
            mem::offset_of!(qjs::JSJitRuntimeAPI, iterator_op),
            mem::size_of::<usize>(),
        ),
    ] {
        hash = layout_field(hash, offset, size);
    }
    hash
}

fn backend_vtable_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitBackendVTable>();
    hash = layout_field(
        hash,
        mem::offset_of!(qjs::JSJitBackendVTable, struct_size),
        4,
    );
    for offset in [
        mem::offset_of!(qjs::JSJitBackendVTable, record_hot),
        mem::offset_of!(qjs::JSJitBackendVTable, submit_snapshot),
        mem::offset_of!(qjs::JSJitBackendVTable, acquire_entry),
        mem::offset_of!(qjs::JSJitBackendVTable, release_entry),
        mem::offset_of!(qjs::JSJitBackendVTable, runtime_detach),
        mem::offset_of!(qjs::JSJitBackendVTable, function_retire),
        mem::offset_of!(qjs::JSJitBackendVTable, memory_used),
        mem::offset_of!(qjs::JSJitBackendVTable, native_enter),
        mem::offset_of!(qjs::JSJitBackendVTable, native_exit),
        mem::offset_of!(qjs::JSJitBackendVTable, record_feedback),
        mem::offset_of!(qjs::JSJitBackendVTable, entry_cache_epoch),
        mem::offset_of!(qjs::JSJitBackendVTable, entry_fast_grant),
    ] {
        hash = layout_field(hash, offset, mem::size_of::<usize>());
    }
    hash
}

fn abi_info_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitABIInfo>();
    for (offset, size) in [
        (mem::offset_of!(qjs::JSJitABIInfo, struct_size), 4),
        (mem::offset_of!(qjs::JSJitABIInfo, major), 2),
        (mem::offset_of!(qjs::JSJitABIInfo, minor), 2),
        (mem::offset_of!(qjs::JSJitABIInfo, pointer_width), 1),
        (mem::offset_of!(qjs::JSJitABIInfo, little_endian), 1),
        (mem::offset_of!(qjs::JSJitABIInfo, value_size), 2),
        (mem::offset_of!(qjs::JSJitABIInfo, source_revision), 8),
        (mem::offset_of!(qjs::JSJitABIInfo, opcode_fingerprint), 8),
        (
            mem::offset_of!(qjs::JSJitABIInfo, value_layout_fingerprint),
            8,
        ),
        (mem::offset_of!(qjs::JSJitABIInfo, build_feature_flags), 8),
        (mem::offset_of!(qjs::JSJitABIInfo, build_fingerprint), 8),
        (
            mem::offset_of!(qjs::JSJitABIInfo, abi_info_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, function_id_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, hot_event_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, function_snapshot_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, entry_handle_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, backend_vtable_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, exec_frame_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, exit_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, runtime_api_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, helper_table_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, element_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, element_layout),
            mem::size_of::<qjs::JSJitElementLayout>(),
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, property_layout_fingerprint),
            8,
        ),
        (
            mem::offset_of!(qjs::JSJitABIInfo, property_layout),
            mem::size_of::<qjs::JSJitPropertyLayout>(),
        ),
    ] {
        hash = layout_field(hash, offset, size);
    }
    hash
}

fn property_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitPropertyLayout>();
    for offset in [
        mem::offset_of!(qjs::JSJitPropertyLayout, struct_size),
        mem::offset_of!(qjs::JSJitPropertyLayout, object_shape_offset),
        mem::offset_of!(qjs::JSJitPropertyLayout, object_properties_offset),
        mem::offset_of!(qjs::JSJitPropertyLayout, shape_generation_offset),
    ] {
        hash = layout_field(hash, offset, 4);
    }
    hash
}

fn element_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSJitElementLayout>();
    for (offset, size) in [
        (mem::offset_of!(qjs::JSJitElementLayout, struct_size), 4),
        (
            mem::offset_of!(qjs::JSJitElementLayout, object_class_id_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, object_flags_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, object_fast_array_mask),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, object_exotic_mask),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, object_union_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, array_size_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, array_count_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, array_data_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, typed_array_ptr_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, typed_array_buffer_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, typed_array_byte_offset_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, typed_array_byte_length_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, typed_array_track_rab_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, array_buffer_data_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, array_buffer_byte_length_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, array_buffer_max_byte_length_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, array_buffer_detached_offset),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, array_buffer_immutable_offset),
            4,
        ),
        (mem::offset_of!(qjs::JSJitElementLayout, array_class_id), 4),
        (
            mem::offset_of!(qjs::JSJitElementLayout, int32_array_class_id),
            4,
        ),
        (
            mem::offset_of!(qjs::JSJitElementLayout, float64_array_class_id),
            4,
        ),
    ] {
        hash = layout_field(hash, offset, size);
    }
    hash
}

/// Byte offset of `JSRefCountHeader::ref_count` in every payload addressed by
/// a JSValue with `JS_VALUE_HAS_REF_COUNT`. Generated code updates the count
/// in place; the linked QuickJS pins this value in its value-layout
/// fingerprint (`0022-inline-refcount.patch`), so a mismatch fails closed.
pub(crate) const REF_COUNT_OFFSET: i32 = 0;
/// Width in bytes of the C `int` reference count.
pub(crate) const REF_COUNT_BYTES: u64 = 4;

fn value_layout_fingerprint() -> u64 {
    let mut hash = layout_start::<qjs::JSValue>();
    hash = hash_u64(hash, (qjs::JS_TAG_FIRST as i64) as u64);
    hash = hash_u64(hash, (qjs::JS_TAG_FLOAT64 as i64) as u64);
    hash = hash_u64(hash, u64::from(cfg!(target_pointer_width = "32")));
    hash = hash_u64(hash, REF_COUNT_OFFSET as u64);
    hash_u64(hash, REF_COUNT_BYTES)
}

fn expected_build_fingerprint() -> u64 {
    let mut hash = hash_u64(FNV_OFFSET, SOURCE_REVISION);
    hash = hash_u64(hash, OPCODE_FINGERPRINT);
    hash = hash_u64(hash, value_layout_fingerprint());
    hash = hash_u64(hash, BUILD_FEATURE_FLAGS);
    hash = hash_u64(hash, abi_info_layout_fingerprint());
    hash = hash_u64(hash, function_id_layout_fingerprint());
    hash = hash_u64(hash, hot_event_layout_fingerprint());
    hash = hash_u64(hash, function_snapshot_layout_fingerprint());
    hash = hash_u64(hash, entry_handle_layout_fingerprint());
    hash = hash_u64(hash, backend_vtable_layout_fingerprint());
    hash = hash_u64(hash, exec_frame_layout_fingerprint());
    hash = hash_u64(hash, exit_layout_fingerprint());
    hash = hash_u64(hash, runtime_api_layout_fingerprint());
    hash = hash_u64(hash, HELPER_TABLE_FINGERPRINT);
    hash = hash_u64(hash, element_layout_fingerprint());
    hash_u64(hash, property_layout_fingerprint())
}

fn query_inline_api() -> Result<qjs::JSJitInlineAPI, AbiError> {
    let pointer = unsafe { qjs::JS_JitGetInlineAPI(qjs::QJSJIT_INLINE_RECOVERY_VERSION) };
    // The runtime query promises static immutable storage. Inspect only the
    // header before copying the full table so a truncated table is rejected.
    if pointer.is_null()
        || unsafe { core::ptr::addr_of!((*pointer).struct_size).read() }
            != mem::size_of::<qjs::JSJitInlineAPI>() as u32
    {
        return Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
            AbiStructure::InlineApi,
        )));
    }
    let api = unsafe { pointer.read() };
    validate_inline_api(&api)?;
    Ok(api)
}

fn validate_inline_api(api: &qjs::JSJitInlineAPI) -> Result<(), AbiError> {
    let effects = qjs::JS_JIT_HELPER_THROWING
        | qjs::JS_JIT_HELPER_ALLOCATING
        | qjs::JS_JIT_HELPER_REENTRANT
        | qjs::JS_JIT_HELPER_FINALIZING;
    if api.struct_size != mem::size_of::<qjs::JSJitInlineAPI>() as u32
        || api.version != qjs::QJSJIT_INLINE_RECOVERY_VERSION
        || api.max_depth != qjs::JS_JIT_INLINE_MAX_DEPTH
        || api.max_bytes != qjs::JS_JIT_INLINE_MAX_BYTES
        || api.effects != effects
        || api.reserved != 0
        || api.enter.is_none()
        || api.leave.is_none()
        || api.resume.is_none()
        || api.check.is_none()
    {
        Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
            AbiStructure::InlineApi,
        )))
    } else {
        Ok(())
    }
}

fn query_array_api() -> Result<qjs::JSJitArrayAPI, AbiError> {
    let pointer = unsafe { qjs::JS_JitGetArrayAPI(qjs::QJSJIT_ARRAY_API_VERSION) };
    if pointer.is_null()
        || unsafe { core::ptr::addr_of!((*pointer).struct_size).read() }
            != mem::size_of::<qjs::JSJitArrayAPI>() as u32
    {
        return Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
            AbiStructure::ArrayApi,
        )));
    }
    let api = unsafe { pointer.read() };
    validate_array_api(&api)?;
    Ok(api)
}

fn validate_array_api(api: &qjs::JSJitArrayAPI) -> Result<(), AbiError> {
    if api.struct_size != mem::size_of::<qjs::JSJitArrayAPI>() as u32
        || api.version != qjs::QJSJIT_ARRAY_API_VERSION
        || api.effects != 0
        || api.reserved != 0
        || api.query.is_none()
    {
        Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
            AbiStructure::ArrayApi,
        )))
    } else {
        Ok(())
    }
}

fn query_iterator_api() -> Result<qjs::JSJitIteratorAPI, AbiError> {
    let pointer = unsafe { qjs::JS_JitGetIteratorAPI(qjs::QJSJIT_ITERATOR_API_VERSION) };
    if pointer.is_null()
        || unsafe { core::ptr::addr_of!((*pointer).struct_size).read() }
            != mem::size_of::<qjs::JSJitIteratorAPI>() as u32
    {
        return Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
            AbiStructure::IteratorApi,
        )));
    }
    let api = unsafe { pointer.read() };
    validate_iterator_api(&api)?;
    Ok(api)
}

fn validate_iterator_api(api: &qjs::JSJitIteratorAPI) -> Result<(), AbiError> {
    if api.struct_size != mem::size_of::<qjs::JSJitIteratorAPI>() as u32
        || api.version != qjs::QJSJIT_ITERATOR_API_VERSION
        || api.effects != 0
        || api.reserved != 0
        || api.array_values_next.is_none()
    {
        Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
            AbiStructure::IteratorApi,
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod iterator_api_tests {
    use super::*;

    #[test]
    fn iterator_api_is_exactly_versioned_and_complete() {
        let pointer = unsafe { qjs::JS_JitGetIteratorAPI(qjs::QJSJIT_ITERATOR_API_VERSION) };
        assert!(!pointer.is_null());
        let valid = unsafe { *pointer };
        assert!(validate_iterator_api(&valid).is_ok());
        assert!(unsafe { qjs::JS_JitGetIteratorAPI(0) }.is_null());
        for field in 0..5 {
            let mut invalid = valid;
            match field {
                0 => invalid.struct_size -= 1,
                1 => invalid.version += 1,
                2 => invalid.effects = 1,
                3 => invalid.reserved = 1,
                _ => invalid.array_values_next = None,
            }
            assert!(validate_iterator_api(&invalid).is_err(), "field {field}");
        }
        let mut info = AbiInfo::linked().unwrap();
        info.iterator_api.effects = 1;
        assert_eq!(
            info.validate(),
            Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
                AbiStructure::IteratorApi
            )))
        );
    }
}

fn query_object_api() -> Result<qjs::JSJitObjectAPI, AbiError> {
    let pointer = unsafe { qjs::JS_JitGetObjectAPI(qjs::QJSJIT_OBJECT_API_VERSION) };
    if pointer.is_null()
        || unsafe { core::ptr::addr_of!((*pointer).struct_size).read() }
            != mem::size_of::<qjs::JSJitObjectAPI>() as u32
    {
        return Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
            AbiStructure::ObjectApi,
        )));
    }
    let api = unsafe { pointer.read() };
    validate_object_api(&api)?;
    Ok(api)
}

/// The object table may allocate and trigger QuickJS' allocation-time cycle
/// collection (C finalizers only). Any other advertised effect, such as a
/// throw or VM reentry, is incompatible with the leaf call sites.
fn validate_object_api(api: &qjs::JSJitObjectAPI) -> Result<(), AbiError> {
    let permitted = qjs::JS_JIT_OBJECT_EFFECT_ALLOCATE | qjs::JS_JIT_OBJECT_EFFECT_COLLECT;
    if api.struct_size != mem::size_of::<qjs::JSJitObjectAPI>() as u32
        || api.version != qjs::QJSJIT_OBJECT_API_VERSION
        || api.effects & !permitted != 0
        || api.max_literal_fields != qjs::JS_JIT_OBJECT_LITERAL_MAX_FIELDS
        || api.literal.is_none()
        || api.retain_shape.is_none()
        || api.array_method.is_none()
        || api.array_push.is_none()
        || api.array_push_method.is_none()
    {
        Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
            AbiStructure::ObjectApi,
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod object_api_tests {
    use super::*;

    fn linked_table() -> qjs::JSJitObjectAPI {
        let pointer = unsafe { qjs::JS_JitGetObjectAPI(qjs::QJSJIT_OBJECT_API_VERSION) };
        assert!(!pointer.is_null());
        unsafe { *pointer }
    }

    #[test]
    fn object_api_is_exactly_versioned_bounded_and_complete() {
        let valid = linked_table();
        assert!(validate_object_api(&valid).is_ok());
        assert!(unsafe { qjs::JS_JitGetObjectAPI(0) }.is_null());
        for field in 0..9 {
            let mut invalid = valid;
            match field {
                0 => invalid.struct_size -= 1,
                1 => invalid.version += 1,
                2 => invalid.effects |= 1 << 8,
                3 => invalid.max_literal_fields += 1,
                4 => invalid.literal = None,
                5 => invalid.retain_shape = None,
                6 => invalid.array_method = None,
                7 => invalid.array_push_method = None,
                _ => invalid.array_push = None,
            }
            assert!(validate_object_api(&invalid).is_err(), "field {field}");
        }
    }

    #[test]
    fn object_table_validation_is_part_of_the_main_abi_contract() {
        let mut info = AbiInfo::linked().unwrap();
        info.object_api.array_push = None;
        assert_eq!(
            info.validate(),
            Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
                AbiStructure::ObjectApi
            )))
        );
    }
}

#[cfg(test)]
mod array_api_tests {
    use super::*;

    fn linked_table() -> qjs::JSJitArrayAPI {
        let pointer = unsafe { qjs::JS_JitGetArrayAPI(qjs::QJSJIT_ARRAY_API_VERSION) };
        assert!(!pointer.is_null());
        unsafe { *pointer }
    }

    #[test]
    fn array_api_is_exactly_versioned_effect_free_and_complete() {
        let valid = linked_table();
        assert!(validate_array_api(&valid).is_ok());
        assert!(unsafe { qjs::JS_JitGetArrayAPI(0) }.is_null());
        for field in 0..5 {
            let mut invalid = valid;
            match field {
                0 => invalid.struct_size -= 1,
                1 => invalid.version += 1,
                2 => invalid.effects = 1,
                3 => invalid.reserved = 1,
                _ => invalid.query = None,
            }
            assert!(validate_array_api(&invalid).is_err(), "field {field}");
        }
    }

    #[test]
    fn array_table_validation_is_part_of_the_main_abi_contract() {
        let mut info = AbiInfo::linked().unwrap();
        assert!(info.array_api.query.is_some());
        info.array_api.effects = 1;
        assert_eq!(
            info.validate(),
            Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
                AbiStructure::ArrayApi
            )))
        );
    }
}

#[cfg(test)]
mod inline_api_tests {
    use super::*;

    fn linked_table() -> qjs::JSJitInlineAPI {
        let pointer = unsafe { qjs::JS_JitGetInlineAPI(qjs::QJSJIT_INLINE_RECOVERY_VERSION) };
        assert!(!pointer.is_null());
        // SAFETY: The linked runtime returns immutable static storage for the
        // requested API version, with its size checked before the full read.
        unsafe {
            assert_eq!(
                (*pointer).struct_size as usize,
                mem::size_of::<qjs::JSJitInlineAPI>()
            );
            *pointer
        }
    }

    #[test]
    fn inline_recovery_requires_the_matching_main_abi_minor() {
        let native = AbiInfo::query_linked().unwrap();
        assert_eq!(ABI_MINOR, native.minor());
        assert_eq!(native.minor(), 25);
        assert!(unsafe { qjs::JS_JitGetInlineAPI(0) }.is_null());
    }

    #[test]
    fn inline_api_rejects_incompatible_header_and_limits() {
        let valid = linked_table();
        assert!(validate_inline_api(&valid).is_ok());
        for field in 0..7 {
            let mut invalid = valid;
            match field {
                0 => invalid.struct_size -= 1,
                1 => invalid.struct_size += 1,
                2 => invalid.version += 1,
                3 => invalid.reserved = 1,
                4 => invalid.max_depth = 0,
                5 => invalid.max_depth += 1,
                _ => invalid.max_bytes -= 1,
            }
            assert!(validate_inline_api(&invalid).is_err(), "field {field}");
        }
    }

    #[test]
    fn inline_api_requires_all_observable_effects_and_no_unknown_effects() {
        let valid = linked_table();
        for effect in [
            qjs::JS_JIT_HELPER_THROWING,
            qjs::JS_JIT_HELPER_ALLOCATING,
            qjs::JS_JIT_HELPER_REENTRANT,
            qjs::JS_JIT_HELPER_FINALIZING,
        ] {
            let mut invalid = valid;
            invalid.effects &= !effect;
            assert!(validate_inline_api(&invalid).is_err(), "effect {effect}");
        }
        let mut invalid = valid;
        invalid.effects |= 1 << 31;
        assert!(validate_inline_api(&invalid).is_err());
    }

    #[test]
    fn inline_api_requires_every_native_operation() {
        let valid = linked_table();
        for field in 0..4 {
            let mut invalid = valid;
            match field {
                0 => invalid.enter = None,
                1 => invalid.leave = None,
                2 => invalid.resume = None,
                _ => invalid.check = None,
            }
            assert!(validate_inline_api(&invalid).is_err(), "operation {field}");
        }
    }

    #[test]
    fn inline_table_validation_is_part_of_the_main_abi_contract() {
        let mut info = AbiInfo::linked().unwrap();
        assert!(info.inline_api().enter.is_some());
        info.inline_api.resume = None;
        assert_eq!(
            info.validate(),
            Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
                AbiStructure::InlineApi
            )))
        );
        // A mismatched main ABI must be rejected before any inline API use.
        info.raw.major ^= 1;
        assert_eq!(
            info.validate(),
            Err(AbiError::Incompatible(AbiMismatch::Major))
        );
    }

    #[test]
    fn missing_inline_check_is_rejected_by_the_main_abi_validator() {
        let mut info = AbiInfo::linked().unwrap();
        info.inline_api.check = None;
        assert_eq!(
            info.validate(),
            Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
                AbiStructure::InlineApi
            )))
        );
    }
}

#[cfg(test)]
mod property_layout_tests {
    use super::*;

    #[test]
    fn property_layout_mismatch_is_rejected() {
        let valid = AbiInfo::linked().unwrap();
        for field in 0..3 {
            let mut invalid = valid;
            match field {
                0 => invalid.raw.property_layout_fingerprint ^= 1,
                1 => invalid.raw.property_layout.struct_size -= 1,
                _ => invalid.raw.property_layout.shape_generation_offset = u32::MAX,
            }
            assert_eq!(
                invalid.validate(),
                Err(AbiError::Incompatible(AbiMismatch::StructureLayout(
                    AbiStructure::PropertyLayout
                )))
            );
        }
    }
}
