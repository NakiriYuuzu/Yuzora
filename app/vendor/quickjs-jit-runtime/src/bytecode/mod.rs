//! Owned QuickJS bytecode snapshots and bounded verification.

mod cfg;
mod decode;
mod policy;
mod stack;
mod verify;

use std::{marker::PhantomData, ptr::NonNull, rc::Rc, slice, sync::Arc};

use rquickjs_core::qjs;

pub use cfg::{BasicBlock, ControlFlowGraph};
pub use decode::{
    decode_raw, linked_opcode_table, DecodeError, Instruction, Opcode, OperandFormat,
};
pub use policy::{
    audited_opcode_policy_table, tier1_policy, AuditedOpcodePolicy, FallbackReason, HelperId,
    Tier1Policy, Tier1Rejection, GENERATED_OPCODE_COUNT, GENERATED_OPCODE_FINGERPRINT,
};
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
pub(crate) use stack::effective_pop;
#[cfg(feature = "compiler")]
pub(crate) use stack::local_index;
pub use stack::{ExceptionHandler, SlotKind};
pub use verify::{Resource, VerifiedFunction, VerifyError, VerifyErrorKind, VerifyLimits};
#[cfg(feature = "compiler")]
pub(crate) use verify::{
    SPECIAL_OBJECT_IMPORT_META, SPECIAL_OBJECT_MAPPED_ARGUMENTS, SPECIAL_OBJECT_MAX,
};

/// Numeric opcode constants generated from QuickJS's authoritative macro table.
pub mod opcode {
    use rquickjs_core::qjs;

    pub const PUSH_UNDEFINED: u8 = qjs::QJS_JIT_OP_UNDEFINED;
    pub const PUSH_I32: u8 = qjs::QJS_JIT_OP_PUSH_I32;
    pub const PUSH_I8: u8 = qjs::QJS_JIT_OP_PUSH_I8;
    pub const PUSH_TRUE: u8 = qjs::QJS_JIT_OP_PUSH_TRUE;
    pub const PUSH_THIS: u8 = qjs::QJS_JIT_OP_PUSH_THIS;
    pub const PUSH_CONST8: u8 = qjs::QJS_JIT_OP_PUSH_CONST8;
    pub const ADD: u8 = qjs::QJS_JIT_OP_ADD;
    pub const PLUS: u8 = qjs::QJS_JIT_OP_PLUS;
    pub const DROP: u8 = qjs::QJS_JIT_OP_DROP;
    pub const DUP: u8 = qjs::QJS_JIT_OP_DUP;
    pub const GET_LOC: u8 = qjs::QJS_JIT_OP_GET_LOC;
    pub const GET_LOC0_LOC1: u8 = qjs::QJS_JIT_OP_GET_LOC0_LOC1;
    pub const GET_ARG: u8 = qjs::QJS_JIT_OP_GET_ARG;
    pub const GET_VAR_REF: u8 = qjs::QJS_JIT_OP_GET_VAR_REF;
    pub const PUT_LOC: u8 = qjs::QJS_JIT_OP_PUT_LOC;
    pub const SET_LOC_UNINITIALIZED: u8 = qjs::QJS_JIT_OP_SET_LOC_UNINITIALIZED;
    pub const FOR_OF_START: u8 = qjs::QJS_JIT_OP_FOR_OF_START;
    pub const USING_DISPOSE_INIT: u8 = qjs::QJS_JIT_OP_USING_DISPOSE_INIT;
    pub const INC_LOC: u8 = qjs::QJS_JIT_OP_INC_LOC;
    pub const DEC_LOC: u8 = qjs::QJS_JIT_OP_DEC_LOC;
    pub const ADD_LOC: u8 = qjs::QJS_JIT_OP_ADD_LOC;
    pub const MAKE_LOC_REF: u8 = qjs::QJS_JIT_OP_MAKE_LOC_REF;
    pub const MAKE_ARG_REF: u8 = qjs::QJS_JIT_OP_MAKE_ARG_REF;
    pub const MAKE_VAR_REF_REF: u8 = qjs::QJS_JIT_OP_MAKE_VAR_REF_REF;
    pub const USING_DISPOSE: u8 = qjs::QJS_JIT_OP_USING_DISPOSE;
    pub const USING_DISPOSE_ASYNC: u8 = qjs::QJS_JIT_OP_USING_DISPOSE_ASYNC;
    pub const EVAL: u8 = qjs::QJS_JIT_OP_EVAL;
    pub const WITH_GET_VAR: u8 = qjs::QJS_JIT_OP_WITH_GET_VAR;
    pub const INITIAL_YIELD: u8 = qjs::QJS_JIT_OP_INITIAL_YIELD;
    pub const AWAIT: u8 = qjs::QJS_JIT_OP_AWAIT;
    pub const IF_FALSE8: u8 = qjs::QJS_JIT_OP_IF_FALSE8;
    pub const GOTO8: u8 = qjs::QJS_JIT_OP_GOTO8;
    pub const CATCH: u8 = qjs::QJS_JIT_OP_CATCH;
    pub const RETURN: u8 = qjs::QJS_JIT_OP_RETURN;
    pub const RETURN_UNDEF: u8 = qjs::QJS_JIT_OP_RETURN_UNDEF;
    pub const THROW_ERROR: u8 = qjs::QJS_JIT_OP_THROW_ERROR;
    pub const NOP: u8 = qjs::QJS_JIT_OP_NOP;
}

