//! P3a native-to-native call convention, first fail-closed slice.
//!
//! A *native call entry* is a secondary machine entry published beside a
//! Tier 2 artifact for a pure, self-recursive scalar function (see
//! `docs/roadmap/p3a-native-calls.md`). Its ABI is
//!
//! ```text
//! status = entry(nc: *mut NativeCallContext, out: *mut Scalar, vsp: usize, args: Scalar...)
//! ```
//!
//! where each `Scalar` is `i32` (Int32) or `f64` (Float64) as fixed by the
//! function's bounded feedback signature. Status zero stores the result. Any
//! other status means the chain performed no observable effect and the
//! outermost compiled caller must re-execute its CALL in the interpreter. The body never calls a helper,
//! allocates, reads or writes the heap, or materializes a `JSStackFrame`; its
//! only calls are to itself. The self reference (`get_var <atom>`) is not
//! evaluated natively: [`qjsjit_native_call_begin`] proves once per chain that
//! the binding is a plain data property holding the callee in its realm.

use crate::bytecode::VerifiedFunction;
use crate::runtime::{BoundedSpecializationSignature, FeedbackRepresentation};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::{
    types, AbiParam, Block, ExtFuncData, ExternalName, Function, InstBuilder, MemFlags, Signature,
    StackSlotData, StackSlotKind, UserExternalName, Value,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use std::collections::BTreeMap;

use super::{CompileControl, CompileFailure};

/// User external-name namespace of a native entry's call to itself. Final
/// relocation resolves it to the entry's own base address.
pub(crate) const NATIVE_SELF_NAMESPACE: u32 = 0x5033_a001;
/// User external-name namespace of a Tier 2 body's reference to the native
/// entry published in the same artifact.
pub(crate) const NATIVE_ENTRY_NAMESPACE: u32 = 0x5033_a002;
/// Relocation symbol for [`NATIVE_ENTRY_NAMESPACE`].
pub(crate) const NATIVE_ENTRY_SYMBOL: &str = "qjsjit.native_call_entry";

/// Status returned by a native entry after a successful return.
pub(crate) const NATIVE_CALL_OK: i64 = 0;
/// Status requesting interpreter re-execution of the outermost CALL.
pub(crate) const NATIVE_CALL_RETRY: i64 = 1;

const FLAG_STACK_EXHAUSTED: u32 = 1 << 0;

/// Per-level charge used when the interpreter frame cannot be measured. It
/// is far above any measured build (about 2 KiB for x86_64 release), so the
/// fallback only shortens native recursion.
const INTERPRETER_FRAME_FALLBACK: u32 = 32 * 1024;
/// Lower bound on the per-level charge; it also covers the native entry's
/// own frame, which is much smaller.
const INTERPRETER_FRAME_MINIMUM: u32 = 1024;
/// Upper bound on generated self-recursive bodies.
const MAX_NATIVE_CALL_INSTRUCTIONS: usize = 256;

/// Layout-checked mirror of `JSJitNativeCallContext` (patch 0029).
#[repr(C)]
#[derive(Debug)]
pub(crate) struct NativeCallContext {
    stack_limit: usize,
    budget: i32,
    flags: u32,
    ctx: *mut rquickjs_core::qjs::JSContext,
}

const OFFSET_STACK_LIMIT: i32 = 0;
const OFFSET_BUDGET: i32 = 8;
const OFFSET_FLAGS: i32 = 12;
const CONTEXT_SIZE: u32 = 24;

unsafe extern "C" {
    fn JS_JitNativeCallBegin(
        ctx: *mut rquickjs_core::qjs::JSContext,
        function: *mut core::ffi::c_void,
        atom: u32,
        nc: *mut NativeCallContext,
        sp: usize,
    ) -> core::ffi::c_int;
    fn JS_JitNativeCallPoll(nc: *mut NativeCallContext) -> core::ffi::c_int;
    fn JS_JitNativeCallEnd(nc: *mut NativeCallContext, sp: usize);
    fn JS_JitNativeCallContextLayout(offsets: *mut usize, count: usize) -> usize;
}

static INTERPRETER_FRAME_BYTES: std::sync::OnceLock<u32> = std::sync::OnceLock::new();

/// C stack bytes one interpreter call level of a minimal function uses in
/// this build (`JS_CallInternal`'s frame plus its value allocation), with a
/// 25% margin. Native entries charge it per level on top of their own value
/// slots, so a native chain fails before the depth at which the interpreter
/// would throw `RangeError`, whatever the C compiler and flags produced.
pub fn interpreter_frame_bytes() -> u32 {
    *INTERPRETER_FRAME_BYTES.get_or_init(|| {
        measure_interpreter_frame()
            .map_or(INTERPRETER_FRAME_FALLBACK, |bytes| {
                bytes.saturating_add(bytes / 4)
            })
            .max(INTERPRETER_FRAME_MINIMUM)
    })
}

/// Measures the distance between the C stack pointers seen by a host probe
/// called at recursion depths 0 and 64 of an interpreted JS function, in a
/// private runtime without a JIT.
fn measure_interpreter_frame() -> Option<u32> {
    use rquickjs_core::{Context, Function, Runtime};
    const LEVELS: f64 = 64.0;
    let runtime = Runtime::new().ok()?;
    let context = Context::full(&runtime).ok()?;
    context.with(|ctx| {
        let probe = Function::new(ctx.clone(), || -> f64 {
            let marker = 0u8;
            core::hint::black_box(&marker) as *const u8 as usize as f64
        })
        .ok()?;
        ctx.globals().set("__qjsjit_stack_probe", probe).ok()?;
        ctx.eval::<(), _>(
            "function __qjsjit_depth(n){return n<=0?__qjsjit_stack_probe():__qjsjit_depth(n-1);}",
        )
        .ok()?;
        let shallow: f64 = ctx.eval("__qjsjit_depth(0)").ok()?;
        let deep: f64 = ctx.eval("__qjsjit_depth(64)").ok()?;
        let bytes = (shallow - deep) / LEVELS;
        (bytes.is_finite() && bytes > 0.0 && bytes < f64::from(INTERPRETER_FRAME_FALLBACK))
            .then(|| bytes.ceil() as u32)
    })
}

/// True when the C context layout matches the offsets generated code uses.
pub(crate) fn context_layout_matches() -> bool {
    let mut offsets = [0usize; 4];
    let size = unsafe { JS_JitNativeCallContextLayout(offsets.as_mut_ptr(), offsets.len()) };
    size == CONTEXT_SIZE as usize
        && size == core::mem::size_of::<NativeCallContext>()
        && offsets
            == [
                OFFSET_STACK_LIMIT as usize,
                OFFSET_BUDGET as usize,
                OFFSET_FLAGS as usize,
                16,
            ]
}

/// Admits one outermost native chain. `sp` is the caller's stack pointer.
/// Returns zero when the chain may run.
///
/// While the interpreter retries a chain that ran out of stack, chains
/// started below that chain's caller are refused, so the doomed recursion is
/// not retried natively at every level. The runtime-owned floor is cleared by
/// the retry's own stack overflow error, by a chain at or above the floor, or
/// after a refusal budget bounded by the remaining stack (patch 0029), so one
/// caught `RangeError` never disables native calls for a stack height.
///
/// # Safety
/// `ctx` must be the live context of the calling frame, `function` a live
/// object guarded by the caller, and `nc` writable for the whole chain.
pub(crate) unsafe extern "C" fn qjsjit_native_call_begin(
    ctx: *mut rquickjs_core::qjs::JSContext,
    function: *mut core::ffi::c_void,
    atom: u32,
    nc: *mut NativeCallContext,
    sp: usize,
) -> i32 {
    if unsafe { JS_JitNativeCallBegin(ctx, function, atom, nc, sp) } != 0 {
        1
    } else {
        0
    }
}

/// Publishes interrupt accounting and records stack exhaustion.
///
/// # Safety
/// `nc` must have been admitted by [`qjsjit_native_call_begin`].
pub(crate) unsafe extern "C" fn qjsjit_native_call_end(nc: *mut NativeCallContext, sp: usize) {
    unsafe { JS_JitNativeCallEnd(nc, sp) };
}

/// Validated description of a function's native entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeCallPlan {
    atom: u32,
    arguments: Box<[FeedbackRepresentation]>,
    result: FeedbackRepresentation,
    frame_charge: u32,
}