/// A pointer-free description of one entry in the QuickJS constant pool.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConstantDescriptor {
    index: u32,
    tag: i32,
    kind: u32,
    payload: u64,
}

/// Scalar execution flags copied from the QuickJS bytecode function.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FunctionFlags(u32);

impl FunctionFlags {
    const STRICT: u32 = qjs::JS_JIT_FUNCTION_STRICT;
    const SUPPORTED: u32 = Self::STRICT;

    const fn from_raw(bits: u32) -> Option<Self> {
        if bits & !Self::SUPPORTED == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    pub const fn non_strict() -> Self {
        Self(0)
    }

    pub const fn strict() -> Self {
        Self(Self::STRICT)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn is_strict(self) -> bool {
        self.0 & Self::STRICT != 0
    }
}

impl ConstantDescriptor {
    pub const fn index(self) -> u32 {
        self.index
    }

    pub const fn tag(self) -> i32 {
        self.tag
    }

    pub const fn payload(self) -> u64 {
        self.payload
    }

    pub const fn kind(self) -> u32 {
        self.kind
    }
}

/// An OSR request and the complete logical frame state at its PC.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OsrPoint {
    pc: u32,
    live_slots: Vec<SlotKind>,
}

impl OsrPoint {
    pub fn new(pc: u32, live_slots: Vec<SlotKind>) -> Self {
        Self { pc, live_slots }
    }

    pub const fn pc(&self) -> u32 {
        self.pc
    }

    pub fn live_slots(&self) -> &[SlotKind] {
        &self.live_slots
    }
}

/// A deoptimization request with states on both sides of a side effect.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeoptPoint {
    pc: u32,
    before: Vec<SlotKind>,
    after: Vec<SlotKind>,
}

impl DeoptPoint {
    pub fn new(pc: u32, before: Vec<SlotKind>, after: Vec<SlotKind>) -> Self {
        Self { pc, before, after }
    }

    pub const fn pc(&self) -> u32 {
        self.pc
    }

    pub fn before(&self) -> &[SlotKind] {
        &self.before
    }

    pub fn after(&self) -> &[SlotKind] {
        &self.after
    }
}

/// Compiler-side metadata that is itself owned and pointer-free.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct VerifierMetadata {
    osr_points: Vec<OsrPoint>,
    deopt_points: Vec<DeoptPoint>,
}

impl VerifierMetadata {
    pub fn new(osr_points: Vec<OsrPoint>, deopt_points: Vec<DeoptPoint>) -> Self {
        Self {
            osr_points,
            deopt_points,
        }
    }

    fn byte_len(&self) -> usize {
        let osr = self
            .osr_points
            .iter()
            .map(|point| 4 + point.live_slots.len())
            .sum::<usize>();
        let deopt = self
            .deopt_points
            .iter()
            .map(|point| 4 + point.before.len() + point.after.len())
            .sum::<usize>();
        osr.saturating_add(deopt)
    }
}

#[derive(Clone, Debug)]
struct SnapshotData {
    function_id: u64,
    generation: u64,
    source_revision: u64,
    opcode_fingerprint: u64,
    flags: FunctionFlags,
    bytecode: Vec<u8>,
    arg_count: u16,
    local_count: u16,
    closure_count: u16,
    stack_size: u16,
    constants: Vec<ConstantDescriptor>,
    constant_count: u32,
    exception_map: Vec<u8>,
    source_map: Vec<u8>,
    metadata: VerifierMetadata,
}

/// Worker-safe snapshot containing copied bytes, scalars, and descriptors only.
#[derive(Clone, Debug)]
pub struct CompileSnapshot {
    data: Arc<SnapshotData>,
}

impl CompileSnapshot {
    pub fn bytecode(&self) -> &[u8] {
        &self.data.bytecode
    }

    pub fn function_id(&self) -> u64 {
        self.data.function_id
    }

    pub fn generation(&self) -> u64 {
        self.data.generation
    }

    pub fn arg_count(&self) -> u16 {
        self.data.arg_count
    }

    pub fn local_count(&self) -> u16 {
        self.data.local_count
    }

    pub fn closure_count(&self) -> u16 {
        self.data.closure_count
    }

    pub fn stack_size(&self) -> u16 {
        self.data.stack_size
    }

    pub fn source_revision(&self) -> u64 {
        self.data.source_revision
    }

    pub fn opcode_fingerprint(&self) -> u64 {
        self.data.opcode_fingerprint
    }

    pub fn flags(&self) -> FunctionFlags {
        self.data.flags
    }

    pub fn constants(&self) -> &[ConstantDescriptor] {
        &self.data.constants
    }

    pub fn constant_count(&self) -> u32 {
        self.data.constant_count
    }

    pub fn exception_map(&self) -> &[u8] {
        &self.data.exception_map
    }

    pub fn source_map(&self) -> &[u8] {
        &self.data.source_map
    }

    pub fn owned_bytes(&self) -> usize {
        self.data
            .bytecode
            .len()
            .saturating_add(self.data.exception_map.len())
            .saturating_add(self.data.source_map.len())
            .saturating_add(
                self.data
                    .constants
                    .len()
                    .saturating_mul(core::mem::size_of::<ConstantDescriptor>()),
            )
    }

    /// Storage kept alive by a snapshot clone, including spare vector capacity
    /// and verifier metadata. Shared storage is charged conservatively to each
    /// retaining artifact/request; allocator bookkeeping is excluded.
    pub fn retained_bytes(&self) -> usize {
        fn vector_bytes<T>(values: &Vec<T>) -> usize {
            values.capacity().saturating_mul(core::mem::size_of::<T>())
        }
        let data = &self.data;
        let mut bytes =
            core::mem::size_of::<SnapshotData>().saturating_add(2 * core::mem::size_of::<usize>());
        for size in [
            vector_bytes(&data.bytecode),
            vector_bytes(&data.constants),
            vector_bytes(&data.exception_map),
            vector_bytes(&data.source_map),
            vector_bytes(&data.metadata.osr_points),
            vector_bytes(&data.metadata.deopt_points),
        ] {
            bytes = bytes.saturating_add(size);
        }
        for point in &data.metadata.osr_points {
            bytes = bytes.saturating_add(vector_bytes(&point.live_slots));
        }
        for point in &data.metadata.deopt_points {
            bytes = bytes
                .saturating_add(vector_bytes(&point.before))
                .saturating_add(vector_bytes(&point.after));
        }
        bytes
    }

    pub fn decode(&self) -> Result<Vec<Instruction>, DecodeError> {
        decode_raw(self.bytecode())
    }

    pub fn verify(&self, limits: VerifyLimits) -> Result<VerifiedFunction, VerifyError> {
        verify::verify(self.clone(), limits)
    }

    pub fn with_metadata(mut self, metadata: VerifierMetadata) -> Self {
        Arc::make_mut(&mut self.data).metadata = metadata;
        self
    }

    #[doc(hidden)]
    pub fn with_exception_map(mut self, exception_map: Vec<u8>) -> Self {
        Arc::make_mut(&mut self.data).exception_map = exception_map;
        self
    }