impl NativeCallPlan {
    pub const fn atom(&self) -> u32 {
        self.atom
    }
    pub fn arity(&self) -> usize {
        self.arguments.len()
    }
    /// Unboxed representation of each argument (Int32 or Float64).
    pub fn arguments(&self) -> &[FeedbackRepresentation] {
        &self.arguments
    }
    /// Unboxed representation of the result (Int32 or Float64).
    pub const fn result(&self) -> FeedbackRepresentation {
        self.result
    }
    pub const fn frame_charge(&self) -> u32 {
        self.frame_charge
    }
}

/// Returns the native-entry plan when the function is a pure, self-recursive
/// Int32/Float64 function under `signature`. This builds (and discards) the
/// same CLIF the lowering publishes, so admission and lowering cannot disagree.
pub fn plan(
    function: &VerifiedFunction,
    signature: &BoundedSpecializationSignature,
) -> Option<NativeCallPlan> {
    if !context_layout_matches() {
        return None;
    }
    // Admission only builds the CLIF; the host ABI does not affect which
    // bytecode shapes the lowering accepts.
    build(
        types::I64,
        cranelift_codegen::isa::CallConv::SystemV,
        function,
        signature,
    )
    .ok()
    .map(|(plan, _)| plan)
}

/// Lowers the native entry. Fails closed for every unsupported shape.
pub(crate) fn lower(
    isa: &cranelift_codegen::isa::OwnedTargetIsa,
    function: &VerifiedFunction,
    signature: &BoundedSpecializationSignature,
    control: Option<&CompileControl>,
) -> Result<(NativeCallPlan, super::baseline::RelocatableCode), CompileFailure> {
    if !context_layout_matches() {
        return Err(CompileFailure::InvalidArtifact);
    }
    let (plan, clif) = build(
        isa.pointer_type(),
        isa.default_call_conv(),
        function,
        signature,
    )?;
    let code = super::baseline::finalize_optimized_machine(isa, clif, control, false)?;
    Ok((plan, code))
}