    /// Creates a pointer-free snapshot from untrusted serialized bytecode.
    ///
    /// The result is not suitable for compilation until [`Self::verify`]
    /// succeeds. The supplied counts are checked against every indexed opcode.
    pub fn from_untrusted_bytecode(
        bytecode: Vec<u8>,
        arg_count: u16,
        local_count: u16,
        closure_count: u16,
        constant_count: u32,
    ) -> Self {
        Self::from_untrusted_bytecode_with_flags(
            bytecode,
            arg_count,
            local_count,
            closure_count,
            constant_count,
            FunctionFlags::non_strict(),
        )
    }

    /// Creates an untrusted snapshot with explicit scalar execution flags.
    pub fn from_untrusted_bytecode_with_flags(
        bytecode: Vec<u8>,
        arg_count: u16,
        local_count: u16,
        closure_count: u16,
        constant_count: u32,
        flags: FunctionFlags,
    ) -> Self {
        Self {
            data: Arc::new(SnapshotData {
                function_id: 0,
                generation: 0,
                source_revision: crate::abi::SOURCE_REVISION,
                opcode_fingerprint: crate::abi::OPCODE_FINGERPRINT,
                flags,
                bytecode,
                arg_count,
                local_count,
                closure_count,
                stack_size: u16::MAX,
                constants: Vec::new(),
                constant_count,
                exception_map: Vec::new(),
                source_map: Vec::new(),
                metadata: VerifierMetadata::default(),
            }),
        }
    }

    /// Copies a QuickJS function into a pointer-free compiler snapshot.
    ///
    /// # Safety
    ///
    /// `ctx` and `function` must belong to the same locked runtime, and the
    /// function value must remain alive for the duration of this call.
    pub unsafe fn capture_raw(
        ctx: *mut qjs::JSContext,
        function: qjs::JSValueConst,
    ) -> Result<Self, SnapshotStatus> {
        let mut raw = std::ptr::null_mut();
        let status = unsafe { qjs::JS_JitSnapshotFunction(ctx, function, &mut raw) };
        if status != 0 {
            return Err(SnapshotStatus::from_raw(status));
        }
        let raw = std::ptr::NonNull::new(raw).ok_or(SnapshotStatus::InvalidBytecode)?;
        let result = unsafe { Self::copy_raw(raw.as_ref()) };
        unsafe { qjs::JS_JitFreeSnapshot(raw.as_ptr()) };
        result
    }

    /// Splits runtime-owned heap constants from the worker-safe snapshot.
    ///
    /// # Safety
    ///
    /// `ctx` and `function` must belong to `runtime` while its lock is held,
    /// and the function value must remain alive for the duration of this call.
    pub unsafe fn capture_with_runtime_constants(
        runtime: &rquickjs_core::Runtime,
        ctx: *mut qjs::JSContext,
        function: qjs::JSValueConst,
    ) -> Result<(Self, RuntimeConstants), SnapshotStatus> {
        let snapshot = unsafe { Self::capture_raw(ctx, function)? };
        let constants = unsafe {
            RuntimeConstants::capture(runtime, ctx, function, snapshot.constant_count())?
        };
        Ok((snapshot, constants))
    }