/// Machine type of an unboxed native-call scalar.
pub(crate) fn scalar_type(representation: FeedbackRepresentation) -> Option<types::Type> {
    match representation {
        FeedbackRepresentation::Int32 => Some(types::I32),
        FeedbackRepresentation::Float64 => Some(types::F64),
        FeedbackRepresentation::Bool | FeedbackRepresentation::HeapRef => None,
    }
}

/// The native entry signature: `(nc, out, vsp, scalar args...) -> i32`.
pub(crate) fn entry_signature(
    pointer_type: types::Type,
    call_conv: cranelift_codegen::isa::CallConv,
    arguments: &[FeedbackRepresentation],
) -> Result<Signature, CompileFailure> {
    let mut abi = Signature::new(call_conv);
    abi.params.push(AbiParam::new(pointer_type));
    abi.params.push(AbiParam::new(pointer_type));
    abi.params.push(AbiParam::new(pointer_type));
    for argument in arguments {
        abi.params.push(AbiParam::new(
            scalar_type(*argument).ok_or(CompileFailure::UnsupportedOpcode)?,
        ));
    }
    abi.returns.push(AbiParam::new(types::I32));
    Ok(abi)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Kind {
    Int32,
    Float64,
    Bool,
    SelfFunction,
}

#[derive(Clone, Copy)]
enum Slot {
    Int32(Value),
    Float64(Value),
    Bool(Value),
    SelfFunction,
}

impl Slot {
    const fn kind(self) -> Kind {
        match self {
            Self::Int32(_) => Kind::Int32,
            Self::Float64(_) => Kind::Float64,
            Self::Bool(_) => Kind::Bool,
            Self::SelfFunction => Kind::SelfFunction,
        }
    }

    const fn value(self) -> Option<Value> {
        match self {
            Self::Int32(value) | Self::Float64(value) | Self::Bool(value) => Some(value),
            Self::SelfFunction => None,
        }
    }

    fn from_representation(
        representation: FeedbackRepresentation,
        value: Value,
    ) -> Result<Self, CompileFailure> {
        match representation {
            FeedbackRepresentation::Int32 => Ok(Self::Int32(value)),
            FeedbackRepresentation::Float64 => Ok(Self::Float64(value)),
            FeedbackRepresentation::Bool | FeedbackRepresentation::HeapRef => {
                Err(CompileFailure::UnsupportedOpcode)
            }
        }
    }
}

/// A JavaScript number as a machine value of the requested representation.
/// Int32 widens exactly to Float64; Float64 never narrows.
fn number_as(
    builder: &mut FunctionBuilder<'_>,
    slot: Slot,
    representation: FeedbackRepresentation,
) -> Result<Value, CompileFailure> {
    match (slot, representation) {
        (Slot::Int32(value), FeedbackRepresentation::Int32)
        | (Slot::Float64(value), FeedbackRepresentation::Float64) => Ok(value),
        (Slot::Int32(value), FeedbackRepresentation::Float64) => {
            Ok(builder.ins().fcvt_from_sint(types::F64, value))
        }
        _ => Err(CompileFailure::UnsupportedOpcode),
    }
}

/// Both operands as Float64 when either is Float64; `None` when both Int32.
fn float_operands(
    builder: &mut FunctionBuilder<'_>,
    lhs: Slot,
    rhs: Slot,
) -> Result<Option<(Value, Value)>, CompileFailure> {
    match (lhs, rhs) {
        (Slot::Int32(_), Slot::Int32(_)) => Ok(None),
        (Slot::Int32(_) | Slot::Float64(_), Slot::Int32(_) | Slot::Float64(_)) => Ok(Some((
            number_as(builder, lhs, FeedbackRepresentation::Float64)?,
            number_as(builder, rhs, FeedbackRepresentation::Float64)?,
        ))),
        _ => Err(CompileFailure::UnsupportedOpcode),
    }
}

struct BlockState {
    block: Block,
    kinds: Option<Vec<Kind>>,
}

fn int_constant(name: &str, bytes: &[u8]) -> Result<Option<i64>, CompileFailure> {
    Ok(match name {
        "push_minus1" => Some(-1),
        "push_0" | "push_1" | "push_2" | "push_3" | "push_4" | "push_5" | "push_6" | "push_7" => {
            Some(i64::from(name.as_bytes()[5] - b'0'))
        }
        "push_i8" => Some(i64::from(
            *bytes.get(1).ok_or(CompileFailure::InvalidArtifact)? as i8,
        )),
        "push_i16" => Some(i64::from(i16::from_le_bytes(
            bytes
                .get(1..3)
                .ok_or(CompileFailure::InvalidArtifact)?
                .try_into()
                .map_err(|_| CompileFailure::InvalidArtifact)?,
        ))),
        "push_i32" => Some(i64::from(i32::from_le_bytes(
            bytes
                .get(1..5)
                .ok_or(CompileFailure::InvalidArtifact)?
                .try_into()
                .map_err(|_| CompileFailure::InvalidArtifact)?,
        ))),
        _ => None,
    })
}

fn argument_index(name: &str, bytes: &[u8]) -> Result<Option<usize>, CompileFailure> {
    Ok(match name {
        "get_arg0" => Some(0),
        "get_arg1" => Some(1),
        "get_arg2" => Some(2),
        "get_arg3" => Some(3),
        "get_arg" => Some(usize::from(u16::from_le_bytes(
            bytes
                .get(1..3)
                .ok_or(CompileFailure::InvalidArtifact)?
                .try_into()
                .map_err(|_| CompileFailure::InvalidArtifact)?,
        ))),
        _ => None,
    })
}

/// Explicit argument count of a call opcode, or `None` for other opcodes.
fn call_argc(name: &str, bytes: &[u8]) -> Result<Option<(usize, bool)>, CompileFailure> {
    let u16_operand = || -> Result<usize, CompileFailure> {
        Ok(usize::from(u16::from_le_bytes(
            bytes
                .get(1..3)
                .ok_or(CompileFailure::InvalidArtifact)?
                .try_into()
                .map_err(|_| CompileFailure::InvalidArtifact)?,
        )))
    };
    Ok(match name {
        "call0" => Some((0, false)),
        "call1" => Some((1, false)),
        "call2" => Some((2, false)),
        "call3" => Some((3, false)),
        "call" => Some((u16_operand()?, false)),
        "tail_call" => Some((u16_operand()?, true)),
        _ => None,
    })
}

fn emit_retry(builder: &mut FunctionBuilder<'_>) {
    let status = builder.ins().iconst(types::I32, NATIVE_CALL_RETRY);
    builder.ins().return_(&[status]);
}

/// Continues in a fresh block when `failed` is non-zero; otherwise retries.
fn emit_fail_if(builder: &mut FunctionBuilder<'_>, failed: Value) {
    let fail = builder.create_block();
    let ok = builder.create_block();
    builder.ins().brif(failed, fail, &[], ok, &[]);
    builder.switch_to_block(fail);
    emit_retry(builder);
    builder.switch_to_block(ok);
}

fn jump_args(stack: &[Slot]) -> Vec<Value> {
    stack.iter().filter_map(|slot| slot.value()).collect()
}

/// Records the operand-stack kinds of a forward edge's target, creating its
/// block parameters on first use and rejecting inconsistent joins.
fn bind_edge(
    builder: &mut FunctionBuilder<'_>,
    blocks: &mut BTreeMap<u32, BlockState>,
    current_pc: u32,
    target_pc: u32,
    stack: &[Slot],
) -> Result<(Block, Vec<Value>), CompileFailure> {
    // Only forward edges: every predecessor is lowered before its target,
    // so each target's stack kinds are known when it is reached.
    if target_pc <= current_pc {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    let state = blocks
        .get_mut(&target_pc)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let kinds = stack.iter().map(|slot| slot.kind()).collect::<Vec<_>>();
    match &state.kinds {
        Some(existing) if *existing != kinds => return Err(CompileFailure::UnsupportedOpcode),
        Some(_) => {}
        None => {
            for kind in &kinds {
                match kind {
                    Kind::Int32 => {
                        builder.append_block_param(state.block, types::I32);
                    }
                    Kind::Float64 => {
                        builder.append_block_param(state.block, types::F64);
                    }
                    Kind::Bool => {
                        builder.append_block_param(state.block, types::I8);
                    }
                    Kind::SelfFunction => {}
                }
            }
            state.kinds = Some(kinds);
        }
    }
    Ok((state.block, jump_args(stack)))
}

fn build(
    pointer_type: types::Type,
    call_conv: cranelift_codegen::isa::CallConv,
    function: &VerifiedFunction,
    signature: &BoundedSpecializationSignature,
) -> Result<(NativeCallPlan, Function), CompileFailure> {
    let snapshot = function.snapshot();
    if signature.function().id != snapshot.function_id()
        || signature.function().generation != snapshot.generation()
        || signature.arity() != usize::from(snapshot.arg_count())
        || scalar_type(signature.result()).is_none()
        || signature
            .arguments()
            .iter()
            .any(|argument| scalar_type(*argument).is_none())
        || !snapshot.exception_map().is_empty()
        || snapshot.closure_count() != 0
        || function.instructions().len() > MAX_NATIVE_CALL_INSTRUCTIONS
    {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    let arity = signature.arity();
    let parameters = signature.arguments();
    let result_representation = signature.result();
    let result_type =
        scalar_type(result_representation).ok_or(CompileFailure::UnsupportedOpcode)?;
    let value_slots = u32::from(snapshot.arg_count())
        .saturating_add(u32::from(snapshot.local_count()))
        .saturating_add(u32::from(snapshot.stack_size()))
        .saturating_add(4);
    let frame_charge = value_slots
        .saturating_mul(16)
        .saturating_add(interpreter_frame_bytes());

    let mut clif = Function::with_name_signature(
        Default::default(),
        entry_signature(pointer_type, call_conv, parameters)?,
    );
    let mut context = FunctionBuilderContext::new();
    let mut atom = None;
    let mut self_calls = 0usize;
    {
        let mut builder = FunctionBuilder::new(&mut clif, &mut context);
        let self_signature =
            builder.import_signature(entry_signature(pointer_type, call_conv, parameters)?);
        let self_name = builder
            .func
            .declare_imported_user_function(UserExternalName::new(NATIVE_SELF_NAMESPACE, 0));
        let self_ref = builder.import_function(ExtFuncData {
            name: ExternalName::user(self_name),
            signature: self_signature,
            colocated: true,
        });
        let mut poll_signature = Signature::new(call_conv);
        poll_signature.params.push(AbiParam::new(pointer_type));
        poll_signature.returns.push(AbiParam::new(types::I32));
        let poll_signature = builder.import_signature(poll_signature);

        let cfg = function.control_flow_graph();
        let mut blocks = cfg
            .blocks()
            .iter()
            .map(|block| {
                (
                    block.start_pc(),
                    BlockState {
                        block: builder.create_block(),
                        kinds: None,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        let prologue = builder.create_block();
        builder.append_block_params_for_function_params(prologue);
        builder.switch_to_block(prologue);
        let params = builder.block_params(prologue).to_vec();
        let nc = params[0];
        let out = params[1];
        let incoming_vsp = params[2];
        let arguments = params[3..].to_vec();

        // Interpreter-equivalent stack check: charge a conservative
        // interpreter frame per level against the caller's stack limit.
        let limit = builder
            .ins()
            .load(pointer_type, MemFlags::trusted(), nc, OFFSET_STACK_LIMIT);
        let charged_limit = builder.ins().iadd_imm(limit, i64::from(frame_charge));
        let exhausted = builder
            .ins()
            .icmp(IntCC::UnsignedLessThan, incoming_vsp, charged_limit);
        let stack_fail = builder.create_block();
        let stack_ok = builder.create_block();
        builder
            .ins()
            .brif(exhausted, stack_fail, &[], stack_ok, &[]);
        builder.switch_to_block(stack_fail);
        let flags = builder
            .ins()
            .load(types::I32, MemFlags::trusted(), nc, OFFSET_FLAGS);
        let flags = builder
            .ins()
            .bor_imm(flags, i64::from(FLAG_STACK_EXHAUSTED));
        builder
            .ins()
            .store(MemFlags::trusted(), flags, nc, OFFSET_FLAGS);
        emit_retry(&mut builder);
        builder.switch_to_block(stack_ok);
        let vsp = builder
            .ins()
            .iadd_imm(incoming_vsp, -i64::from(frame_charge));

        // Interrupt accounting mirrors JS_CallInternal's per-call poll.
        let budget = builder
            .ins()
            .load(types::I32, MemFlags::trusted(), nc, OFFSET_BUDGET);
        let budget = builder.ins().iadd_imm(budget, -1);
        builder
            .ins()
            .store(MemFlags::trusted(), budget, nc, OFFSET_BUDGET);
        let due = builder
            .ins()
            .icmp_imm(IntCC::SignedLessThanOrEqual, budget, 0);
        let poll = builder.create_block();
        let body = blocks.get(&0).ok_or(CompileFailure::InvalidArtifact)?.block;
        builder.ins().brif(due, poll, &[], body, &[]);
        builder.switch_to_block(poll);
        let poll_target = builder.ins().iconst(
            pointer_type,
            JS_JitNativeCallPoll as *const () as usize as i64,
        );
        let call = super::emit_external_call(
            &mut builder,
            poll_signature,
            poll_target,
            &[nc],
            pointer_type,
            None,
            None,
        );
        let interrupted = builder.inst_results(call)[0];
        let interrupted = builder.ins().icmp_imm(IntCC::NotEqual, interrupted, 0);
        emit_fail_if(&mut builder, interrupted);
        builder.ins().jump(body, &[]);
        blocks
            .get_mut(&0)
            .ok_or(CompileFailure::InvalidArtifact)?
            .kinds = Some(Vec::new());

        for block in cfg.blocks() {
            let state = &blocks[&block.start_pc()];
            let Some(kinds) = state.kinds.clone() else {
                // Unreachable from the entry through forward edges.
                if block.start_pc() == 0 {
                    return Err(CompileFailure::InvalidArtifact);
                }
                continue;
            };
            let machine_block = state.block;
            builder.switch_to_block(machine_block);
            let block_params = builder.block_params(machine_block).to_vec();
            let mut next_param = block_params.into_iter();
            let mut stack = Vec::with_capacity(kinds.len());
            for kind in kinds {
                stack.push(match kind {
                    Kind::Int32 => {
                        Slot::Int32(next_param.next().ok_or(CompileFailure::InvalidArtifact)?)
                    }
                    Kind::Float64 => {
                        Slot::Float64(next_param.next().ok_or(CompileFailure::InvalidArtifact)?)
                    }
                    Kind::Bool => {
                        Slot::Bool(next_param.next().ok_or(CompileFailure::InvalidArtifact)?)
                    }
                    Kind::SelfFunction => Slot::SelfFunction,
                });
            }
            let mut terminated = false;
            for instruction in &function.instructions()[block.instruction_range()] {
                if terminated {
                    return Err(CompileFailure::InvalidArtifact);
                }
                let name = instruction.opcode().name();
                let bytes = instruction.bytes();
                if let Some(value) = int_constant(name, bytes)? {
                    stack.push(Slot::Int32(builder.ins().iconst(types::I32, value)));
                    continue;
                }
                if let Some(index) = argument_index(name, bytes)? {
                    let value = *arguments
                        .get(index)
                        .ok_or(CompileFailure::UnsupportedOpcode)?;
                    stack.push(Slot::from_representation(parameters[index], value)?);
                    continue;
                }
                if let Some((argc, tail)) = call_argc(name, bytes)? {
                    if argc != arity {
                        return Err(CompileFailure::UnsupportedOpcode);
                    }
                    let base = stack
                        .len()
                        .checked_sub(argc + 1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    if !matches!(stack[base], Slot::SelfFunction) {
                        return Err(CompileFailure::UnsupportedOpcode);
                    }
                    let mut call_arguments = Vec::with_capacity(argc + 3);
                    let result_slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        8,
                        3,
                    ));
                    let result_address = builder.ins().stack_addr(pointer_type, result_slot, 0);
                    call_arguments.push(nc);
                    call_arguments.push(result_address);
                    call_arguments.push(vsp);
                    let passed = stack.split_off(base + 1);
                    for (slot, parameter) in passed.into_iter().zip(parameters) {
                        let value = number_as(&mut builder, slot, *parameter)?;
                        call_arguments.push(value);
                    }
                    stack.truncate(base);
                    let call = builder.ins().call(self_ref, &call_arguments);
                    let status = builder.inst_results(call)[0];
                    let failed = builder
                        .ins()
                        .icmp_imm(IntCC::NotEqual, status, NATIVE_CALL_OK);
                    let propagate = builder.create_block();
                    let resume = builder.create_block();
                    builder.ins().brif(failed, propagate, &[], resume, &[]);
                    builder.switch_to_block(propagate);
                    builder.ins().return_(&[status]);
                    builder.switch_to_block(resume);
                    let result =
                        builder
                            .ins()
                            .load(result_type, MemFlags::trusted(), result_address, 0);
                    self_calls += 1;
                    if tail {
                        if !stack.is_empty() {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        builder.ins().store(MemFlags::trusted(), result, out, 0);
                        let ok = builder.ins().iconst(types::I32, NATIVE_CALL_OK);
                        builder.ins().return_(&[ok]);
                        terminated = true;
                    } else {
                        stack.push(Slot::from_representation(result_representation, result)?);
                    }
                    continue;
                }
                match name {
                    "nop" => {}
                    "push_const" | "push_const8" => {
                        let index = if name == "push_const8" {
                            u32::from(*bytes.get(1).ok_or(CompileFailure::InvalidArtifact)?)
                        } else {
                            u32::from_le_bytes(
                                bytes
                                    .get(1..5)
                                    .ok_or(CompileFailure::InvalidArtifact)?
                                    .try_into()
                                    .map_err(|_| CompileFailure::InvalidArtifact)?,
                            )
                        };
                        let constant = snapshot
                            .constants()
                            .iter()
                            .find(|constant| constant.index() == index)
                            .ok_or(CompileFailure::UnsupportedOpcode)?;
                        stack.push(match constant.tag() {
                            rquickjs_core::qjs::JS_TAG_INT => Slot::Int32(
                                builder
                                    .ins()
                                    .iconst(types::I32, i64::from(constant.payload() as i32)),
                            ),
                            rquickjs_core::qjs::JS_TAG_FLOAT64 => Slot::Float64(
                                builder.ins().f64const(f64::from_bits(constant.payload())),
                            ),
                            _ => return Err(CompileFailure::UnsupportedOpcode),
                        });
                    }
                    "get_var" => {
                        let operand = u32::from_le_bytes(
                            bytes
                                .get(1..5)
                                .ok_or(CompileFailure::InvalidArtifact)?
                                .try_into()
                                .map_err(|_| CompileFailure::InvalidArtifact)?,
                        );
                        if *atom.get_or_insert(operand) != operand {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        stack.push(Slot::SelfFunction);
                    }
                    "drop" => match stack.pop() {
                        Some(Slot::Int32(_) | Slot::Float64(_) | Slot::Bool(_)) => {}
                        _ => return Err(CompileFailure::UnsupportedOpcode),
                    },
                    "add" | "sub" | "mul" | "div" => {
                        let (Some(rhs), Some(lhs)) = (stack.pop(), stack.pop()) else {
                            return Err(CompileFailure::InvalidArtifact);
                        };
                        // Every Float64 operation is exact IEEE-754 double
                        // arithmetic, as in the interpreter; division always
                        // produces a JavaScript number, so it uses doubles.
                        let floats = if name == "div" {
                            Some((
                                number_as(&mut builder, lhs, FeedbackRepresentation::Float64)?,
                                number_as(&mut builder, rhs, FeedbackRepresentation::Float64)?,
                            ))
                        } else {
                            float_operands(&mut builder, lhs, rhs)?
                        };
                        if let Some((lhs, rhs)) = floats {
                            let value = match name {
                                "add" => builder.ins().fadd(lhs, rhs),
                                "sub" => builder.ins().fsub(lhs, rhs),
                                "mul" => builder.ins().fmul(lhs, rhs),
                                _ => builder.ins().fdiv(lhs, rhs),
                            };
                            stack.push(Slot::Float64(value));
                            continue;
                        }
                        let (Slot::Int32(lhs), Slot::Int32(rhs)) = (lhs, rhs) else {
                            return Err(CompileFailure::UnsupportedOpcode);
                        };
                        let (result, overflow) = match name {
                            "add" => builder.ins().sadd_overflow(lhs, rhs),
                            "sub" => builder.ins().ssub_overflow(lhs, rhs),
                            _ => builder.ins().smul_overflow(lhs, rhs),
                        };
                        emit_fail_if(&mut builder, overflow);
                        if name == "mul" {
                            // `0 * -n` is -0, which Int32 cannot represent.
                            let zero = builder.ins().icmp_imm(IntCC::Equal, result, 0);
                            let signs = builder.ins().bor(lhs, rhs);
                            let negative = builder.ins().icmp_imm(IntCC::SignedLessThan, signs, 0);
                            let negative_zero = builder.ins().band(zero, negative);
                            emit_fail_if(&mut builder, negative_zero);
                        }
                        stack.push(Slot::Int32(result));
                    }
                    "inc" | "dec" => {
                        let value = match stack.pop() {
                            Some(Slot::Int32(value)) => value,
                            Some(Slot::Float64(value)) => {
                                let one = builder.ins().f64const(1.0);
                                let result = if name == "inc" {
                                    builder.ins().fadd(value, one)
                                } else {
                                    builder.ins().fsub(value, one)
                                };
                                stack.push(Slot::Float64(result));
                                continue;
                            }
                            _ => return Err(CompileFailure::UnsupportedOpcode),
                        };
                        let one = builder.ins().iconst(types::I32, 1);
                        let (result, overflow) = if name == "inc" {
                            builder.ins().sadd_overflow(value, one)
                        } else {
                            builder.ins().ssub_overflow(value, one)
                        };
                        emit_fail_if(&mut builder, overflow);
                        stack.push(Slot::Int32(result));
                    }
                    "lt" | "lte" | "gt" | "gte" | "eq" | "neq" | "strict_eq" | "strict_neq" => {
                        let (Some(rhs), Some(lhs)) = (stack.pop(), stack.pop()) else {
                            return Err(CompileFailure::InvalidArtifact);
                        };
                        if let Some((lhs, rhs)) = float_operands(&mut builder, lhs, rhs)? {
                            // Ordered comparisons are false for NaN; `!=`
                            // is unordered-or-not-equal, true for NaN.
                            let condition = match name {
                                "lt" => FloatCC::LessThan,
                                "lte" => FloatCC::LessThanOrEqual,
                                "gt" => FloatCC::GreaterThan,
                                "gte" => FloatCC::GreaterThanOrEqual,
                                "eq" | "strict_eq" => FloatCC::Equal,
                                _ => FloatCC::NotEqual,
                            };
                            stack.push(Slot::Bool(builder.ins().fcmp(condition, lhs, rhs)));
                            continue;
                        }
                        let (Slot::Int32(lhs), Slot::Int32(rhs)) = (lhs, rhs) else {
                            return Err(CompileFailure::UnsupportedOpcode);
                        };
                        let condition = match name {
                            "lt" => IntCC::SignedLessThan,
                            "lte" => IntCC::SignedLessThanOrEqual,
                            "gt" => IntCC::SignedGreaterThan,
                            "gte" => IntCC::SignedGreaterThanOrEqual,
                            "eq" | "strict_eq" => IntCC::Equal,
                            _ => IntCC::NotEqual,
                        };
                        stack.push(Slot::Bool(builder.ins().icmp(condition, lhs, rhs)));
                    }
                    "lnot" => {
                        let truth = match stack.pop() {
                            Some(Slot::Bool(value)) => {
                                builder.ins().icmp_imm(IntCC::NotEqual, value, 0)
                            }
                            Some(Slot::Int32(value)) => {
                                builder.ins().icmp_imm(IntCC::NotEqual, value, 0)
                            }
                            _ => return Err(CompileFailure::UnsupportedOpcode),
                        };
                        let negated = builder.ins().bxor_imm(truth, 1);
                        stack.push(Slot::Bool(negated));
                    }
                    "if_false" | "if_true" | "if_false8" | "if_true8" => {
                        let condition = match stack.pop() {
                            Some(Slot::Bool(value) | Slot::Int32(value)) => value,
                            _ => return Err(CompileFailure::UnsupportedOpcode),
                        };
                        let target_pc = u32::try_from(
                            instruction
                                .branch_target()
                                .ok_or(CompileFailure::InvalidArtifact)?,
                        )
                        .map_err(|_| CompileFailure::InvalidArtifact)?;
                        let (target, target_args) = bind_edge(
                            &mut builder,
                            &mut blocks,
                            block.start_pc(),
                            target_pc,
                            &stack,
                        )?;
                        let (fallthrough, fallthrough_args) = bind_edge(
                            &mut builder,
                            &mut blocks,
                            block.start_pc(),
                            block.end_pc(),
                            &stack,
                        )?;
                        if name.starts_with("if_false") {
                            builder.ins().brif(
                                condition,
                                fallthrough,
                                &fallthrough_args,
                                target,
                                &target_args,
                            );
                        } else {
                            builder.ins().brif(
                                condition,
                                target,
                                &target_args,
                                fallthrough,
                                &fallthrough_args,
                            );
                        }
                        terminated = true;
                    }
                    "goto" | "goto8" | "goto16" => {
                        let target_pc = u32::try_from(
                            instruction
                                .branch_target()
                                .ok_or(CompileFailure::InvalidArtifact)?,
                        )
                        .map_err(|_| CompileFailure::InvalidArtifact)?;
                        let (target, target_args) = bind_edge(
                            &mut builder,
                            &mut blocks,
                            block.start_pc(),
                            target_pc,
                            &stack,
                        )?;
                        builder.ins().jump(target, &target_args);
                        terminated = true;
                    }
                    "return" => {
                        let result = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        let result = number_as(&mut builder, result, result_representation)?;
                        if !stack.is_empty() {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        builder.ins().store(MemFlags::trusted(), result, out, 0);
                        let ok = builder.ins().iconst(types::I32, NATIVE_CALL_OK);
                        builder.ins().return_(&[ok]);
                        terminated = true;
                    }
                    _ => return Err(CompileFailure::UnsupportedOpcode),
                }
            }
            if !terminated {
                if block.successors().len() != 1 || block.successors()[0] != block.end_pc() {
                    return Err(CompileFailure::UnsupportedOpcode);
                }
                let (target, target_args) = bind_edge(
                    &mut builder,
                    &mut blocks,
                    block.start_pc(),
                    block.end_pc(),
                    &stack,
                )?;
                builder.ins().jump(target, &target_args);
            }
        }
        builder.seal_all_blocks();
        builder.finalize();
    }
    let atom = atom.ok_or(CompileFailure::UnsupportedOpcode)?;
    if self_calls == 0 {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    Ok((
        NativeCallPlan {
            atom,
            arguments: parameters.into(),
            result: result_representation,
            frame_charge,
        },
        clif,
    ))
}