    unsafe fn copy_raw(raw: &qjs::JSJitFunctionSnapshot) -> Result<Self, SnapshotStatus> {
        if raw.struct_size as usize != std::mem::size_of::<qjs::JSJitFunctionSnapshot>()
            || raw.source_revision != crate::abi::SOURCE_REVISION
            || raw.opcode_fingerprint != crate::abi::OPCODE_FINGERPRINT
        {
            return Err(SnapshotStatus::IncompatibleAbi);
        }
        let flags = FunctionFlags::from_raw(raw.flags).ok_or(SnapshotStatus::IncompatibleAbi)?;
        const MAX_RAW_SNAPSHOT: usize = 64 * 1024 * 1024;
        let total = (raw.bytecode_len as usize)
            .saturating_add(raw.exception_map_len as usize)
            .saturating_add(raw.source_map_len as usize)
            .saturating_add(
                (raw.constant_count as usize)
                    .saturating_mul(std::mem::size_of::<qjs::JSJitConstantDescriptor>()),
            );
        if total > MAX_RAW_SNAPSHOT {
            return Err(SnapshotStatus::TooLarge);
        }

        unsafe fn copied<T: Copy>(pointer: *const T, len: usize) -> Result<Vec<T>, SnapshotStatus> {
            if len == 0 {
                return Ok(Vec::new());
            }
            if pointer.is_null() {
                return Err(SnapshotStatus::InvalidBytecode);
            }
            Ok(unsafe { slice::from_raw_parts(pointer, len) }.to_vec())
        }

        let constants = unsafe { copied(raw.constants, raw.constant_count as usize)? }
            .into_iter()
            .map(|descriptor| ConstantDescriptor {
                index: descriptor.index,
                tag: descriptor.tag,
                kind: descriptor.kind,
                payload: descriptor.payload,
            })
            .collect();
        Ok(Self {
            data: Arc::new(SnapshotData {
                function_id: raw.function.id,
                generation: raw.function.generation,
                source_revision: raw.source_revision,
                opcode_fingerprint: raw.opcode_fingerprint,
                flags,
                bytecode: unsafe { copied(raw.bytecode, raw.bytecode_len as usize)? },
                arg_count: raw.arg_count,
                local_count: raw.local_count,
                closure_count: raw.closure_count,
                stack_size: raw.stack_size,
                constants,
                constant_count: raw.constant_count,
                exception_map: unsafe {
                    copied(raw.exception_map, raw.exception_map_len as usize)?
                },
                source_map: unsafe { copied(raw.source_map, raw.source_map_len as usize)? },
                metadata: VerifierMetadata::default(),
            }),
        })
    }

    /// Copies one callback-borrowed C snapshot into worker-safe owned memory.
    ///
    /// # Safety
    /// `raw` and every slice described by it must remain valid for this call.
    pub unsafe fn copy_borrowed_raw(
        raw: &qjs::JSJitFunctionSnapshot,
    ) -> Result<Self, SnapshotStatus> {
        unsafe { Self::copy_raw(raw) }
    }
}

/// Runtime-thread owner for the source function and its constant pool.
///
/// Task 3 retains the constant pool transitively through the duplicated source
/// function. The versioned per-index runtime resolver belongs to Task 8; until
/// then this type deliberately exposes no `JSValue` or raw heap handle.
pub struct RuntimeConstants {
    _runtime: rquickjs_core::Runtime,
    ctx: NonNull<qjs::JSContext>,
    retained_function: qjs::JSValue,
    constant_count: u32,
    _runtime_thread: PhantomData<Rc<()>>,
}

impl RuntimeConstants {
    unsafe fn capture(
        runtime: &rquickjs_core::Runtime,
        ctx: *mut qjs::JSContext,
        function: qjs::JSValueConst,
        constant_count: u32,
    ) -> Result<Self, SnapshotStatus> {
        let ctx = NonNull::new(unsafe { qjs::JS_DupContext(ctx) })
            .ok_or(SnapshotStatus::InvalidArgument)?;
        let retained_function = unsafe { qjs::JS_DupValue(ctx.as_ptr(), function) };
        Ok(Self {
            _runtime: runtime.clone(),
            ctx,
            retained_function,
            constant_count,
            _runtime_thread: PhantomData,
        })
    }

    pub fn len(&self) -> usize {
        self.constant_count as usize
    }

    pub fn is_empty(&self) -> bool {
        self.constant_count == 0
    }
}

impl Drop for RuntimeConstants {
    fn drop(&mut self) {
        unsafe {
            qjs::JS_FreeValue(self.ctx.as_ptr(), self.retained_function);
            qjs::JS_FreeContext(self.ctx.as_ptr());
        }
    }
}

/// Categorized result returned by the QuickJS snapshot adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotStatus {
    Ok,
    InvalidArgument,
    NotBytecode,
    Generator,
    Async,
    Eval,
    With,
    OutOfMemory,
    TooLarge,
    InvalidBytecode,
    IncompatibleAbi,
}

impl SnapshotStatus {
    pub(crate) const fn from_raw(status: i32) -> Self {
        match status {
            0 => Self::Ok,
            -1 => Self::InvalidArgument,
            -2 => Self::NotBytecode,
            -3 => Self::Generator,
            -4 => Self::Async,
            -5 => Self::Eval,
            -6 => Self::With,
            -7 => Self::OutOfMemory,
            -8 => Self::TooLarge,
            _ => Self::InvalidBytecode,
        }
    }
}
