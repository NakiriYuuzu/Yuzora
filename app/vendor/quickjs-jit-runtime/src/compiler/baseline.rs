//! Cranelift lowering and W^X publication for Tier 1 pure frame operations.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    mem,
    sync::Arc,
};

#[cfg(feature = "test-support")]
use std::sync::Mutex;

use cranelift_codegen::{
    binemit::Reloc as CraneliftReloc,
    control::ControlPlane,
    ir::{
        condcodes::{FloatCC, IntCC},
        types, AbiParam, ArgumentPurpose, Block, Function, InstBuilder, MemFlags, Signature,
        SourceLoc, StackSlot, StackSlotData, StackSlotKind, TrapCode, Value,
    },
    isa::{unwind::UnwindInfo as CraneliftUnwindInfo, OwnedTargetIsa, TargetIsa},
    settings::{self, Configurable},
    Context, FinalizedRelocTarget,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use rquickjs_core::qjs;
use target_lexicon::{Endianness, Triple};

use crate::{
    bytecode::VerifiedFunction,
    code_cache::{
        CodeAllocation, CompiledArtifact, FrameState as ArtifactFrameState, FrameStateLocationKind,
        Relocation, RelocationKind, RelocationTarget, StackMap, UnwindKind, UnwindMetadata,
    },
    ir::{
        BaselineIr, BinaryOp, FrameSlot, FrameStateId, FrameStateKind, GenericOp, IrOp, IteratorOp,
        PollKind, StackOp, TaggedValue, UnaryOp, VarRefMode, MAX_HELPER_SCRATCH_SLOTS,
    },
    platform::{CodeAllocator, CodeMemoryError, ExecutableCode},
    runtime::CompileRequest,
};

use super::{
    emit_external_call, emit_msan_unpoison,
    helpers::{generated_signatures, FrameLayout},
    CompileControl, CompileFailure, Compiler,
};

#[derive(Clone, Copy)]
struct Pair {
    payload: Value,
    tag: Value,
}

#[derive(Clone, Copy)]
struct PairVars {
    payload: Variable,
    tag: Variable,
}

#[derive(Clone, Copy)]
struct PollLocation {
    bytecode_pc: u32,
    source_location: SourceLoc,
}

// Keep interrupt handling bounded without crossing the C helper boundary on
// every short loop trip.  Tier 1 materializes the complete interpreter frame
// before a poll, so a 64-backedge cadence made that safepoint work dominate
// otherwise native numeric kernels.  1024 is still a fixed, deterministic
// bound (and entry polls remain unconditional).
const LOOP_POLL_INTERVAL: i64 = 1024;

macro_rules! element_guard {
    ($builder:expr, $condition:expr, $fallback:expr $(,)?) => {{
        let element_condition = $condition;
        emit_element_guard($builder, element_condition, $fallback);
    }};
}

struct HelperLowering<'a> {
    ir: &'a BaselineIr,
    frame: Value,
    runtime_api: Value,
    sret: Value,
    arg_buf: Value,
    var_buf: Value,
    stack_base: Value,
    arguments: &'a [PairVars],
    locals: &'a [PairVars],
    stack: &'a [PairVars],
    signatures: &'a [cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: FrameLayout,
    /// The function creates closures (`fclosure`), so attached var refs may
    /// alias its argument and local slots. Any helper that can run JavaScript
    /// may then write those slots through the closure, so the native copies
    /// are reloaded from the interpreter frame after every helper returns.
    reload_captured_slots: bool,
    /// Landing pad of the innermost try region around the instruction being
    /// lowered; `None` exits to the interpreter with the pending exception.
    exception_target: std::cell::Cell<Option<Block>>,
    element_layout: crate::abi::ElementLayout,
    object_fast: Option<ObjectFastPaths>,
}

/// Addresses of the versioned object/array leaves (`JSJitObjectAPI`). Every
/// leaf either completes the interpreter's exact effect or misses without an
/// observable effect, never throws and never reenters the VM, so generated
/// code keeps the generic helper sequence as the fallback of every hit test.
#[derive(Clone, Copy, Debug)]
struct ObjectFastPaths {
    literal: usize,
    retain_shape: usize,
    array_method: usize,
    array_push: usize,
    array_push_method: usize,
    max_literal_fields: usize,
}

impl ObjectFastPaths {
    fn from_api(api: &qjs::JSJitObjectAPI) -> Option<Self> {
        Some(Self {
            literal: api.literal? as usize,
            retain_shape: api.retain_shape? as usize,
            array_method: api.array_method? as usize,
            array_push: api.array_push? as usize,
            array_push_method: api.array_push_method? as usize,
            max_literal_fields: usize::try_from(api.max_literal_fields).ok()?,
        })
    }
}

/// A straight-line instruction range with a leaf fast path. The hit path
/// jumps to `join`; the unchanged instructions `start..=end` are lowered in
/// the fallback block, which then jumps to `join` as well.
#[derive(Clone, Copy, Debug)]
struct FusedRegion {
    end: usize,
    join: Block,
    /// Stack slot of a literal built on the fallback path whose final shape
    /// is retained so later executions hit the literal leaf.
    retain_literal: Option<usize>,
}

fn literal_operand(op: &IrOp, argument_count: usize, local_count: usize) -> Option<LiteralValue> {
    match *op {
        IrOp::Push(constant) if constant.tag >= 0 => Some(LiteralValue::Constant(constant)),
        IrOp::GetArgument(index) if usize::from(index) < argument_count => {
            Some(LiteralValue::Argument(index))
        }
        IrOp::GetLocal(index) | IrOp::GetLocalChecked(index)
            if usize::from(index) < local_count =>
        {
            Some(LiteralValue::Local(index))
        }
        _ => None,
    }
}

fn literal_operand_pair(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    value: LiteralValue,
) -> Result<Pair, CompileFailure> {
    Ok(match value {
        LiteralValue::Constant(constant) => constant_pair(builder, constant),
        LiteralValue::Argument(argument) => use_pair(
            builder,
            *helpers
                .arguments
                .get(usize::from(argument))
                .ok_or(CompileFailure::InvalidArtifact)?,
        ),
        LiteralValue::Local(local) => use_pair(
            builder,
            *helpers
                .locals
                .get(usize::from(local))
                .ok_or(CompileFailure::InvalidArtifact)?,
        ),
    })
}

/// `receiver.method(value)` where the receiver and the single argument are
/// side-effect-free reads: `(receiver, atom, value, end)`.
fn array_push_statement_pattern(
    instructions: &[crate::ir::IrInstruction],
    start: usize,
    argument_count: usize,
    local_count: usize,
) -> Option<(LiteralValue, u32, LiteralValue, usize)> {
    let receiver = match literal_operand(&instructions.get(start)?.op, argument_count, local_count)?
    {
        LiteralValue::Constant(_) => return None,
        receiver => receiver,
    };
    let IrOp::GetPropertyKeep(atom) = instructions.get(start + 1)?.op else {
        return None;
    };
    let value = literal_operand(
        &instructions.get(start + 2)?.op,
        argument_count,
        local_count,
    )?;
    matches!(
        instructions.get(start + 3)?.op,
        IrOp::Call {
            argc: 1,
            has_this: true
        }
    )
    .then_some((receiver, atom, value, start + 3))
}

fn emit_fused_fast_path(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    fast: ObjectFastPaths,
    instructions: &[crate::ir::IrInstruction],
    start: usize,
    depth: usize,
) -> Result<Option<FusedRegion>, CompileFailure> {
    let argument_count = helpers.arguments.len();
    let local_count = helpers.locals.len();
    if let Some((atoms, values, end)) = object_literal_pattern(
        instructions,
        start,
        argument_count,
        local_count,
        fast.max_literal_fields,
    ) {
        if depth + usize::from(!values.is_empty()) >= helpers.stack.len() {
            return Ok(None);
        }
        let join = emit_object_literal_fast_path(builder, helpers, fast, &atoms, &values, depth)?;
        return Ok(Some(FusedRegion {
            end,
            join,
            retain_literal: Some(depth),
        }));
    }
    if let Some((receiver, atom, value, end)) =
        array_push_statement_pattern(instructions, start, argument_count, local_count)
    {
        if depth + 2 >= helpers.stack.len() {
            return Ok(None);
        }
        let join = emit_array_push_statement_fast_path(
            builder, helpers, fast, receiver, atom, value, depth,
        )?;
        return Ok(Some(FusedRegion {
            end,
            join,
            retain_literal: None,
        }));
    }
    Ok(None)
}

/// Whole-statement `array.push(value)`: one leaf performs the exact
/// `get_field2` lookup and the built-in push without materializing the
/// receiver, method or argument references (no DUP/FREE helpers).
#[allow(clippy::too_many_arguments)]
fn emit_array_push_statement_fast_path(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    fast: ObjectFastPaths,
    receiver: LiteralValue,
    atom: u32,
    value: LiteralValue,
    depth: usize,
) -> Result<Block, CompileFailure> {
    let pointer_type = helpers.pointer_type;
    let receiver = literal_operand_pair(builder, helpers, receiver)?;
    let value = literal_operand_pair(builder, helpers, value)?;
    let call = builder.create_block();
    let hit = builder.create_block();
    let fallback = builder.create_block();
    let join = builder.create_block();
    let is_object = tag_is(builder, receiver.tag, qjs::JS_TAG_OBJECT);
    let class_check = builder.create_block();
    builder
        .ins()
        .brif(is_object, class_check, &[], fallback, &[]);
    builder.seal_block(class_check);
    builder.switch_to_block(class_check);
    let class = builder.ins().load(
        types::I16,
        MemFlags::trusted(),
        receiver.payload,
        helpers.element_layout.object_class_id_offset,
    );
    let class = builder.ins().uextend(types::I64, class);
    let is_array =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, class, helpers.element_layout.array_class_id);
    builder.ins().brif(is_array, call, &[], fallback, &[]);
    builder.seal_block(call);
    builder.switch_to_block(call);
    let value_size =
        i32::try_from(mem::size_of::<qjs::JSValue>()).map_err(|_| CompileFailure::ResourceLimit)?;
    let scratch = object_scratch(builder, 3);
    for (index, pair) in [receiver, value].into_iter().enumerate() {
        let offset =
            value_size * i32::try_from(index).map_err(|_| CompileFailure::ResourceLimit)?;
        builder.ins().stack_store(pair.payload, scratch, offset);
        builder
            .ins()
            .stack_store(pair.tag, scratch, offset + helpers.layout.value_tag);
    }
    let ctx = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        helpers.frame,
        helpers.layout.ctx,
    );
    let receiver_address = builder.ins().stack_addr(pointer_type, scratch, 0);
    let value_address = builder.ins().stack_addr(pointer_type, scratch, value_size);
    let out_address = builder
        .ins()
        .stack_addr(pointer_type, scratch, 2 * value_size);
    let atom = builder.ins().iconst(types::I32, i64::from(atom));
    let status = emit_object_leaf_call(
        builder,
        pointer_type,
        fast.array_push_method,
        &[ctx, receiver_address, atom, value_address, out_address],
        &[scratch],
    );
    let succeeded = builder.ins().icmp_imm(
        IntCC::Equal,
        status,
        i64::from(qjs::JSJitObjectStatus_JS_JIT_OBJECT_OK),
    );
    builder.ins().brif(succeeded, hit, &[], fallback, &[]);
    builder.seal_block(hit);
    builder.seal_block(fallback);
    builder.switch_to_block(hit);
    let length = Pair {
        payload: builder
            .ins()
            .stack_load(types::I64, scratch, 2 * value_size),
        tag: builder.ins().stack_load(
            types::I64,
            scratch,
            2 * value_size + helpers.layout.value_tag,
        ),
    };
    define_pair(builder, helpers.stack[depth], length);
    let undefined = constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64));
    define_pair(builder, helpers.stack[depth + 1], undefined);
    define_pair(builder, helpers.stack[depth + 2], undefined);
    builder.ins().jump(join, &[]);
    builder.switch_to_block(fallback);
    Ok(join)
}

/// After the generic literal sequence, retain the object's final shape.
fn emit_retain_literal_shape(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    fast: ObjectFastPaths,
    object_index: usize,
) -> Result<(), CompileFailure> {
    let pointer_type = helpers.pointer_type;
    let object = use_pair(builder, helpers.stack[object_index]);
    let scratch = object_scratch(builder, 1);
    builder.ins().stack_store(object.payload, scratch, 0);
    builder
        .ins()
        .stack_store(object.tag, scratch, helpers.layout.value_tag);
    let ctx = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        helpers.frame,
        helpers.layout.ctx,
    );
    let object_address = builder.ins().stack_addr(pointer_type, scratch, 0);
    emit_object_leaf_call(
        builder,
        pointer_type,
        fast.retain_shape,
        &[ctx, object_address],
        &[scratch],
    );
    Ok(())
}

/// One side-effect-free value of a fused object literal. Each operand is read
/// in place and duplicated by the leaf, exactly like the bytecode push would.
#[derive(Clone, Copy, Debug)]
enum LiteralValue {
    Constant(TaggedValue),
    Argument(u16),
    Local(u16),
}

/// `object` followed by `(pure push, define_field)*`, all inside one block.
/// Returns the atoms/values and the index of the last fused instruction.
fn object_literal_pattern(
    instructions: &[crate::ir::IrInstruction],
    start: usize,
    argument_count: usize,
    local_count: usize,
    max_fields: usize,
) -> Option<(Vec<u32>, Vec<LiteralValue>, usize)> {
    if !matches!(instructions.get(start)?.op, IrOp::NewObject) {
        return None;
    }
    let mut atoms = Vec::new();
    let mut values = Vec::new();
    let mut end = start;
    while atoms.len() < max_fields {
        let (Some(value), Some(define)) = (instructions.get(end + 1), instructions.get(end + 2))
        else {
            break;
        };
        let IrOp::DefineProperty(atom) = define.op else {
            break;
        };
        let Some(value) = literal_operand(&value.op, argument_count, local_count) else {
            break;
        };
        if atom == 0 || atoms.contains(&atom) {
            break;
        }
        atoms.push(atom);
        values.push(value);
        end += 2;
    }
    // QuickJS keeps only the final hashed shape of a literal alive (the
    // transition shapes are mutated in place), so a prefix of a longer
    // literal would miss on every execution. Fuse complete literals only.
    literal_is_complete(instructions, end).then_some((atoms, values, end))
}

/// Bytecode stack effect `(pops, pushes)` of the Tier 1 operations that may
/// follow a literal. `None` stops the completeness scan conservatively.
fn literal_scan_stack_effect(op: &IrOp) -> Option<(usize, usize)> {
    Some(match *op {
        IrOp::Nop => (0, 0),
        IrOp::Push(_)
        | IrOp::ResolveConstant(_)
        | IrOp::ResolveAtom(_)
        | IrOp::GetGlobal(_)
        | IrOp::NewObject
        | IrOp::GetArgument(_)
        | IrOp::GetLocal(_)
        | IrOp::GetLocalChecked(_) => (0, 1),
        IrOp::GetLocalPair => (0, 2),
        IrOp::NewArrayFrom(count) => (usize::from(count), 1),
        IrOp::GetProperty(_) | IrOp::ToPropertyKey | IrOp::Unary(_) => (1, 1),
        IrOp::GetPropertyKeep(_) => (1, 2),
        IrOp::SetProperty(_) => (2, 0),
        IrOp::DefineProperty(_) | IrOp::GetElement | IrOp::Binary(_) => (2, 1),
        IrOp::SetElement => (3, 0),
        IrOp::DefineElement => (3, 1),
        IrOp::Call { argc, has_this } => (usize::from(argc) + 1 + usize::from(has_this), 1),
        IrOp::PutArgument { keep, .. } | IrOp::PutLocal { keep, .. } => (1, usize::from(keep)),
        IrOp::Drop => (1, 0),
        _ => return None,
    })
}

/// True when the object produced by the literal ending at `end` is consumed
/// in this block before another definition could target it.
fn literal_is_complete(instructions: &[crate::ir::IrInstruction], end: usize) -> bool {
    // Depths are relative to the literal object's slot, which sits at 0.
    let mut depth = 1_usize;
    for instruction in instructions.iter().skip(end + 1).take(64) {
        match instruction.op {
            IrOp::DefineProperty(_) if depth == 2 => return false,
            IrOp::DefineElement if depth == 3 => return false,
            _ => {}
        }
        let Some((pops, pushes)) = literal_scan_stack_effect(&instruction.op) else {
            return true;
        };
        if pops >= depth {
            return true;
        }
        depth = depth - pops + pushes;
    }
    false
}

fn object_scratch(builder: &mut FunctionBuilder<'_>, values: usize) -> StackSlot {
    let bytes = values.max(1) * mem::size_of::<qjs::JSValue>();
    builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        u32::try_from(bytes).expect("bounded scratch"),
        3,
    ))
}

/// Calls an object leaf. `slots` are the stack slots the leaf reads through
/// pointer parameters; they are unpoisoned first because MSan cannot observe
/// Cranelift stores.
fn emit_object_leaf_call(
    builder: &mut FunctionBuilder<'_>,
    pointer_type: cranelift_codegen::ir::Type,
    target: usize,
    params: &[Value],
    slots: &[StackSlot],
) -> Value {
    for slot in slots {
        let size = builder.func.sized_stack_slots[*slot].size as usize;
        let address = builder.ins().stack_addr(pointer_type, *slot, 0);
        emit_msan_unpoison(builder, pointer_type, address, size);
    }
    let mut signature = Signature::new(builder.func.signature.call_conv);
    for param in params {
        signature
            .params
            .push(AbiParam::new(builder.func.dfg.value_type(*param)));
    }
    signature.returns.push(AbiParam::new(types::I32));
    let signature = builder.import_signature(signature);
    let target = builder.ins().iconst(pointer_type, target as i64);
    let call = emit_external_call(builder, signature, target, params, pointer_type, None, None);
    builder.inst_results(call)[0]
}

/// Emit the literal leaf before the generic `object`/`define_field` sequence.
/// On a hit the fused result is installed and control jumps to the returned
/// join block; on a miss the builder is left in the fallback block, where the
/// caller lowers the unchanged instruction sequence and then jumps to join.
#[allow(clippy::too_many_arguments)]
fn emit_object_literal_fast_path(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    fast: ObjectFastPaths,
    atoms: &[u32],
    values: &[LiteralValue],
    depth: usize,
) -> Result<Block, CompileFailure> {
    let pointer_type = helpers.pointer_type;
    let value_slot = object_scratch(builder, values.len());
    let out_slot = object_scratch(builder, 1);
    let atom_slot = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        u32::try_from(atoms.len().max(1) * 4).map_err(|_| CompileFailure::ResourceLimit)?,
        2,
    ));
    for (index, (atom, value)) in atoms.iter().zip(values).enumerate() {
        let pair = literal_operand_pair(builder, helpers, *value)?;
        let offset = i32::try_from(index * mem::size_of::<qjs::JSValue>())
            .map_err(|_| CompileFailure::ResourceLimit)?;
        builder.ins().stack_store(pair.payload, value_slot, offset);
        builder
            .ins()
            .stack_store(pair.tag, value_slot, offset + helpers.layout.value_tag);
        let atom = builder.ins().iconst(types::I32, i64::from(*atom));
        let atom_offset = i32::try_from(index * 4).map_err(|_| CompileFailure::ResourceLimit)?;
        builder.ins().stack_store(atom, atom_slot, atom_offset);
    }
    let ctx = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        helpers.frame,
        helpers.layout.ctx,
    );
    let atoms_address = builder.ins().stack_addr(pointer_type, atom_slot, 0);
    let values_address = builder.ins().stack_addr(pointer_type, value_slot, 0);
    let count = builder.ins().iconst(
        types::I32,
        i64::try_from(atoms.len()).map_err(|_| CompileFailure::ResourceLimit)?,
    );
    let out_address = builder.ins().stack_addr(pointer_type, out_slot, 0);
    let status = emit_object_leaf_call(
        builder,
        pointer_type,
        fast.literal,
        &[ctx, atoms_address, values_address, count, out_address],
        &[atom_slot, value_slot],
    );
    let hit = builder.create_block();
    let fallback = builder.create_block();
    let join = builder.create_block();
    let succeeded = builder.ins().icmp_imm(
        IntCC::Equal,
        status,
        i64::from(qjs::JSJitObjectStatus_JS_JIT_OBJECT_OK),
    );
    builder.ins().brif(succeeded, hit, &[], fallback, &[]);
    builder.seal_block(hit);
    builder.seal_block(fallback);
    builder.switch_to_block(hit);
    let object = Pair {
        payload: builder.ins().stack_load(types::I64, out_slot, 0),
        tag: builder
            .ins()
            .stack_load(types::I64, out_slot, helpers.layout.value_tag),
    };
    define_pair(builder, helpers.stack[depth], object);
    if !values.is_empty() {
        let undefined = constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64));
        define_pair(builder, helpers.stack[depth + 1], undefined);
    }
    builder.ins().jump(join, &[]);
    builder.switch_to_block(fallback);
    Ok(join)
}

impl HelperLowering<'_> {
    /// Refresh the native argument/local copies from the interpreter frame.
    /// Every helper call first publishes all of them (`materialize_frame`),
    /// so right after it returns the frame is the authoritative owner.
    fn reload_captured_slots(&self, builder: &mut FunctionBuilder<'_>) {
        if !self.reload_captured_slots {
            return;
        }
        for (index, pair) in self.arguments.iter().copied().enumerate() {
            reload_pair(builder, pair, self.arg_buf, index, self.layout);
        }
        for (index, pair) in self.locals.iter().copied().enumerate() {
            reload_pair(builder, pair, self.var_buf, index, self.layout);
        }
    }

    fn invoke(
        &self,
        builder: &mut FunctionBuilder<'_>,
        helper_id: qjs::JSJitHelperId,
        state: FrameStateId,
        live_depth: usize,
        exception_depth: usize,
        arguments: &[u32],
    ) -> Result<(), CompileFailure> {
        invoke_frame_helper(
            builder,
            self.ir,
            self.frame,
            self.runtime_api,
            self.sret,
            self.arg_buf,
            self.var_buf,
            self.stack_base,
            self.arguments,
            self.locals,
            self.stack,
            live_depth,
            exception_depth,
            self.signatures,
            helper_id as usize,
            state,
            arguments,
            self.pointer_type,
            self.layout,
            self.exception_target.get(),
        )?;
        self.reload_captured_slots(builder);
        Ok(())
    }

    fn set_depth(
        &self,
        builder: &mut FunctionBuilder<'_>,
        depth: usize,
    ) -> Result<(), CompileFailure> {
        set_visible_stack_depth(builder, self.frame, self.stack_base, depth, self.layout)
    }

    /// Natively lowered arithmetic reinterprets the payload word as Int32 or
    /// Float64 bits, so every operand must carry a numeric tag.  The entry
    /// analysis proves that for argument/local roots and literal pushes, but
    /// values with an unknown domain (globals, elements, constants, property
    /// keys, call results) are only known at run time.  Guard them here and
    /// hand the untouched frame back to the interpreter at this instruction
    /// when the proof fails; the interpreter then applies the exact coercion
    /// (`ToNumeric`, `Symbol.toPrimitive`, BigInt arithmetic, TypeErrors).
    fn deopt_unless_numeric(
        &self,
        builder: &mut FunctionBuilder<'_>,
        operands: &[Pair],
        depth: usize,
        bytecode_pc: u32,
    ) -> Result<(), CompileFailure> {
        let mut condition = None;
        for operand in operands {
            let numeric = emit_numeric_tag(builder, operand.tag);
            condition = Some(match condition {
                Some(previous) => builder.ins().band(previous, numeric),
                None => numeric,
            });
        }
        let Some(condition) = condition else {
            return Ok(());
        };
        let deopt = builder.create_block();
        let continuation = builder.create_block();
        builder.ins().brif(condition, continuation, &[], deopt, &[]);
        builder.seal_block(deopt);
        builder.switch_to_block(deopt);
        builder.set_cold_block(deopt);
        materialize_frame(
            builder,
            self.frame,
            self.arg_buf,
            self.var_buf,
            self.stack_base,
            self.arguments,
            self.locals,
            self.stack,
            depth,
            depth,
            bytecode_pc,
            self.pointer_type,
            self.layout,
        )?;
        let bytecode = builder.ins().load(
            self.pointer_type,
            MemFlags::new(),
            self.frame,
            self.layout.bytecode_start,
        );
        let resume = builder.ins().iadd_imm(bytecode, i64::from(bytecode_pc));
        emit_exit(
            builder,
            self.sret,
            qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
            Some(resume),
            self.pointer_type,
        );
        builder.seal_block(continuation);
        builder.switch_to_block(continuation);
        Ok(())
    }
}

/// Cranelift compiler configured for one explicit target ISA.
#[derive(Clone)]
pub struct BaselineCompiler {
    isa: OwnedTargetIsa,
    host_publishable: bool,
}

#[derive(Clone, Debug)]
struct BaselineDirectCallSite {
    pc: u32,
    call: crate::runtime::CallSpecializationKey,
    entry: usize,
}

#[derive(Clone, Debug)]
struct BaselinePropertySite {
    pc: u32,
    store: bool,
    observations: Box<[crate::runtime::ShapeObservation]>,
}

fn baseline_property_sites(
    function: &VerifiedFunction,
    feedback: &crate::runtime::FeedbackSnapshot,
) -> Vec<BaselinePropertySite> {
    use crate::runtime::{ObservedType, PropertyAttributes, ShapeFeedbackState};
    function
        .instructions()
        .iter()
        .filter_map(|instruction| {
            let store = match instruction.opcode().name() {
                "get_field" => false,
                "put_field" => true,
                _ => return None,
            };
            let site = feedback.property_at(instruction.pc())?;
            let observations = site.observations();
            let safe = site.state() != ShapeFeedbackState::Megamorphic
                && !observations.is_empty()
                && observations.len() <= crate::runtime::POLYMORPHIC_PROPERTY_LIMIT
                && observations.iter().all(|observation| {
                    observation.shape().identity() != 0
                        && observation.shape().generation() != 0
                        && matches!(
                            observation.value(),
                            ObservedType::Int32
                                | ObservedType::Float64
                                | ObservedType::Bool
                                | ObservedType::Null
                                | ObservedType::Undefined
                        )
                        && observation.prototype().identity() == 0
                        && observation.prototype().generation() == 0
                        && !observation
                            .attributes()
                            .contains(PropertyAttributes::ACCESSOR)
                        && (!store
                            || observation
                                .attributes()
                                .contains(PropertyAttributes::WRITABLE))
                });
            safe.then(|| BaselinePropertySite {
                pc: instruction.pc(),
                store,
                observations: observations.to_vec().into_boxed_slice(),
            })
        })
        .collect()
}

pub(crate) fn has_baseline_property_sites(
    function: &VerifiedFunction,
    feedback: &crate::runtime::FeedbackSnapshot,
) -> bool {
    !baseline_property_sites(function, feedback).is_empty()
}

/// Stable, complete identity of the Cranelift target used to produce code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetIdentity {
    triple: String,
    shared_flags: Vec<(String, String)>,
    isa_flags: Vec<(String, String)>,
}

impl TargetIdentity {
    pub fn from_isa(isa: &dyn TargetIsa) -> Self {
        let mut shared_flags = isa
            .flags()
            .iter()
            .map(|value| (value.name.to_owned(), value.value_string()))
            .collect::<Vec<_>>();
        let mut isa_flags = isa
            .isa_flags()
            .into_iter()
            .map(|value| (value.name.to_owned(), value.value_string()))
            .collect::<Vec<_>>();
        shared_flags.sort_unstable();
        isa_flags.sort_unstable();
        Self {
            triple: isa.triple().to_string(),
            shared_flags,
            isa_flags,
        }
    }

    pub fn triple(&self) -> &str {
        &self.triple
    }

    pub fn shared_flags(&self) -> &[(String, String)] {
        &self.shared_flags
    }

    pub fn isa_flags(&self) -> &[(String, String)] {
        &self.isa_flags
    }

    fn hash_fields<'a>(fields: impl IntoIterator<Item = &'a str>) -> u64 {
        let mut hash = 0xcbf29ce484222325_u64;
        for field in fields {
            for byte in (field.len() as u64)
                .to_le_bytes()
                .into_iter()
                .chain(field.bytes())
            {
                hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
            }
        }
        hash
    }

    pub fn triple_fingerprint(&self) -> u64 {
        Self::hash_fields([self.triple.as_str()])
    }

    pub fn codegen_fingerprint(&self) -> u64 {
        Self::hash_fields(
            self.shared_flags
                .iter()
                .flat_map(|(name, value)| ["shared", name.as_str(), value.as_str()])
                .chain(
                    self.isa_flags
                        .iter()
                        .flat_map(|(name, value)| ["isa", name.as_str(), value.as_str()]),
                ),
        )
    }
}

#[derive(Clone, Copy)]
enum CompilePolicy {
    AdvertisedOnly,
    #[cfg(feature = "test-support")]
    ImplementedForCompilerTest,
}

#[derive(Clone, Copy)]
enum GuardExit {
    Retry,
}

impl BaselineCompiler {
    pub fn host() -> Self {
        let mut flag_builder = settings::builder();
        flag_builder
            .set("opt_level", "speed")
            .expect("Cranelift opt_level setting");
        // Cranelift's IR verifier is a development aid that runs superlinear
        // passes over every function; production artifacts are validated by
        // the compiler's own frame-state and stack-map proofs instead.
        flag_builder
            .set("enable_verifier", "false")
            .expect("Cranelift enable_verifier setting");
        let flags = settings::Flags::new(flag_builder);
        let isa = cranelift_native::builder()
            .expect("host architecture is supported by Cranelift")
            .finish(flags)
            .expect("host ISA settings are valid");
        Self {
            isa,
            host_publishable: true,
        }
    }

    pub fn new(isa: OwnedTargetIsa) -> Self {
        let host_publishable = isa_is_host_compatible(&*isa);
        Self {
            isa,
            host_publishable,
        }
    }

    pub fn isa(&self) -> &dyn TargetIsa {
        &*self.isa
    }

    pub fn target_identity(&self) -> TargetIdentity {
        TargetIdentity::from_isa(self.isa())
    }

    /// Compiles verified bytecode without allocating executable memory.
    pub fn compile(&self, function: &VerifiedFunction) -> Result<RelocatableCode, CompileFailure> {
        self.compile_with_policy(function, CompilePolicy::AdvertisedOnly, None)
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn compile_implemented_for_test(
        &self,
        function: &VerifiedFunction,
    ) -> Result<RelocatableCode, CompileFailure> {
        self.compile_with_policy(function, CompilePolicy::ImplementedForCompilerTest, None)
    }

    fn compile_with_policy(
        &self,
        function: &VerifiedFunction,
        policy: CompilePolicy,
        control: Option<&CompileControl>,
    ) -> Result<RelocatableCode, CompileFailure> {
        self.compile_with_policy_and_specializations(function, policy, control, &[], &[])
    }

    fn compile_with_policy_and_specializations(
        &self,
        function: &VerifiedFunction,
        policy: CompilePolicy,
        control: Option<&CompileControl>,
        direct_calls: &[crate::runtime::DirectCallTarget],
        properties: &[BaselinePropertySite],
    ) -> Result<RelocatableCode, CompileFailure> {
        let direct_calls = direct_calls
            .iter()
            .map(|target| BaselineDirectCallSite {
                pc: target.pc(),
                call: target.call().clone(),
                entry: target.entry() as usize,
            })
            .collect::<Vec<_>>();
        self.compile_with_policy_start(
            function,
            policy,
            control,
            None,
            GuardExit::Retry,
            &direct_calls,
            properties,
        )
    }

    #[cfg(feature = "test-support")]
    pub fn lower_with_direct_target_for_test(
        &self,
        function: &VerifiedFunction,
        pc: u32,
        call: crate::runtime::CallSpecializationKey,
        entry: usize,
    ) -> Result<String, CompileFailure> {
        self.compile_with_policy_start(
            function,
            CompilePolicy::AdvertisedOnly,
            None,
            None,
            GuardExit::Retry,
            &[BaselineDirectCallSite { pc, call, entry }],
            &[],
        )
        .map(|code| code.clif().to_owned())
    }

    #[allow(clippy::too_many_arguments)] // Compiler inputs intentionally remain explicit at this boundary.
    fn compile_with_policy_start(
        &self,
        function: &VerifiedFunction,
        policy: CompilePolicy,
        control: Option<&CompileControl>,
        osr_start: Option<u32>,
        _guard_exit: GuardExit,
        direct_calls: &[BaselineDirectCallSite],
        properties: &[BaselinePropertySite],
    ) -> Result<RelocatableCode, CompileFailure> {
        if let Some(control) = control {
            control.check()?;
        }
        if self.isa.triple().pointer_width().map(|width| width.bits()) != Ok(64)
            || self.isa.triple().endianness() != Ok(Endianness::Little)
        {
            return Err(CompileFailure::InvalidArtifact);
        }
        let pointer_type = self.isa.pointer_type();
        let layout = FrameLayout::validated(
            u8::try_from(pointer_type.bytes()).map_err(|_| CompileFailure::InvalidArtifact)?,
        )?;
        let abi = crate::abi::AbiInfo::linked().map_err(|_| CompileFailure::InvalidArtifact)?;
        let element_layout = abi.element_layout();
        let object_fast = ObjectFastPaths::from_api(abi.object_api());
        let ir = match policy {
            CompilePolicy::AdvertisedOnly => BaselineIr::translate(function)?,
            #[cfg(feature = "test-support")]
            CompilePolicy::ImplementedForCompilerTest => {
                BaselineIr::translate_implemented_for_test(function)?
            }
        };
        if let Some(control) = control {
            let estimated_ir = function.snapshot().owned_bytes().saturating_mul(32);
            control.check_ir_bytes(estimated_ir)?;
        }
        let logical_stack_capacity = u32::from(function.snapshot().stack_size());
        let scratch_slots =
            u32::try_from(MAX_HELPER_SCRATCH_SLOTS).map_err(|_| CompileFailure::InvalidArtifact)?;
        let stack_capacity = logical_stack_capacity
            .checked_add(scratch_slots)
            .ok_or(CompileFailure::ResourceLimit)?;
        let required_scratch = u32::from(ir.max_stack_depth).saturating_sub(logical_stack_capacity);
        if required_scratch > scratch_slots || u32::from(ir.max_stack_depth) > stack_capacity {
            return Err(CompileFailure::ResourceLimit);
        }
        let entry_analysis = analyze_entry_domains(&ir)?;
        let mut signature = Signature::new(self.isa.default_call_conv());
        signature.params.push(AbiParam::special(
            pointer_type,
            ArgumentPurpose::StructReturn,
        ));
        signature.params.push(AbiParam::new(pointer_type));

        let mut clif = Function::with_name_signature(Default::default(), signature);
        let mut builder_context = FunctionBuilderContext::new();
        {
            let mut builder = FunctionBuilder::new(&mut clif, &mut builder_context);
            lower_function(
                &mut builder,
                &ir,
                &*self.isa,
                layout,
                element_layout,
                &entry_analysis,
                osr_start,
                GuardExit::Retry,
                direct_calls,
                properties,
                object_fast,
            )?;
            builder.seal_all_blocks();
            builder.finalize();
        }
        if let Some(control) = control {
            control.check()?;
        }

        let clif_text = clif.display().to_string();
        if let Some(control) = control {
            control.check_ir_bytes(clif_text.len())?;
        }
        let function_parameters = clif.params.clone();
        let reachable_locations = reachable_source_locations(&clif);
        let mut context = Context::for_function(clif);
        context.set_disasm(cfg!(feature = "test-support"));
        let compiled = context
            .compile(&*self.isa, &mut ControlPlane::default())
            .map_err(|_| CompileFailure::InvalidArtifact)?;
        if let Some(control) = control {
            control.check()?;
        }
        let unwind_info = compiled
            .create_unwind_info(&*self.isa)
            .map_err(|_| CompileFailure::InvalidArtifact)?
            .ok_or(CompileFailure::InvalidArtifact)?;
        let unwind_metadata = Some(retain_unwind_metadata(&unwind_info, compiled.frame_size)?);
        let native_unwind = NativeUnwindPlan::new(unwind_info, &*self.isa)?;
        let bytes = compiled.code_buffer().to_vec();
        let source_ranges = compiled.buffer.get_srclocs_sorted();
        let call_return_offsets: Vec<_> = compiled
            .buffer
            .call_sites()
            .iter()
            .map(|site| site.ret_addr)
            .collect();
        let relocations = compiled
            .buffer
            .relocs()
            .iter()
            .map(|reloc| {
                let target = match &reloc.target {
                    FinalizedRelocTarget::Func(offset) => RelocationTarget::FunctionOffset(*offset),
                    FinalizedRelocTarget::ExternalName(name) => RelocationTarget::Symbol(
                        name.display(Some(&function_parameters))
                            .to_string()
                            .into_boxed_str(),
                    ),
                };
                Relocation::with_target(
                    reloc.offset,
                    relocation_kind(reloc.kind),
                    target,
                    reloc.addend,
                )
            })
            .collect();
        let mut distinct_state_offsets = BTreeSet::new();
        let frame_states: Vec<_> = if entry_analysis.retry_before_entry {
            Vec::new()
        } else {
            ir.frame_states
                .iter()
                .enumerate()
                .filter_map(|state| {
                    let (state_index, state) = state;
                    let slots = state
                        .slots
                        .iter()
                        .map(|slot| match *slot {
                            FrameSlot::Argument(index) => Ok(index),
                            FrameSlot::Local(index) => ir
                                .argument_count
                                .checked_add(index)
                                .ok_or(CompileFailure::ResourceLimit),
                            FrameSlot::Stack(index) => ir
                                .argument_count
                                .checked_add(ir.local_count)
                                .and_then(|base| base.checked_add(index))
                                .ok_or(CompileFailure::ResourceLimit),
                        })
                        .collect::<Result<Vec<_>, _>>();
                    let slots = match slots {
                        Ok(slots) => slots,
                        Err(error) => return Some(Err(error)),
                    };
                    let state_id =
                        FrameStateId::from_index(state_index).ok_or(CompileFailure::ResourceLimit);
                    let state_id = match state_id {
                        Ok(state_id) => state_id,
                        Err(error) => return Some(Err(error)),
                    };
                    let source_location = match frame_state_source_loc(state_id) {
                        Ok(location) => location.bits(),
                        Err(error) => return Some(Err(error)),
                    };
                    let matching_ranges: Vec<_> = source_ranges
                        .iter()
                        .filter(|range| range.loc.bits() == source_location)
                        .collect();
                    let [source_range] = matching_ranges.as_slice() else {
                        // A type-proven Tier 1 fast path may make its cold
                        // helper edge unreachable. Cranelift then removes the
                        // call and its source range; there is no machine
                        // safepoint to describe for that helper state.
                        if state.kind == FrameStateKind::Helper && matching_ranges.is_empty() {
                            return None;
                        }
                        if osr_start.is_some() && matching_ranges.is_empty() {
                            return None;
                        }
                        // A state lowered only into blocks the CLIF control
                        // flow graph cannot reach (for example the handler of
                        // a try region with no exceptional edge) is removed
                        // with its block; no machine code can observe it.
                        if matching_ranges.is_empty()
                            && !reachable_locations.contains(&source_location)
                        {
                            return None;
                        }
                        return Some(Err(CompileFailure::InvalidArtifact));
                    };
                    let (location_kind, code_offset) = match state.kind {
                        FrameStateKind::Poll | FrameStateKind::Helper => {
                            let matching_calls: Vec<_> = call_return_offsets
                                .iter()
                                .copied()
                                .filter(|return_address| {
                                    source_range.start < *return_address
                                        && *return_address <= source_range.end
                                })
                                .collect();
                            let [return_address] = matching_calls.as_slice() else {
                                return Some(Err(CompileFailure::InvalidArtifact));
                            };
                            (FrameStateLocationKind::CallReturn, *return_address)
                        }
                        FrameStateKind::Marker => {
                            if call_return_offsets.iter().any(|return_address| {
                                source_range.start < *return_address
                                    && *return_address <= source_range.end
                            }) {
                                return Some(Err(CompileFailure::InvalidArtifact));
                            }
                            (FrameStateLocationKind::Marker, source_range.start)
                        }
                    };
                    if code_offset as usize >= bytes.len()
                        || !distinct_state_offsets.insert(code_offset)
                    {
                        return Some(Err(CompileFailure::InvalidArtifact));
                    }
                    Some(Ok(ArtifactFrameState::with_location(
                        code_offset,
                        state.pc,
                        slots,
                        location_kind,
                        source_location,
                        source_range.start,
                        source_range.end,
                    )))
                })
                .collect::<Result<_, _>>()?
        };
        let stack_maps = frame_states
            .iter()
            .map(|state| StackMap::new(state.code_offset, state.slots.to_vec()))
            .collect();
        let mut code = RelocatableCode {
            bytes,
            relocations,
            unwind_metadata,
            native_unwind,
            stack_maps,
            frame_states,
            call_return_offsets,
            target: self.isa.triple().clone(),
            host_publishable: self.host_publishable,
            clif: clif_text,
            machine_disassembly: compiled.vcode.clone(),
            osr_codes: Vec::new(),
        };
        #[cfg(feature = "test-support")]
        dump_code_for_diagnostics(
            if osr_start.is_some() {
                "tier1-osr"
            } else {
                "tier1"
            },
            &code.clif,
            code.machine_disassembly(),
        );
        if osr_start.is_none() && matches!(policy, CompilePolicy::AdvertisedOnly) {
            for point in function.osr_points() {
                let Some(map) =
                    crate::runtime::OsrMap::from_verified(function, point.pc(), point.pc())
                else {
                    continue;
                };
                let child = self.compile_with_policy_start(
                    function,
                    policy,
                    control,
                    Some(point.pc()),
                    GuardExit::Retry,
                    direct_calls,
                    properties,
                )?;
                code.osr_codes.push((map, child));
            }
        }
        Ok(code)
    }
}

#[cfg(test)]
mod target_identity_tests {
    use super::*;

    #[test]
    fn target_identity_is_the_canonical_identity_of_the_compilers_actual_isa() {
        let compiler = BaselineCompiler::host();
        assert_eq!(
            compiler.target_identity(),
            TargetIdentity::from_isa(compiler.isa())
        );
        assert_eq!(
            compiler.target_identity().triple(),
            compiler.isa().triple().to_string()
        );
        assert!(!compiler.target_identity().shared_flags().is_empty());
        assert!(!compiler.target_identity().isa_flags().is_empty());
    }

    #[test]
    fn every_codegen_setting_value_participates_in_target_identity() {
        let compiler = BaselineCompiler::host();
        let identity = compiler.target_identity();
        assert!(!identity.shared_flags().is_empty());
        for index in 0..identity.shared_flags().len() {
            let mut changed = identity.clone();
            changed.shared_flags[index].1.push_str("-changed");
            assert_ne!(
                identity.codegen_fingerprint(),
                changed.codegen_fingerprint(),
                "setting {} was omitted from the identity",
                identity.shared_flags()[index].0
            );
        }
        for index in 0..identity.isa_flags().len() {
            let mut changed = identity.clone();
            changed.isa_flags[index].1.push_str("-changed");
            assert_ne!(
                identity.codegen_fingerprint(),
                changed.codegen_fingerprint()
            );
        }
    }
}

impl Compiler for BaselineCompiler {
    fn compile(&self, request: CompileRequest) -> Result<CompiledArtifact, CompileFailure> {
        let dependencies = request
            .direct_call_targets()
            .iter()
            .map(crate::runtime::DirectCallTarget::publication)
            .collect::<Vec<_>>();
        let artifact_dependencies = request
            .direct_call_targets()
            .iter()
            .map(|target| crate::code_cache::ArtifactDependency::new(target.call().callee()))
            .collect::<Vec<_>>();
        let direct_signature = request.feedback().bounded_specialization(request.key());
        let direct_code = direct_signature.as_ref().and_then(|signature| {
            super::optimized::lower_direct_call_machine(
                &self.isa,
                request.snapshot(),
                signature,
                Some(request.feedback()),
                None,
            )
            .ok()
        });
        let properties = baseline_property_sites(request.snapshot(), request.feedback());
        let code = self.compile_with_policy_and_specializations(
            request.snapshot(),
            CompilePolicy::AdvertisedOnly,
            None,
            request.direct_call_targets(),
            &properties,
        )?;
        let mut artifact = artifact_from_relocatable(request, code)
            .with_dependencies(artifact_dependencies)
            .with_direct_call_dependencies(dependencies);
        if let (Some(signature), Some(direct_code)) = (direct_signature, direct_code) {
            artifact = artifact
                .with_optimized_metadata(
                    crate::code_cache::OptimizedArtifactMetadata::new(
                        signature.feedback_epoch(),
                        Vec::new(),
                        0,
                        0,
                        0,
                    )
                    .with_direct_call_signature(signature),
                )
                .with_direct_call_relocatable(direct_code);
        }
        Ok(artifact)
    }

    fn compile_controlled(
        &self,
        request: CompileRequest,
        control: &CompileControl,
    ) -> Result<CompiledArtifact, CompileFailure> {
        let dependencies = request
            .direct_call_targets()
            .iter()
            .map(crate::runtime::DirectCallTarget::publication)
            .collect::<Vec<_>>();
        let artifact_dependencies = request
            .direct_call_targets()
            .iter()
            .map(|target| crate::code_cache::ArtifactDependency::new(target.call().callee()))
            .collect::<Vec<_>>();
        let direct_signature = request.feedback().bounded_specialization(request.key());
        let direct_code = direct_signature.as_ref().and_then(|signature| {
            super::optimized::lower_direct_call_machine(
                &self.isa,
                request.snapshot(),
                signature,
                Some(request.feedback()),
                Some(control),
            )
            .ok()
        });
        let properties = baseline_property_sites(request.snapshot(), request.feedback());
        let code = self.compile_with_policy_and_specializations(
            request.snapshot(),
            CompilePolicy::AdvertisedOnly,
            Some(control),
            request.direct_call_targets(),
            &properties,
        )?;
        control.check()?;
        let mut artifact = artifact_from_relocatable(request, code)
            .with_dependencies(artifact_dependencies)
            .with_direct_call_dependencies(dependencies);
        if let (Some(signature), Some(direct_code)) = (direct_signature, direct_code) {
            artifact = artifact
                .with_optimized_metadata(
                    crate::code_cache::OptimizedArtifactMetadata::new(
                        signature.feedback_epoch(),
                        Vec::new(),
                        0,
                        0,
                        0,
                    )
                    .with_direct_call_signature(signature),
                )
                .with_direct_call_relocatable(direct_code);
        }
        Ok(artifact)
    }
}

/// Finalizes Cranelift IR produced by the independent optimizing builder. This
/// owns only target encoding/unwind packaging; it does not translate or lower
/// baseline IR.
/// Native-call entries call themselves through a colocated user name, and a
/// Tier 2 body names the native entry published in its own artifact. Both are
/// resolved at publication; every other external name keeps its symbol text.
fn native_call_relocation_target(
    name: &cranelift_codegen::ir::ExternalName,
    parameters: &cranelift_codegen::ir::function::FunctionParameters,
) -> Option<RelocationTarget> {
    let cranelift_codegen::ir::ExternalName::User(reference) = name else {
        return None;
    };
    let user = parameters.user_named_funcs().get(*reference)?;
    if user.index != 0 {
        return None;
    }
    match user.namespace {
        super::native_call::NATIVE_SELF_NAMESPACE => Some(RelocationTarget::FunctionOffset(0)),
        super::native_call::NATIVE_ENTRY_NAMESPACE => Some(RelocationTarget::Symbol(
            super::native_call::NATIVE_ENTRY_SYMBOL.into(),
        )),
        _ => None,
    }
}

pub(crate) fn finalize_optimized_machine(
    isa: &OwnedTargetIsa,
    clif: Function,
    control: Option<&CompileControl>,
    requires_helper_stack_map: bool,
) -> Result<RelocatableCode, CompileFailure> {
    finalize_optimized_machine_with_lowered_text(
        isa,
        clif,
        control,
        requires_helper_stack_map,
        None,
    )
}

/// As [`finalize_optimized_machine`]. `lowered_text` is the builder's CLIF
/// before any post-lowering cleanup; it becomes the artifact's `clif()` text so
/// lowering-structure evidence is independent of the machine-level cleanups
/// (constant canonicalization, constant-branch folding, exit
/// rematerialization) that only change register pressure and dead code.
pub(crate) fn finalize_optimized_machine_with_lowered_text(
    isa: &OwnedTargetIsa,
    clif: Function,
    control: Option<&CompileControl>,
    requires_helper_stack_map: bool,
    lowered_text: Option<String>,
) -> Result<RelocatableCode, CompileFailure> {
    let clif_text = lowered_text.unwrap_or_else(|| clif.display().to_string());
    if let Some(control) = control {
        control.check()?;
        control.check_ir_bytes(clif_text.len())?;
    }
    let mut clif = clif;
    canonicalize_integer_constants(&mut clif);
    fold_constant_branches(&mut clif);
    let function_parameters = clif.params.clone();
    let mut context = Context::for_function(clif);
    // With shared constants, loop-invariant tags and values become constant
    // block parameters. Remove those first so guards on them fold to jumps
    // too; `compile` below reruns the complete Cranelift pipeline.
    context.compute_cfg();
    context.compute_domtree();
    context
        .eliminate_unreachable_code(&**isa)
        .map_err(|_| CompileFailure::InvalidArtifact)?;
    context
        .remove_constant_phis(&**isa)
        .map_err(|_| CompileFailure::InvalidArtifact)?;
    context.func.dfg.resolve_all_aliases();
    fold_bitcast_round_trips(&mut context.func);
    fold_constant_branches(&mut context.func);
    // Folding can remove the only helper call (for example one guarded by a
    // proven check); an artifact without calls publishes no helper map.
    let requires_helper_stack_map = requires_helper_stack_map
        && context.func.layout.blocks().any(|block| {
            context
                .func
                .layout
                .block_insts(block)
                .any(|inst| context.func.dfg.insts[inst].opcode().is_call())
        });
    #[cfg(feature = "test-support")]
    let final_text = diagnostics_directory()
        .is_some()
        .then(|| context.func.display().to_string());
    context.set_disasm(cfg!(feature = "test-support"));
    let compiled = context
        .compile(&**isa, &mut ControlPlane::default())
        .map_err(|_| CompileFailure::InvalidArtifact)?;
    if let Some(control) = control {
        control.check()?;
    }
    let unwind_info = compiled
        .create_unwind_info(&**isa)
        .map_err(|_| CompileFailure::InvalidArtifact)?
        .ok_or(CompileFailure::InvalidArtifact)?;
    let unwind_metadata = Some(retain_unwind_metadata(&unwind_info, compiled.frame_size)?);
    let native_unwind = NativeUnwindPlan::new(unwind_info, &**isa)?;
    let bytes = compiled.code_buffer().to_vec();
    let relocations = compiled
        .buffer
        .relocs()
        .iter()
        .map(|reloc| {
            let target = match &reloc.target {
                FinalizedRelocTarget::Func(offset) => RelocationTarget::FunctionOffset(*offset),
                FinalizedRelocTarget::ExternalName(name) => {
                    native_call_relocation_target(name, &function_parameters).unwrap_or_else(|| {
                        RelocationTarget::Symbol(
                            name.display(Some(&function_parameters))
                                .to_string()
                                .into_boxed_str(),
                        )
                    })
                }
            };
            Relocation::with_target(
                reloc.offset,
                relocation_kind(reloc.kind),
                target,
                reloc.addend,
            )
        })
        .collect();
    let call_return_offsets = compiled
        .buffer
        .call_sites()
        .iter()
        .map(|site| site.ret_addr)
        .collect::<Vec<_>>();
    let (stack_maps, frame_states) = if requires_helper_stack_map {
        let code_offset = *call_return_offsets
            .first()
            .ok_or(CompileFailure::InvalidArtifact)?;
        (
            vec![StackMap::new(code_offset, Vec::new())],
            vec![ArtifactFrameState::with_location(
                code_offset,
                0,
                Vec::new(),
                FrameStateLocationKind::CallReturn,
                0,
                code_offset.saturating_sub(1),
                code_offset,
            )],
        )
    } else {
        (Vec::new(), Vec::new())
    };
    #[cfg(feature = "test-support")]
    if let Some(final_text) = final_text {
        dump_code_for_diagnostics(
            "tier2",
            &final_text,
            compiled.vcode.as_deref().unwrap_or(""),
        );
    }
    Ok(RelocatableCode {
        bytes,
        relocations,
        unwind_metadata,
        native_unwind,
        stack_maps,
        frame_states,
        call_return_offsets,
        target: isa.triple().clone(),
        host_publishable: isa_is_host_compatible(&**isa),
        clif: clif_text,
        machine_disassembly: compiled.vcode.clone(),
        osr_codes: Vec::new(),
    })
}

/// Rewrites every `iconst` to one canonical definition per (type, value) at
/// the top of the entry block.
///
/// The optimizing lowering materializes tag and payload constants where they
/// are needed, so SSA construction turns loop-invariant constants such as the
/// Int32 tag into block parameters whose incoming values are distinct `iconst`
/// instructions. Cranelift's constant-phi removal compares value identity and
/// runs before GVN, so those parameters otherwise survive into register
/// allocation and cost a register (and moves) per loop-carried slot. Constants
/// are pure and the entry block dominates every block, so sharing one
/// definition preserves semantics exactly.
pub(crate) fn canonicalize_integer_constants(function: &mut Function) {
    use cranelift_codegen::cursor::{Cursor, FuncCursor};
    use cranelift_codegen::ir::{InstructionData, Opcode};
    let Some(entry) = function.layout.entry_block() else {
        return;
    };
    function.dfg.resolve_all_aliases();
    let mut constants = Vec::new();
    for block in function.layout.blocks() {
        for inst in function.layout.block_insts(block) {
            if let InstructionData::UnaryImm {
                opcode: Opcode::Iconst,
                imm,
            } = function.dfg.insts[inst]
            {
                let value = function.dfg.first_result(inst);
                constants.push((inst, value, function.dfg.value_type(value), imm.bits()));
            }
        }
    }
    if constants.len() < 2 {
        return;
    }
    let mut canonical = std::collections::HashMap::new();
    let mut cursor = FuncCursor::new(function);
    cursor.goto_first_insertion_point(entry);
    for &(_, _, ty, bits) in &constants {
        canonical
            .entry((ty, bits))
            .or_insert_with(|| cursor.ins().iconst(ty, bits));
    }
    let mut replacements = std::collections::HashMap::new();
    for &(inst, value, ty, bits) in &constants {
        cursor.func.layout.remove_inst(inst);
        replacements.insert(value, canonical[&(ty, bits)]);
    }
    let function = cursor.func;
    let blocks = function.layout.blocks().collect::<Vec<_>>();
    for block in blocks {
        let mut next = function.layout.first_inst(block);
        while let Some(inst) = next {
            next = function.layout.next_inst(inst);
            function.dfg.map_inst_values(inst, |value| {
                replacements.get(&value).copied().unwrap_or(value)
            });
        }
    }
}

/// Forwards `bitcast.T (bitcast.U x)` to `x` when `x` already has type `T`.
///
/// The optimizing lowering keeps Float64 payloads in 64-bit integer frame
/// variables, so each arithmetic step converts its operands back to `f64`
/// and its result to `i64`. Without this rewrite every chained Float64
/// operation pays a GPR/XMM move pair on its dependency chain. Only scalar
/// bitcasts are forwarded; a round trip through another type of the same
/// width is the identity on the bits.
pub(crate) fn fold_bitcast_round_trips(function: &mut Function) {
    use cranelift_codegen::ir::{InstructionData, Opcode, ValueDef};
    let mut replacements = std::collections::HashMap::new();
    for block in function.layout.blocks() {
        for inst in function.layout.block_insts(block) {
            let InstructionData::LoadNoOffset {
                opcode: Opcode::Bitcast,
                arg,
                ..
            } = function.dfg.insts[inst]
            else {
                continue;
            };
            let ValueDef::Result(source, 0) = function.dfg.value_def(arg) else {
                continue;
            };
            let InstructionData::LoadNoOffset {
                opcode: Opcode::Bitcast,
                arg: original,
                ..
            } = function.dfg.insts[source]
            else {
                continue;
            };
            let result = function.dfg.first_result(inst);
            let ty = function.dfg.value_type(result);
            let middle = function.dfg.value_type(arg);
            if ty.is_vector() || middle.is_vector() || function.dfg.value_type(original) != ty {
                continue;
            }
            replacements.insert(result, original);
        }
    }
    if replacements.is_empty() {
        return;
    }
    let blocks = function.layout.blocks().collect::<Vec<_>>();
    for block in blocks {
        let mut next = function.layout.first_inst(block);
        while let Some(inst) = next {
            next = function.layout.next_inst(inst);
            function.dfg.map_inst_values(inst, |value| {
                replacements.get(&value).copied().unwrap_or(value)
            });
        }
    }
}

/// Evaluates `value` when it is a compile-time integer built only from
/// constants, integer width conversions, bitwise operations and integer
/// comparisons. Results are normalized to the value's type width.
fn constant_integer_value(
    function: &Function,
    value: cranelift_codegen::ir::Value,
    memo: &mut std::collections::HashMap<cranelift_codegen::ir::Value, Option<i64>>,
    depth: u32,
) -> Option<i64> {
    use cranelift_codegen::ir::{condcodes::IntCC, InstructionData, Opcode, ValueDef};
    if let Some(&known) = memo.get(&value) {
        return known;
    }
    if depth > 64 {
        return None;
    }
    let ty = function.dfg.value_type(value);
    if !ty.is_int() || ty.bits() > 64 {
        return None;
    }
    let bits = ty.bits();
    let wrap = |raw: i64| -> i64 {
        if bits == 64 {
            raw
        } else {
            let shift = 64 - bits;
            (raw << shift) >> shift
        }
    };
    let ValueDef::Result(inst, 0) = function.dfg.value_def(value) else {
        return None;
    };
    let mut operand = |value| constant_integer_value(function, value, memo, depth + 1);
    let compare = |cond: IntCC, lhs: i64, rhs: i64, width: u32| -> i64 {
        let unsigned = |raw: i64| -> u64 {
            if width == 64 {
                raw as u64
            } else {
                (raw as u64) & ((1u64 << width) - 1)
            }
        };
        let result = match cond {
            IntCC::Equal => lhs == rhs,
            IntCC::NotEqual => lhs != rhs,
            IntCC::SignedLessThan => lhs < rhs,
            IntCC::SignedGreaterThanOrEqual => lhs >= rhs,
            IntCC::SignedGreaterThan => lhs > rhs,
            IntCC::SignedLessThanOrEqual => lhs <= rhs,
            IntCC::UnsignedLessThan => unsigned(lhs) < unsigned(rhs),
            IntCC::UnsignedGreaterThanOrEqual => unsigned(lhs) >= unsigned(rhs),
            IntCC::UnsignedGreaterThan => unsigned(lhs) > unsigned(rhs),
            IntCC::UnsignedLessThanOrEqual => unsigned(lhs) <= unsigned(rhs),
        };
        i64::from(result)
    };
    let result = match function.dfg.insts[inst] {
        InstructionData::UnaryImm {
            opcode: Opcode::Iconst,
            imm,
        } => Some(wrap(imm.bits())),
        InstructionData::Unary { opcode, arg } => {
            let arg_ty = function.dfg.value_type(arg);
            let raw = operand(arg);
            match (opcode, raw) {
                (Opcode::Ireduce | Opcode::Sextend, Some(raw)) => Some(wrap(raw)),
                (Opcode::Uextend, Some(raw)) if arg_ty.bits() < 64 => {
                    Some(wrap(raw & ((1i64 << arg_ty.bits()) - 1)))
                }
                (Opcode::Bnot, Some(raw)) => Some(wrap(!raw)),
                _ => None,
            }
        }
        InstructionData::Binary { opcode, args } => {
            match (opcode, operand(args[0]), operand(args[1])) {
                (Opcode::Band, Some(lhs), Some(rhs)) => Some(wrap(lhs & rhs)),
                (Opcode::Bor, Some(lhs), Some(rhs)) => Some(wrap(lhs | rhs)),
                (Opcode::Bxor, Some(lhs), Some(rhs)) => Some(wrap(lhs ^ rhs)),
                // An and with a known zero, or an or with a known all-ones
                // operand, is constant whatever the other operand is.
                (Opcode::Band, Some(0), _) | (Opcode::Band, _, Some(0)) => Some(0),
                (Opcode::Bor, Some(-1), _) | (Opcode::Bor, _, Some(-1)) => Some(wrap(-1)),
                _ => None,
            }
        }
        InstructionData::BinaryImm64 { opcode, arg, imm } => {
            let imm = wrap(imm.bits());
            match (opcode, operand(arg)) {
                (Opcode::BandImm, Some(raw)) => Some(wrap(raw & imm)),
                (Opcode::BorImm, Some(raw)) => Some(wrap(raw | imm)),
                (Opcode::BxorImm, Some(raw)) => Some(wrap(raw ^ imm)),
                _ => None,
            }
        }
        InstructionData::IntCompare { cond, args, .. } => {
            let width = function.dfg.value_type(args[0]).bits();
            match (operand(args[0]), operand(args[1])) {
                (Some(lhs), Some(rhs)) if width <= 64 => Some(compare(cond, lhs, rhs, width)),
                _ => None,
            }
        }
        InstructionData::IntCompareImm { cond, arg, imm, .. } => {
            let width = function.dfg.value_type(arg).bits();
            let rhs = if width == 64 {
                imm.bits()
            } else {
                let shift = 64 - width;
                (imm.bits() << shift) >> shift
            };
            operand(arg)
                .filter(|_| width <= 64)
                .map(|lhs| compare(cond, lhs, rhs, width))
        }
        _ => None,
    };
    memo.insert(value, result);
    result
}

/// Replaces `brif` on a compile-time constant condition with a jump.
///
/// Guards whose checks the specializer has already proven (for example a tag
/// comparison against the canonical Int32 tag constant) reach Cranelift as
/// branches on folded constants, which its mid-end does not turn into jumps.
/// The dead deopt blocks then keep values live for register allocation and
/// the hot path executes a materialize/test/branch per proven guard. The
/// untaken successor is unreachable and is removed by Cranelift's
/// unreachable-code elimination.
pub(crate) fn fold_constant_branches(function: &mut Function) {
    use cranelift_codegen::ir::{InstBuilder, InstructionData, Opcode};
    let mut memo = std::collections::HashMap::new();
    let mut folds = Vec::new();
    for block in function.layout.blocks() {
        let Some(inst) = function.layout.last_inst(block) else {
            continue;
        };
        let InstructionData::Brif {
            opcode: Opcode::Brif,
            arg,
            blocks,
        } = function.dfg.insts[inst]
        else {
            continue;
        };
        let Some(condition) = constant_integer_value(function, arg, &mut memo, 0) else {
            continue;
        };
        let taken = blocks[usize::from(condition == 0)];
        let target = taken.block(&function.dfg.value_lists);
        let arguments = taken.args_slice(&function.dfg.value_lists).to_vec();
        folds.push((inst, target, arguments));
    }
    if folds.is_empty() {
        return;
    }
    for (inst, target, arguments) in folds {
        function.dfg.replace(inst).jump(target, &arguments);
    }
    // Drop the blocks the folded branches made unreachable, so callers that
    // inspect the finalized IR (for example to decide whether a helper call
    // remains) see exactly the code that will be emitted.
    let Some(entry) = function.layout.entry_block() else {
        return;
    };
    let mut reachable = std::collections::HashSet::new();
    let mut worklist = vec![entry];
    while let Some(block) = worklist.pop() {
        if !reachable.insert(block) {
            continue;
        }
        for inst in function.layout.block_insts(block) {
            for successor in function.dfg.insts[inst].branch_destination(&function.dfg.jump_tables)
            {
                worklist.push(successor.block(&function.dfg.value_lists));
            }
        }
    }
    let unreachable = function
        .layout
        .blocks()
        .filter(|block| !reachable.contains(block))
        .collect::<Vec<_>>();
    for block in unreachable {
        while let Some(inst) = function.layout.first_inst(block) {
            function.layout.remove_inst(inst);
        }
        function.layout.remove_block(block);
    }
}

/// Test-support diagnostic: when `QJSJIT_DUMP_CODE` names a directory, write
/// each finalized function's Cranelift IR and machine disassembly there so
/// benchmark kernels can be inspected under production tiering.
#[cfg(feature = "test-support")]
fn diagnostics_directory() -> Option<std::ffi::OsString> {
    std::env::var_os("QJSJIT_DUMP_CODE")
}

#[cfg(feature = "test-support")]
fn dump_code_for_diagnostics(kind: &str, clif: &str, machine: &str) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let Some(directory) = diagnostics_directory() else {
        return;
    };
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::path::Path::new(&directory)
        .join(format!("{}-{sequence:04}-{kind}.txt", std::process::id()));
    let _ = std::fs::write(path, format!("{clif}\n;; machine code\n{machine}"));
}

pub(crate) fn artifact_from_relocatable(
    request: CompileRequest,
    code: RelocatableCode,
) -> CompiledArtifact {
    let mut charged_code = Vec::with_capacity(code.total_code_bytes());
    let mut charged_relocations = Vec::new();
    let mut charged_stack_maps = Vec::new();
    let mut charged_frame_states = Vec::new();
    code.collect_charge_metadata(
        &mut charged_code,
        &mut charged_relocations,
        &mut charged_stack_maps,
        &mut charged_frame_states,
    );
    CompiledArtifact::from_parts(
        request.artifact_key(),
        CodeAllocation::inert(charged_code),
        charged_relocations,
        charged_stack_maps,
        charged_frame_states,
        Vec::new(),
    )
    .with_inline_snapshot(request.snapshot())
    .with_unwind_metadata(code.unwind_metadata.clone())
    .with_relocatable(code)
}

/// Relocatable Cranelift output. Publication is intentionally host-gated.
#[derive(Clone, Debug)]
pub struct RelocatableCode {
    bytes: Vec<u8>,
    relocations: Vec<Relocation>,
    unwind_metadata: Option<UnwindMetadata>,
    native_unwind: NativeUnwindPlan,
    stack_maps: Vec<StackMap>,
    frame_states: Vec<ArtifactFrameState>,
    call_return_offsets: Vec<u32>,
    target: Triple,
    host_publishable: bool,
    clif: String,
    machine_disassembly: Option<String>,
    osr_codes: Vec<(crate::runtime::OsrMap, RelocatableCode)>,
}

impl RelocatableCode {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn osr_entry_count(&self) -> usize {
        self.osr_codes.len()
    }

    pub fn total_code_bytes(&self) -> usize {
        self.osr_codes
            .iter()
            .fold(self.bytes.len(), |total, (_, code)| {
                total.saturating_add(code.total_code_bytes())
            })
    }

    fn collect_charge_metadata(
        &self,
        code_bytes: &mut Vec<u8>,
        relocations: &mut Vec<Relocation>,
        stack_maps: &mut Vec<StackMap>,
        frame_states: &mut Vec<ArtifactFrameState>,
    ) {
        code_bytes.extend_from_slice(&self.bytes);
        relocations.extend_from_slice(&self.relocations);
        stack_maps.extend_from_slice(&self.stack_maps);
        frame_states.extend_from_slice(&self.frame_states);
        for (_, code) in &self.osr_codes {
            code.collect_charge_metadata(code_bytes, relocations, stack_maps, frame_states);
        }
    }

    pub fn relocations(&self) -> &[Relocation] {
        &self.relocations
    }

    #[cfg(feature = "test-support")]
    pub fn machine_disassembly(&self) -> &str {
        self.machine_disassembly.as_deref().unwrap_or("")
    }

    pub fn unwind_metadata(&self) -> Option<&UnwindMetadata> {
        self.unwind_metadata.as_ref()
    }

    pub fn stack_maps(&self) -> &[StackMap] {
        &self.stack_maps
    }

    pub fn frame_states(&self) -> &[ArtifactFrameState] {
        &self.frame_states
    }

    pub fn call_return_offsets(&self) -> &[u32] {
        &self.call_return_offsets
    }

    /// Textual CLIF retained for ABI and safe-point audits.
    pub fn clif(&self) -> &str {
        &self.clif
    }

    pub fn target(&self) -> &Triple {
        &self.target
    }

    pub fn publish(self) -> Result<PublishedBaselineCode, CodeMemoryError> {
        self.publish_with(&[], Vec::new())
    }

    /// Publishes with named relocation symbols resolved to absolute
    /// addresses. `retained` publications stay executable at least as long
    /// as this code, which may embed their addresses.
    pub(crate) fn publish_with(
        self,
        symbols: &[(&str, u64)],
        retained: Vec<PublishedBaselineCode>,
    ) -> Result<PublishedBaselineCode, CodeMemoryError> {
        if !self.host_publishable || self.target != Triple::host() {
            return Err(CodeMemoryError::TargetIsaMismatch);
        }
        let mut bytes = self.bytes;
        let native_unwind = self.native_unwind.prepare(&mut bytes)?;
        let allocator = CodeAllocator::for_host()?;
        let mut writable = allocator.allocate(bytes.len())?;
        writable.write(0, &bytes)?;
        let base = writable.as_ptr() as usize as u64;
        let mut resolved = Vec::with_capacity(self.relocations.len());
        for relocation in &self.relocations {
            let resolved_relocation = relocation
                .resolve_with(|target| match target {
                    RelocationTarget::FunctionOffset(offset) => {
                        base.checked_add(u64::from(*offset))
                    }
                    RelocationTarget::Absolute(value) => Some(*value),
                    RelocationTarget::Symbol(name) => symbols
                        .iter()
                        .find(|(symbol, _)| *symbol == &**name)
                        .map(|(_, address)| *address),
                })
                .map_err(|_| CodeMemoryError::UnresolvedRelocationTarget)?;
            resolved.push(resolved_relocation);
        }
        writable.apply_relocations(&resolved)?;
        writable.declare_indirect_targets(&[0])?;
        let executable = writable.publish()?;
        let unwind_registration = native_unwind.register(&executable)?;
        let osr_codes = self
            .osr_codes
            .into_iter()
            .map(|(map, code)| Ok((map, code.publish_with(symbols, retained.clone())?)))
            .collect::<Result<Vec<_>, CodeMemoryError>>()?;
        let mut published = PublishedBaselineCode::new(
            executable,
            unwind_registration,
            self.unwind_metadata,
            self.stack_maps,
            self.frame_states,
            osr_codes,
        );
        if !retained.is_empty() {
            Arc::get_mut(&mut published.allocation)
                .expect("new publication is uniquely owned")
                .retained = retained.into_boxed_slice();
        }
        Ok(published)
    }
}

/// Published Tier 1 code together with metadata pinned for its full RX lifetime.
#[derive(Clone, Debug)]
pub struct PublishedBaselineCode {
    allocation: Arc<PublishedBaselineAllocation>,
}

#[derive(Debug)]
struct PublishedBaselineAllocation {
    unwind_registration: Option<NativeUnwindRegistration>,
    executable: Option<ExecutableCode>,
    unwind_metadata: Option<UnwindMetadata>,
    stack_maps: Box<[StackMap]>,
    frame_states: Box<[ArtifactFrameState]>,
    osr_codes: Box<[(crate::runtime::OsrMap, PublishedBaselineCode)]>,
    /// Publications whose absolute addresses this code embeds.
    retained: Box<[PublishedBaselineCode]>,
    #[cfg(feature = "test-support")]
    lifetime_events: Arc<Mutex<Vec<&'static str>>>,
}

impl PublishedBaselineCode {
    fn new(
        executable: ExecutableCode,
        unwind_registration: NativeUnwindRegistration,
        unwind_metadata: Option<UnwindMetadata>,
        stack_maps: Vec<StackMap>,
        frame_states: Vec<ArtifactFrameState>,
        osr_codes: Vec<(crate::runtime::OsrMap, PublishedBaselineCode)>,
    ) -> Self {
        Self {
            allocation: Arc::new(PublishedBaselineAllocation {
                unwind_registration: Some(unwind_registration),
                executable: Some(executable),
                unwind_metadata,
                stack_maps: stack_maps.into_boxed_slice(),
                frame_states: frame_states.into_boxed_slice(),
                osr_codes: osr_codes.into_boxed_slice(),
                retained: Box::new([]),
                #[cfg(feature = "test-support")]
                lifetime_events: Arc::new(Mutex::new(Vec::new())),
            }),
        }
    }

    pub fn unwind_metadata(&self) -> Option<&UnwindMetadata> {
        self.allocation.unwind_metadata.as_ref()
    }

    pub fn len(&self) -> usize {
        self.allocation
            .executable
            .as_ref()
            .expect("published executable remains live while pinned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.allocation
            .executable
            .as_ref()
            .expect("published executable remains live while pinned")
            .as_ptr()
    }

    pub const fn is_writable(&self) -> bool {
        false
    }

    pub fn stack_maps(&self) -> &[StackMap] {
        &self.allocation.stack_maps
    }

    pub fn required_stack_map_count(&self) -> u32 {
        self.allocation
            .frame_states
            .iter()
            .map(|state| state.source_location)
            .max()
            .and_then(|maximum| maximum.checked_add(1))
            .unwrap_or(0)
    }

    pub fn frame_states(&self) -> &[ArtifactFrameState] {
        &self.allocation.frame_states
    }

    pub fn osr_entry(&self, pc: u32) -> Option<(&crate::runtime::OsrMap, &PublishedBaselineCode)> {
        self.allocation
            .osr_codes
            .iter()
            .find(|(map, _)| map.key().pc() == pc)
            .map(|(map, code)| (map, code))
    }

    pub fn unwind_is_registered(&self) -> bool {
        self.allocation.unwind_registration.is_some()
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn lifetime_probe(&self) -> PublishedLifetimeProbe {
        PublishedLifetimeProbe(Arc::clone(&self.allocation.lifetime_events))
    }
}

impl Drop for PublishedBaselineAllocation {
    fn drop(&mut self) {
        drop(self.unwind_registration.take());
        #[cfg(feature = "test-support")]
        self.lifetime_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push("unwind_deregistered");

        drop(self.executable.take());
        #[cfg(feature = "test-support")]
        self.lifetime_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push("executable_released");
    }
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct PublishedLifetimeProbe(Arc<Mutex<Vec<&'static str>>>);

#[cfg(feature = "test-support")]
impl PublishedLifetimeProbe {
    pub fn events(&self) -> Vec<&'static str> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

fn retain_unwind_metadata(
    unwind: &CraneliftUnwindInfo,
    frame_size: u32,
) -> Result<UnwindMetadata, CompileFailure> {
    let kind = match &unwind {
        CraneliftUnwindInfo::SystemV(_) => UnwindKind::SystemV,
        CraneliftUnwindInfo::WindowsX64(_) => UnwindKind::WindowsX64,
        CraneliftUnwindInfo::WindowsArm64(_) => UnwindKind::WindowsArm64,
        _ => return Err(CompileFailure::InvalidArtifact),
    };
    let encoding = postcard::to_allocvec(unwind).map_err(|_| CompileFailure::InvalidArtifact)?;
    Ok(UnwindMetadata::new(kind, frame_size, encoding))
}

#[derive(Clone, Debug)]
struct NativeUnwindPlan {
    info: CraneliftUnwindInfo,
    #[cfg(all(
        any(target_os = "linux", target_os = "macos"),
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ))]
    systemv_cie: Option<gimli::write::CommonInformationEntry>,
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    windows_function_len: Option<u32>,
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    windows_unwind_offset: Option<u32>,
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    windows_arm64_unwind_offset: Option<u32>,
}

impl NativeUnwindPlan {
    fn new(info: CraneliftUnwindInfo, isa: &dyn TargetIsa) -> Result<Self, CompileFailure> {
        #[cfg(all(
            any(target_os = "linux", target_os = "macos"),
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))]
        let systemv_cie = if matches!(info, CraneliftUnwindInfo::SystemV(_)) {
            Some(
                isa.create_systemv_cie()
                    .ok_or(CompileFailure::InvalidArtifact)?,
            )
        } else {
            None
        };
        #[cfg(not(all(
            any(target_os = "linux", target_os = "macos"),
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )))]
        let _ = isa;
        Ok(Self {
            info,
            #[cfg(all(
                any(target_os = "linux", target_os = "macos"),
                target_endian = "little",
                any(target_arch = "x86_64", target_arch = "aarch64")
            ))]
            systemv_cie,
            #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
            windows_function_len: None,
            #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
            windows_unwind_offset: None,
            #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
            windows_arm64_unwind_offset: None,
        })
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    fn prepare(mut self, bytes: &mut Vec<u8>) -> Result<Self, CodeMemoryError> {
        let CraneliftUnwindInfo::WindowsX64(info) = &self.info else {
            return Err(CodeMemoryError::UnwindRegistrationUnsupported);
        };
        let function_len = u32::try_from(bytes.len()).map_err(|_| CodeMemoryError::SizeOverflow)?;
        let aligned_len = bytes
            .len()
            .checked_add(3)
            .map(|len| len & !3)
            .ok_or(CodeMemoryError::SizeOverflow)?;
        bytes.resize(aligned_len, 0);
        let unwind_offset =
            u32::try_from(aligned_len).map_err(|_| CodeMemoryError::SizeOverflow)?;
        let unwind_end = aligned_len
            .checked_add(info.emit_size())
            .ok_or(CodeMemoryError::SizeOverflow)?;
        bytes.resize(unwind_end, 0);
        info.emit(&mut bytes[aligned_len..unwind_end]);
        self.windows_function_len = Some(function_len);
        self.windows_unwind_offset = Some(unwind_offset);
        Ok(self)
    }

    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    fn prepare(mut self, bytes: &mut Vec<u8>) -> Result<Self, CodeMemoryError> {
        let CraneliftUnwindInfo::WindowsArm64(info) = &self.info else {
            return Err(CodeMemoryError::UnwindRegistrationUnsupported);
        };
        let function_len = bytes.len();
        if function_len == 0 || function_len % 4 != 0 || function_len / 4 >= (1 << 18) {
            return Err(CodeMemoryError::SizeOverflow);
        }
        let aligned_len = function_len
            .checked_add(3)
            .map(|len| len & !3)
            .ok_or(CodeMemoryError::SizeOverflow)?;
        bytes.resize(aligned_len, 0);
        let unwind_offset =
            u32::try_from(aligned_len).map_err(|_| CodeMemoryError::SizeOverflow)?;

        // Cranelift emits the operation codes but not the terminating `end`.
        // Reserve one additional word pre-filled with `end` (0xe4), so the
        // first byte after Cranelift's sequence terminates it even when the
        // sequence itself ends on a word boundary.
        let code_words = usize::from(info.code_words())
            .checked_add(1)
            .ok_or(CodeMemoryError::SizeOverflow)?;
        if code_words > u8::MAX as usize {
            return Err(CodeMemoryError::SizeOverflow);
        }
        let extended = code_words > 31;
        let header_words: usize = if extended { 2 } else { 1 };
        let xdata_len = header_words
            .checked_add(code_words)
            .and_then(|words| words.checked_mul(4))
            .ok_or(CodeMemoryError::SizeOverflow)?;
        let xdata_end = aligned_len
            .checked_add(xdata_len)
            .ok_or(CodeMemoryError::SizeOverflow)?;
        bytes.resize(xdata_end, 0xe4);

        let function_instructions =
            u32::try_from(function_len / 4).map_err(|_| CodeMemoryError::SizeOverflow)?;
        // E=1 describes the single mirrored epilog at the end of the function;
        // its unwind sequence starts at byte index zero.
        let mut header = function_instructions | (1 << 21);
        if !extended {
            header |= u32::try_from(code_words).unwrap() << 27;
        }
        bytes[aligned_len..aligned_len + 4].copy_from_slice(&header.to_le_bytes());
        let codes_offset = aligned_len + header_words * 4;
        if extended {
            let extension = u32::try_from(code_words).unwrap() << 16;
            bytes[aligned_len + 4..aligned_len + 8].copy_from_slice(&extension.to_le_bytes());
        }
        info.emit(&mut bytes[codes_offset..xdata_end]);
        self.windows_arm64_unwind_offset = Some(unwind_offset);
        Ok(self)
    }

    #[cfg(not(all(
        target_os = "windows",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )))]
    fn prepare(self, _bytes: &mut Vec<u8>) -> Result<Self, CodeMemoryError> {
        Ok(self)
    }

    fn register(
        self,
        executable: &ExecutableCode,
    ) -> Result<NativeUnwindRegistration, CodeMemoryError> {
        #[cfg(all(
            any(target_os = "linux", target_os = "macos"),
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))]
        if let CraneliftUnwindInfo::SystemV(info) = self.info {
            use gimli::{
                write::{Address, EhFrame, EndianVec, FrameTable},
                LittleEndian,
            };

            let cie = self
                .systemv_cie
                .ok_or(CodeMemoryError::UnwindRegistrationUnsupported)?;
            let mut table = FrameTable::default();
            let cie = table.add_cie(cie);
            table.add_fde(
                cie,
                info.to_fde(Address::Constant(executable.as_ptr() as usize as u64)),
            );
            let mut eh_frame = EhFrame::from(EndianVec::new(LittleEndian));
            table.write_eh_frame(&mut eh_frame).map_err(|_| {
                CodeMemoryError::UnwindRegistrationFailed {
                    operation: "encode System V .eh_frame",
                }
            })?;
            let mut bytes = eh_frame.0.into_vec();
            bytes.extend_from_slice(&0_u32.to_ne_bytes());
            let bytes = bytes.into_boxed_slice();
            let registration_offset = systemv_registration_offset(&bytes)?;
            {
                let _registration_guard = systemv_registration_lock()
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                unsafe {
                    systemv_register_frame(bytes.as_ptr().add(registration_offset).cast());
                }
            }
            return Ok(NativeUnwindRegistration {
                systemv_eh_frame: Some(bytes),
                systemv_registration_offset: registration_offset,
            });
        }

        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        if matches!(&self.info, CraneliftUnwindInfo::WindowsX64(_)) {
            use windows_sys::Win32::System::Diagnostics::Debug::{
                RtlAddFunctionTable, IMAGE_RUNTIME_FUNCTION_ENTRY, IMAGE_RUNTIME_FUNCTION_ENTRY_0,
            };

            let mut function_table = Box::new(IMAGE_RUNTIME_FUNCTION_ENTRY {
                BeginAddress: 0,
                EndAddress: self
                    .windows_function_len
                    .ok_or(CodeMemoryError::UnwindRegistrationUnsupported)?,
                Anonymous: IMAGE_RUNTIME_FUNCTION_ENTRY_0 {
                    UnwindInfoAddress: self
                        .windows_unwind_offset
                        .ok_or(CodeMemoryError::UnwindRegistrationUnsupported)?,
                },
            });
            let registered = unsafe {
                RtlAddFunctionTable(
                    function_table.as_mut(),
                    1,
                    executable.as_ptr() as usize as u64,
                )
            };
            if !registered {
                return Err(CodeMemoryError::UnwindRegistrationFailed {
                    operation: "register Windows x64 function table",
                });
            }
            return Ok(NativeUnwindRegistration {
                systemv_eh_frame: None,
                systemv_registration_offset: 0,
                windows_function_table: Some(function_table),
            });
        }

        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        if matches!(&self.info, CraneliftUnwindInfo::WindowsArm64(_)) {
            use windows_sys::Win32::System::Diagnostics::Debug::{
                RtlAddFunctionTable, IMAGE_ARM64_RUNTIME_FUNCTION_ENTRY,
                IMAGE_ARM64_RUNTIME_FUNCTION_ENTRY_0,
            };

            let mut function_table = Box::new(IMAGE_ARM64_RUNTIME_FUNCTION_ENTRY {
                BeginAddress: 0,
                Anonymous: IMAGE_ARM64_RUNTIME_FUNCTION_ENTRY_0 {
                    UnwindData: self
                        .windows_arm64_unwind_offset
                        .ok_or(CodeMemoryError::UnwindRegistrationUnsupported)?,
                },
            });
            let registered = unsafe {
                RtlAddFunctionTable(function_table.as_mut(), 1, executable.as_ptr() as usize)
            };
            if !registered {
                return Err(CodeMemoryError::UnwindRegistrationFailed {
                    operation: "register Windows ARM64 function table",
                });
            }
            return Ok(NativeUnwindRegistration {
                systemv_eh_frame: None,
                systemv_registration_offset: 0,
                windows_arm64_function_table: Some(function_table),
            });
        }

        let _ = executable;
        Err(CodeMemoryError::UnwindRegistrationUnsupported)
    }
}

struct NativeUnwindRegistration {
    systemv_eh_frame: Option<Box<[u8]>>,
    systemv_registration_offset: usize,
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    windows_function_table:
        Option<Box<windows_sys::Win32::System::Diagnostics::Debug::IMAGE_RUNTIME_FUNCTION_ENTRY>>,
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    windows_arm64_function_table: Option<
        Box<windows_sys::Win32::System::Diagnostics::Debug::IMAGE_ARM64_RUNTIME_FUNCTION_ENTRY>,
    >,
}

impl std::fmt::Debug for NativeUnwindRegistration {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeUnwindRegistration")
            .field(
                "systemv_eh_frame_len",
                &self.systemv_eh_frame.as_ref().map(|bytes| bytes.len()),
            )
            .field(
                "systemv_registration_offset",
                &self.systemv_registration_offset,
            )
            .finish_non_exhaustive()
    }
}

impl Drop for NativeUnwindRegistration {
    fn drop(&mut self) {
        #[cfg(all(
            any(target_os = "linux", target_os = "macos"),
            target_endian = "little",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ))]
        if let Some(bytes) = self.systemv_eh_frame.as_ref() {
            let _registration_guard = systemv_registration_lock()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            unsafe {
                systemv_deregister_frame(
                    bytes.as_ptr().add(self.systemv_registration_offset).cast(),
                );
            }
        }

        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        if let Some(function_table) = self.windows_function_table.as_ref() {
            use windows_sys::Win32::System::Diagnostics::Debug::RtlDeleteFunctionTable;

            unsafe {
                let _ = RtlDeleteFunctionTable(function_table.as_ref());
            }
        }

        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        if let Some(function_table) = self.windows_arm64_function_table.as_ref() {
            use windows_sys::Win32::System::Diagnostics::Debug::RtlDeleteFunctionTable;

            unsafe {
                let _ = RtlDeleteFunctionTable(function_table.as_ref());
            }
        }
    }
}

#[cfg(all(
    any(target_os = "linux", target_os = "macos"),
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn systemv_registration_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    &LOCK
}

#[cfg(all(
    target_os = "linux",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn systemv_registration_offset(_eh_frame: &[u8]) -> Result<usize, CodeMemoryError> {
    // GCC's frame API consumes a complete, zero-terminated .eh_frame section.
    Ok(0)
}

#[cfg(all(
    target_os = "macos",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
fn systemv_registration_offset(eh_frame: &[u8]) -> Result<usize, CodeMemoryError> {
    // Apple's libunwind frame API consumes one FDE rather than the section's CIE.
    let cie_length = eh_frame
        .get(..4)
        .and_then(|bytes| bytes.try_into().ok())
        .map(u32::from_le_bytes)
        .and_then(|length| usize::try_from(length).ok())
        .ok_or(CodeMemoryError::UnwindRegistrationFailed {
            operation: "locate the System V FDE",
        })?;
    let fde_offset =
        4_usize
            .checked_add(cie_length)
            .ok_or(CodeMemoryError::UnwindRegistrationFailed {
                operation: "locate the System V FDE",
            })?;
    if fde_offset
        .checked_add(4)
        .is_none_or(|end| end > eh_frame.len())
    {
        return Err(CodeMemoryError::UnwindRegistrationFailed {
            operation: "locate the System V FDE",
        });
    }
    Ok(fde_offset)
}

#[cfg(all(
    target_os = "linux",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
#[link(name = "gcc_s")]
unsafe extern "C" {
    #[link_name = "__register_frame"]
    fn systemv_register_frame(begin: *const std::ffi::c_void);
    #[link_name = "__deregister_frame"]
    fn systemv_deregister_frame(begin: *const std::ffi::c_void);
}

#[cfg(all(
    target_os = "macos",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
unsafe extern "C" {
    #[link_name = "__register_frame"]
    fn systemv_register_frame(begin: *const std::ffi::c_void);
    #[link_name = "__deregister_frame"]
    fn systemv_deregister_frame(begin: *const std::ffi::c_void);
}

fn isa_is_host_compatible(target: &dyn TargetIsa) -> bool {
    if target.triple() != &Triple::host() {
        return false;
    }
    let Ok(builder) = cranelift_native::builder() else {
        return false;
    };
    let Ok(host) = builder.finish(settings::Flags::new(settings::builder())) else {
        return false;
    };
    if target.is_branch_protection_enabled() && !host.is_branch_protection_enabled() {
        return false;
    }
    let host_flags: BTreeMap<_, _> = host
        .isa_flags()
        .into_iter()
        .map(|value| (value.name, (value.as_bool(), value.value_string())))
        .collect();
    target.isa_flags().into_iter().all(|value| {
        let Some((host_bool, host_value)) = host_flags.get(value.name) else {
            return false;
        };
        match value.as_bool() {
            Some(false) => true,
            Some(true) => *host_bool == Some(true),
            None => value.value_string() == *host_value,
        }
    })
}

fn relocation_kind(kind: CraneliftReloc) -> RelocationKind {
    match kind {
        CraneliftReloc::Abs4 => RelocationKind::Abs4,
        CraneliftReloc::Abs8 => RelocationKind::Abs8,
        CraneliftReloc::X86PCRel4 => RelocationKind::X86PCRel4,
        CraneliftReloc::X86CallPCRel4 => RelocationKind::X86CallPCRel4,
        CraneliftReloc::X86CallPLTRel4 => RelocationKind::X86CallPLTRel4,
        CraneliftReloc::X86GOTPCRel4 => RelocationKind::X86GOTPCRel4,
        CraneliftReloc::X86SecRel => RelocationKind::X86SecRel,
        CraneliftReloc::Arm32Call => RelocationKind::Arm32Call,
        CraneliftReloc::Arm64Call => RelocationKind::Arm64Call,
        CraneliftReloc::S390xPCRel32Dbl => RelocationKind::S390xPCRel32Dbl,
        CraneliftReloc::S390xPLTRel32Dbl => RelocationKind::S390xPLTRel32Dbl,
        CraneliftReloc::ElfX86_64TlsGd => RelocationKind::ElfX86_64TlsGd,
        CraneliftReloc::MachOX86_64Tlv => RelocationKind::MachOX86_64Tlv,
        CraneliftReloc::MachOAarch64TlsAdrPage21 => RelocationKind::MachOAarch64TlsAdrPage21,
        CraneliftReloc::MachOAarch64TlsAdrPageOff12 => RelocationKind::MachOAarch64TlsAdrPageOff12,
        CraneliftReloc::Aarch64TlsDescAdrPage21 => RelocationKind::Aarch64TlsDescAdrPage21,
        CraneliftReloc::Aarch64TlsDescLd64Lo12 => RelocationKind::Aarch64TlsDescLd64Lo12,
        CraneliftReloc::Aarch64TlsDescAddLo12 => RelocationKind::Aarch64TlsDescAddLo12,
        CraneliftReloc::Aarch64TlsDescCall => RelocationKind::Aarch64TlsDescCall,
        CraneliftReloc::Aarch64AdrGotPage21 => RelocationKind::Aarch64AdrGotPage21,
        CraneliftReloc::Aarch64Ld64GotLo12Nc => RelocationKind::Aarch64Ld64GotLo12Nc,
        CraneliftReloc::RiscvCallPlt => RelocationKind::RiscvCallPlt,
        CraneliftReloc::RiscvTlsGdHi20 => RelocationKind::RiscvTlsGdHi20,
        CraneliftReloc::RiscvPCRelLo12I => RelocationKind::RiscvPCRelLo12I,
        CraneliftReloc::RiscvGotHi20 => RelocationKind::RiscvGotHi20,
        CraneliftReloc::S390xTlsGd64 => RelocationKind::S390xTlsGd64,
        CraneliftReloc::S390xTlsGdCall => RelocationKind::S390xTlsGdCall,
        CraneliftReloc::PulleyCallIndirectHost => RelocationKind::PulleyCallIndirectHost,
    }
}

fn frame_state_source_loc(state: FrameStateId) -> Result<SourceLoc, CompileFailure> {
    let bits = u32::try_from(state.index())
        .ok()
        .filter(|bits| *bits != u32::MAX)
        .ok_or(CompileFailure::ResourceLimit)?;
    Ok(SourceLoc::new(bits))
}

/// Source locations of every instruction in a block reachable from the CLIF
/// entry block. Cranelift's unreachable-code elimination uses the same
/// control flow graph, so a frame state whose location is absent here was
/// only lowered into code that is removed before emission.
fn reachable_source_locations(function: &Function) -> BTreeSet<u32> {
    let mut locations = BTreeSet::new();
    let Some(entry) = function.layout.entry_block() else {
        return locations;
    };
    let cfg = cranelift_codegen::flowgraph::ControlFlowGraph::with_function(function);
    let mut visited = BTreeSet::from([entry]);
    let mut worklist = vec![entry];
    while let Some(block) = worklist.pop() {
        for instruction in function.layout.block_insts(block) {
            let location = function.srcloc(instruction);
            if !location.is_default() {
                locations.insert(location.bits());
            }
        }
        for successor in cfg.succ_iter(block) {
            if visited.insert(successor) {
                worklist.push(successor);
            }
        }
    }
    locations
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum EntryRoot {
    Argument(u16),
    Local(u16),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum KnownKind {
    Number,
    Boolean,
    Null,
    Undefined,
    ShortBigInt,
    Uninitialized,
    Other,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RequiredDomain {
    Numeric,
    Initialized,
    Uninitialized,
}

impl KnownKind {
    fn satisfies(self, required: RequiredDomain) -> bool {
        match required {
            RequiredDomain::Numeric => self == Self::Number,
            RequiredDomain::Initialized => self != Self::Uninitialized,
            RequiredDomain::Uninitialized => self == Self::Uninitialized,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AbstractValue {
    roots: BTreeSet<EntryRoot>,
    known: BTreeSet<KnownKind>,
}

impl AbstractValue {
    fn unknown() -> Self {
        Self {
            roots: BTreeSet::new(),
            known: BTreeSet::new(),
        }
    }

    fn root(root: EntryRoot) -> Self {
        Self {
            roots: BTreeSet::from([root]),
            known: BTreeSet::new(),
        }
    }

    fn known(kind: KnownKind) -> Self {
        Self {
            roots: BTreeSet::new(),
            known: BTreeSet::from([kind]),
        }
    }

    fn from_tagged(value: TaggedValue) -> Self {
        let tag = value.tag;
        let kind = if tag == i64::from(qjs::JS_TAG_INT) || tag == i64::from(qjs::JS_TAG_FLOAT64) {
            KnownKind::Number
        } else if tag == i64::from(qjs::JS_TAG_BOOL) {
            KnownKind::Boolean
        } else if tag == i64::from(qjs::JS_TAG_NULL) {
            KnownKind::Null
        } else if tag == i64::from(qjs::JS_TAG_UNDEFINED) {
            KnownKind::Undefined
        } else if tag == i64::from(qjs::JS_TAG_SHORT_BIG_INT) {
            KnownKind::ShortBigInt
        } else if tag == i64::from(qjs::JS_TAG_UNINITIALIZED) {
            KnownKind::Uninitialized
        } else {
            KnownKind::Other
        };
        Self::known(kind)
    }

    fn merge_from(&mut self, other: &Self) -> bool {
        let old_roots = self.roots.len();
        let old_known = self.known.len();
        self.roots.extend(other.roots.iter().copied());
        self.known.extend(other.known.iter().copied());
        old_roots != self.roots.len() || old_known != self.known.len()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct RootRequirements {
    numeric: bool,
    initialized: bool,
    uninitialized: bool,
}

impl RootRequirements {
    fn add(&mut self, required: RequiredDomain) -> bool {
        match required {
            RequiredDomain::Numeric => {
                self.numeric = true;
                self.initialized = true;
            }
            RequiredDomain::Initialized => self.initialized = true,
            RequiredDomain::Uninitialized => self.uninitialized = true,
        }
        self.uninitialized && (self.numeric || self.initialized)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AbstractFrame {
    arguments: Vec<AbstractValue>,
    locals: Vec<AbstractValue>,
    stack: Vec<AbstractValue>,
}

impl AbstractFrame {
    fn merge_from(&mut self, other: &Self) -> Result<bool, CompileFailure> {
        if self.arguments.len() != other.arguments.len()
            || self.locals.len() != other.locals.len()
            || self.stack.len() != other.stack.len()
        {
            return Err(CompileFailure::InvalidArtifact);
        }
        let mut changed = false;
        for (current, incoming) in self
            .arguments
            .iter_mut()
            .zip(&other.arguments)
            .chain(self.locals.iter_mut().zip(&other.locals))
            .chain(self.stack.iter_mut().zip(&other.stack))
        {
            changed |= current.merge_from(incoming);
        }
        Ok(changed)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct EntryAnalysis {
    retry_before_entry: bool,
    requirements: BTreeMap<EntryRoot, RootRequirements>,
}

impl EntryAnalysis {
    fn require(&mut self, value: &AbstractValue, required: RequiredDomain) -> bool {
        if value
            .known
            .iter()
            .copied()
            .any(|kind| !kind.satisfies(required))
        {
            self.retry_before_entry = true;
            return false;
        }
        for root in &value.roots {
            if self.requirements.entry(*root).or_default().add(required) {
                self.retry_before_entry = true;
                return false;
            }
        }
        true
    }
}

fn analyze_entry_domains(ir: &BaselineIr) -> Result<EntryAnalysis, CompileFailure> {
    let block_indices: BTreeMap<_, _> = ir
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.start_pc, index))
        .collect();
    let Some(&entry_index) = block_indices.get(&0) else {
        return Err(CompileFailure::InvalidArtifact);
    };
    let entry = AbstractFrame {
        arguments: (0..ir.argument_count)
            .map(|index| AbstractValue::root(EntryRoot::Argument(index)))
            .collect(),
        locals: (0..ir.local_count)
            .map(|index| AbstractValue::root(EntryRoot::Local(index)))
            .collect(),
        stack: Vec::new(),
    };
    let mut block_inputs = vec![None; ir.blocks.len()];
    block_inputs[entry_index] = Some(entry);
    let mut queue = VecDeque::from([entry_index]);
    let mut queued = BTreeSet::from([entry_index]);
    let mut analysis = EntryAnalysis::default();

    while let Some(block_index) = queue.pop_front() {
        queued.remove(&block_index);
        let block = &ir.blocks[block_index];
        let mut frame = block_inputs[block_index]
            .clone()
            .ok_or(CompileFailure::InvalidArtifact)?;
        if frame.stack.len() != usize::from(block.stack_depth) {
            return Err(CompileFailure::InvalidArtifact);
        }
        let mut successors = Vec::new();
        let mut terminated = false;

        for instruction in &block.instructions {
            // Every helper edge of an instruction inside a try region may
            // enter its handler. The handler observes the frame's arguments
            // and locals as they are at that edge: before, after, or (for a
            // kept store) between the release of the old value and the store.
            let handler = (!matches!(
                instruction.op,
                IrOp::Poll { .. } | IrOp::OsrLabel { .. } | IrOp::Nop
            ))
            .then(|| ir.exception_handlers.get(&instruction.pc).copied())
            .flatten();
            if let Some(handler) = handler {
                successors.push(exceptional_input(&frame, handler)?);
                match instruction.op {
                    IrOp::PutLocal { index, keep: true } => {
                        let mut transient = frame.clone();
                        *transient
                            .locals
                            .get_mut(usize::from(index))
                            .ok_or(CompileFailure::InvalidArtifact)? =
                            AbstractValue::known(KnownKind::Undefined);
                        successors.push(exceptional_input(&transient, handler)?);
                    }
                    IrOp::PutArgument { index, keep: true } => {
                        let mut transient = frame.clone();
                        *transient
                            .arguments
                            .get_mut(usize::from(index))
                            .ok_or(CompileFailure::InvalidArtifact)? =
                            AbstractValue::known(KnownKind::Undefined);
                        successors.push(exceptional_input(&transient, handler)?);
                    }
                    _ => {}
                }
            }
            match &instruction.op {
                IrOp::Poll { .. } | IrOp::OsrLabel { state: _ } | IrOp::Nop => {}
                IrOp::Push(value) => frame.stack.push(AbstractValue::from_tagged(*value)),
                IrOp::Catch(_) => frame.stack.push(AbstractValue::known(KnownKind::Other)),
                IrOp::NipCatch => {
                    let value = frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.push(value);
                }
                IrOp::Throw => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    terminated = true;
                }
                IrOp::ThrowError { .. } => terminated = true,
                IrOp::Gosub { target, .. } => {
                    frame.stack.push(AbstractValue::known(KnownKind::Number));
                    successors.push((*target, frame.clone()));
                    terminated = true;
                }
                IrOp::Ret { targets } => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    for target in targets.iter() {
                        successors.push((*target, frame.clone()));
                    }
                    terminated = true;
                }
                // Constant-pool descriptors are pointer-free but intentionally
                // do not carry the full runtime value. Keep their abstract
                // domain unknown: the lowering performs the exact tag guard
                // after ResolveConst. Classifying every constant as `Other`
                // made valid Float64 constants force an unconditional entry
                // retry, so nominal native entries never executed code.
                IrOp::ResolveConstant(_) | IrOp::ResolveAtom(_) => {
                    frame.stack.push(AbstractValue::unknown());
                }
                IrOp::GetGlobal(_) => {
                    frame.stack.push(AbstractValue::unknown());
                }
                IrOp::FClosure(_) => {
                    frame.stack.push(AbstractValue::known(KnownKind::Other));
                }
                // Closure variables may hold any value, including the TDZ
                // sentinel for the unchecked form; uses guard at run time.
                IrOp::GetVarRef { .. } => {
                    frame.stack.push(AbstractValue::unknown());
                }
                IrOp::PutVarRef { keep, .. } => {
                    if *keep {
                        frame.stack.last().ok_or(CompileFailure::InvalidArtifact)?;
                    } else {
                        frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    }
                }
                IrOp::SetName(_) => {
                    frame.stack.last().ok_or(CompileFailure::InvalidArtifact)?;
                }
                IrOp::CloseLocal(index) => {
                    frame
                        .locals
                        .get(usize::from(*index))
                        .ok_or(CompileFailure::InvalidArtifact)?;
                }
                IrOp::NewObject => {
                    frame.stack.push(AbstractValue::known(KnownKind::Other));
                }
                IrOp::NewArrayFrom(count) => {
                    let new_len = frame
                        .stack
                        .len()
                        .checked_sub(usize::from(*count))
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.truncate(new_len);
                    frame.stack.push(AbstractValue::known(KnownKind::Other));
                }
                // Property loads and call results carry any value. Their
                // domain stays unknown so numeric uses are guarded at the use
                // site (`deopt_unless_numeric`) instead of forcing the whole
                // function to retry before entry.
                IrOp::GetProperty(_) => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.push(AbstractValue::unknown());
                }
                IrOp::GetPropertyKeep(_) => {
                    frame.stack.last().ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.push(AbstractValue::unknown());
                }
                IrOp::SetProperty(_) => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                }
                IrOp::DefineProperty(_) => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.last().ok_or(CompileFailure::InvalidArtifact)?;
                }
                IrOp::GetElement => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.push(AbstractValue::unknown());
                }
                IrOp::SetElement => {
                    for _ in 0..3 {
                        frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    }
                }
                IrOp::DefineElement => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    if frame.stack.len() < 2 {
                        return Err(CompileFailure::InvalidArtifact);
                    }
                }
                IrOp::ToPropertyKey => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.push(AbstractValue::unknown());
                }
                IrOp::Call { argc, has_this } => {
                    let pop = usize::from(*argc) + 1 + usize::from(*has_this);
                    let new_len = frame
                        .stack
                        .len()
                        .checked_sub(pop)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.truncate(new_len);
                    frame.stack.push(AbstractValue::unknown());
                }
                IrOp::CallConstructor(argc) => {
                    let pop = usize::from(*argc) + 2;
                    let new_len = frame
                        .stack
                        .len()
                        .checked_sub(pop)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.truncate(new_len);
                    frame.stack.push(AbstractValue::known(KnownKind::Other));
                }
                IrOp::Regexp => {
                    let new_len = frame
                        .stack
                        .len()
                        .checked_sub(2)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    frame.stack.truncate(new_len);
                    frame.stack.push(AbstractValue::known(KnownKind::Other));
                }
                IrOp::Generic(operation) => {
                    let (pop, push) = operation.stack_effect();
                    let start = frame
                        .stack
                        .len()
                        .checked_sub(pop)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    if *operation == GenericOp::ToPropertyKey2 {
                        // The object stays in place; only the key changes.
                        frame.stack.truncate(start + 1);
                        frame.stack.push(AbstractValue::unknown());
                    } else {
                        frame.stack.truncate(start);
                        for _ in 0..push {
                            frame.stack.push(if operation.produces_boolean() {
                                AbstractValue::known(KnownKind::Boolean)
                            } else if *operation == GenericOp::ToObject {
                                AbstractValue::known(KnownKind::Other)
                            } else {
                                AbstractValue::unknown()
                            });
                        }
                    }
                }
                IrOp::Iterator(operation) => {
                    let base = frame
                        .stack
                        .len()
                        .checked_sub(operation.window_below())
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    match operation {
                        IteratorOp::ForOfStart => {
                            frame.stack.truncate(base);
                            frame.stack.extend([
                                AbstractValue::unknown(),
                                AbstractValue::unknown(),
                                AbstractValue::unknown(),
                            ]);
                        }
                        IteratorOp::ForInStart => {
                            frame.stack.truncate(base);
                            frame.stack.push(AbstractValue::unknown());
                        }
                        IteratorOp::ForOfNext(_) | IteratorOp::ForInNext => {
                            // Completion replaces the iterator with undefined.
                            frame.stack[base] = AbstractValue::unknown();
                            frame.stack.push(AbstractValue::unknown());
                            frame.stack.push(AbstractValue::known(KnownKind::Boolean));
                        }
                        IteratorOp::Close => frame.stack.truncate(base),
                    }
                }
                IrOp::GetArgument(index) => frame.stack.push(
                    frame
                        .arguments
                        .get(usize::from(*index))
                        .cloned()
                        .ok_or(CompileFailure::InvalidArtifact)?,
                ),
                IrOp::GetLocal(index) => frame.stack.push(
                    frame
                        .locals
                        .get(usize::from(*index))
                        .cloned()
                        .ok_or(CompileFailure::InvalidArtifact)?,
                ),
                IrOp::GetLocalChecked(index) => {
                    let value = frame
                        .locals
                        .get(usize::from(*index))
                        .cloned()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    if !analysis.require(&value, RequiredDomain::Initialized) {
                        return Ok(analysis);
                    }
                    frame.stack.push(value);
                }
                IrOp::GetLocalPair => {
                    frame.stack.push(
                        frame
                            .locals
                            .first()
                            .cloned()
                            .ok_or(CompileFailure::InvalidArtifact)?,
                    );
                    frame.stack.push(
                        frame
                            .locals
                            .get(1)
                            .cloned()
                            .ok_or(CompileFailure::InvalidArtifact)?,
                    );
                }
                IrOp::PutArgument { index, keep } => {
                    let source = frame
                        .stack
                        .last()
                        .cloned()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    *frame
                        .arguments
                        .get_mut(usize::from(*index))
                        .ok_or(CompileFailure::InvalidArtifact)? = source;
                    if !keep {
                        frame.stack.pop();
                    }
                }
                IrOp::PutLocal { index, keep } => {
                    let source = frame
                        .stack
                        .last()
                        .cloned()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    *frame
                        .locals
                        .get_mut(usize::from(*index))
                        .ok_or(CompileFailure::InvalidArtifact)? = source;
                    if !keep {
                        frame.stack.pop();
                    }
                }
                IrOp::PutLocalChecked { index, initialize } => {
                    let current = frame
                        .locals
                        .get(usize::from(*index))
                        .cloned()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let required = if *initialize {
                        RequiredDomain::Uninitialized
                    } else {
                        RequiredDomain::Initialized
                    };
                    if !analysis.require(&current, required) {
                        return Ok(analysis);
                    }
                    let source = frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    frame.locals[usize::from(*index)] = source;
                }
                IrOp::SetLocalUninitialized(index) => {
                    *frame
                        .locals
                        .get_mut(usize::from(*index))
                        .ok_or(CompileFailure::InvalidArtifact)? =
                        AbstractValue::known(KnownKind::Uninitialized);
                }
                IrOp::Drop => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                }
                IrOp::Stack(operation) => {
                    apply_abstract_stack_operation(&mut frame.stack, *operation)?;
                }
                IrOp::Unary(operation) => {
                    let value = frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    let result = analyze_unary(&mut analysis, value, *operation, true)?;
                    if analysis.retry_before_entry {
                        return Ok(analysis);
                    }
                    frame.stack.push(result);
                }
                IrOp::PostUnary(operation) => {
                    let value = frame
                        .stack
                        .last()
                        .cloned()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let result = analyze_unary(&mut analysis, value, *operation, false)?;
                    if analysis.retry_before_entry {
                        return Ok(analysis);
                    }
                    frame.stack.push(result);
                }
                IrOp::LocalUnary { index, op } => {
                    let value = frame
                        .locals
                        .get(usize::from(*index))
                        .cloned()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let result = analyze_unary(&mut analysis, value, *op, false)?;
                    if analysis.retry_before_entry {
                        return Ok(analysis);
                    }
                    frame.locals[usize::from(*index)] = result;
                }
                IrOp::AddLocal(index) => {
                    let right = frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    let left = frame
                        .locals
                        .get(usize::from(*index))
                        .cloned()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    if !analysis.require(&left, RequiredDomain::Numeric)
                        || !analysis.require(&right, RequiredDomain::Numeric)
                    {
                        return Ok(analysis);
                    }
                    frame.locals[usize::from(*index)] = AbstractValue::known(KnownKind::Number);
                }
                IrOp::Binary(operation) => {
                    let right = frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    let left = frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    let helper = matches!(
                        operation,
                        BinaryOp::Add
                            | BinaryOp::Mod
                            | BinaryOp::LessThan
                            | BinaryOp::LessThanOrEqual
                            | BinaryOp::GreaterThan
                            | BinaryOp::GreaterThanOrEqual
                            | BinaryOp::Equal
                            | BinaryOp::NotEqual
                            | BinaryOp::StrictEqual
                            | BinaryOp::StrictNotEqual
                    );
                    if !helper
                        && (!analysis.require(&left, RequiredDomain::Numeric)
                            || !analysis.require(&right, RequiredDomain::Numeric))
                    {
                        return Ok(analysis);
                    }
                    frame.stack.push(AbstractValue::known(
                        if binary_returns_boolean(*operation) {
                            KnownKind::Boolean
                        } else {
                            KnownKind::Number
                        },
                    ));
                }
                IrOp::Jump(target) => {
                    successors.push((*target, frame.clone()));
                    terminated = true;
                }
                IrOp::Branch {
                    target,
                    when_true: _,
                } => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    let fallthrough = ir
                        .blocks
                        .get(block_index + 1)
                        .ok_or(CompileFailure::InvalidArtifact)?
                        .start_pc;
                    successors.push((*target, frame.clone()));
                    successors.push((fallthrough, frame.clone()));
                    terminated = true;
                }
                IrOp::Return => {
                    frame.stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    terminated = true;
                }
                IrOp::ReturnUndefined => terminated = true,
            }
            if let Some(handler) = handler {
                if frame.stack.len() >= usize::from(handler.catch_index) {
                    successors.push(exceptional_input(&frame, handler)?);
                }
            }
            if terminated {
                break;
            }
        }

        if !terminated {
            let next = ir
                .blocks
                .get(block_index + 1)
                .ok_or(CompileFailure::InvalidArtifact)?;
            successors.push((next.start_pc, frame));
        }

        for (successor_pc, incoming) in successors {
            let successor_index = *block_indices
                .get(&successor_pc)
                .ok_or(CompileFailure::InvalidArtifact)?;
            if incoming.stack.len() != usize::from(ir.blocks[successor_index].stack_depth) {
                return Err(CompileFailure::InvalidArtifact);
            }
            let changed = if let Some(existing) = &mut block_inputs[successor_index] {
                existing.merge_from(&incoming)?
            } else {
                block_inputs[successor_index] = Some(incoming);
                true
            };
            if changed && queued.insert(successor_index) {
                queue.push_back(successor_index);
            }
        }
    }

    Ok(analysis)
}

/// The abstract frame a handler block is entered with from one exceptional
/// edge: operands below the catch offset, then the caught (unknown) value.
fn exceptional_input(
    frame: &AbstractFrame,
    handler: crate::ir::IrExceptionHandler,
) -> Result<(u32, AbstractFrame), CompileFailure> {
    let catch_index = usize::from(handler.catch_index);
    let mut stack = frame
        .stack
        .get(..catch_index)
        .ok_or(CompileFailure::InvalidArtifact)?
        .to_vec();
    stack.push(AbstractValue::unknown());
    Ok((
        handler.handler_pc,
        AbstractFrame {
            arguments: frame.arguments.clone(),
            locals: frame.locals.clone(),
            stack,
        },
    ))
}

/// `helper_backed` is true for the plain `Unary` form, whose non-numeric
/// operands take the exact `UNARY_ARITH_SLOW` helper path instead of
/// requiring a numeric entry proof. The post-increment and local forms
/// keep the entry requirement because they are lowered natively only.
fn analyze_unary(
    analysis: &mut EntryAnalysis,
    value: AbstractValue,
    operation: UnaryOp,
    helper_backed: bool,
) -> Result<AbstractValue, CompileFailure> {
    let (required, result) = match operation {
        UnaryOp::LogicalNot
        | UnaryOp::IsUndefinedOrNull
        | UnaryOp::IsUndefined
        | UnaryOp::IsNull => {
            return Ok(AbstractValue::known(KnownKind::Boolean));
        }
        UnaryOp::Plus => return Ok(AbstractValue::known(KnownKind::Other)),
        UnaryOp::Neg | UnaryOp::Increment | UnaryOp::Decrement | UnaryOp::BitNot => {
            if helper_backed {
                return Ok(AbstractValue::known(KnownKind::Number));
            }
            (RequiredDomain::Numeric, KnownKind::Number)
        }
    };
    let _ = analysis.require(&value, required);
    Ok(AbstractValue::known(result))
}

/// Authoritative QuickJS opcode id passed to the arithmetic slow-path helpers.
fn quickjs_opcode_id(name: &str) -> Result<u32, CompileFailure> {
    crate::bytecode::linked_opcode_table()
        .find(|opcode| opcode.name() == name)
        .map(|opcode| u32::from(opcode.id()))
        .ok_or(CompileFailure::InvalidArtifact)
}

fn binary_returns_boolean(operation: BinaryOp) -> bool {
    matches!(
        operation,
        BinaryOp::LessThan
            | BinaryOp::LessThanOrEqual
            | BinaryOp::GreaterThan
            | BinaryOp::GreaterThanOrEqual
            | BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::StrictEqual
            | BinaryOp::StrictNotEqual
    )
}

fn ir_op_produces_boolean(operation: &IrOp) -> bool {
    match operation {
        IrOp::Binary(operation) => binary_returns_boolean(*operation),
        IrOp::Generic(operation) => operation.produces_boolean(),
        // `js_for_of_next`/`js_for_in_next` and the Array values leaf always
        // leave an exact JS boolean `done` flag on the stack top.
        IrOp::Iterator(IteratorOp::ForOfNext(_) | IteratorOp::ForInNext) => true,
        _ => false,
    }
}

fn apply_abstract_stack_operation(
    stack: &mut Vec<AbstractValue>,
    operation: StackOp,
) -> Result<(), CompileFailure> {
    let (take, order): (usize, &[usize]) = stack_operation_order(operation);
    let start = stack
        .len()
        .checked_sub(take)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let values = stack[start..].to_vec();
    stack.truncate(start);
    stack.extend(order.iter().map(|source| values[*source].clone()));
    Ok(())
}

#[allow(clippy::too_many_arguments)] // Lowering state is passed explicitly to keep it borrow-scoped.
fn lower_function(
    builder: &mut FunctionBuilder<'_>,
    ir: &BaselineIr,
    isa: &dyn TargetIsa,
    layout: FrameLayout,
    element_layout: crate::abi::ElementLayout,
    analysis: &EntryAnalysis,
    osr_start: Option<u32>,
    _guard_exit: GuardExit,
    direct_calls: &[BaselineDirectCallSite],
    properties: &[BaselinePropertySite],
    object_fast: Option<ObjectFastPaths>,
) -> Result<(), CompileFailure> {
    let pointer_type = isa.pointer_type();
    let blocks: BTreeMap<u32, Block> = ir
        .blocks
        .iter()
        .map(|block| (block.start_pc, builder.create_block()))
        .collect();
    let retry = builder.create_block();
    let prologue = builder.create_block();
    let osr_post_poll = osr_start.map(|_| builder.create_block());
    let entry_pc = osr_start.unwrap_or(0);
    let entry = *blocks
        .get(&entry_pc)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let entry_depth = ir
        .blocks
        .iter()
        .find(|block| block.start_pc == entry_pc)
        .map(|block| usize::from(block.stack_depth))
        .ok_or(CompileFailure::InvalidArtifact)?;
    builder.append_block_params_for_function_params(prologue);
    builder.switch_to_block(prologue);
    let params = builder.block_params(prologue);
    if params.len() != 2 {
        return Err(CompileFailure::InvalidArtifact);
    }
    let sret = params[0];
    let frame = params[1];

    if analysis.retry_before_entry {
        emit_exit(
            builder,
            sret,
            qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER,
            None,
            pointer_type,
        );
        return Ok(());
    }
    let invariant_trap = builder.create_block();

    let mut next_variable = 0_u32;
    let mut allocate_pair = || {
        let pair = PairVars {
            payload: Variable::from_u32(next_variable),
            tag: Variable::from_u32(next_variable + 1),
        };
        next_variable += 2;
        pair
    };
    let arguments: Vec<PairVars> = (0..ir.argument_count).map(|_| allocate_pair()).collect();
    let locals: Vec<PairVars> = (0..ir.local_count).map(|_| allocate_pair()).collect();
    let stack_var_count = usize::from(ir.max_stack_depth)
        .checked_add(MAX_HELPER_SCRATCH_SLOTS)
        .ok_or(CompileFailure::ResourceLimit)?;
    let stack: Vec<PairVars> = (0..stack_var_count).map(|_| allocate_pair()).collect();
    let loop_poll_budget = Variable::from_u32(next_variable);
    for pair in arguments.iter().chain(&locals).chain(&stack) {
        builder.declare_var(pair.payload, types::I64);
        builder.declare_var(pair.tag, types::I64);
    }
    builder.declare_var(loop_poll_budget, types::I32);
    let initial_loop_poll_budget = builder.ins().iconst(types::I32, LOOP_POLL_INTERVAL);
    builder.def_var(loop_poll_budget, initial_loop_poll_budget);
    let flags = MemFlags::new();
    let arg_buf = builder
        .ins()
        .load(pointer_type, flags, frame, layout.arg_buf);
    let var_buf = builder
        .ins()
        .load(pointer_type, flags, frame, layout.var_buf);
    let stack_base = builder
        .ins()
        .load(pointer_type, flags, frame, layout.stack_base);
    // Runtime helper tables are immutable for the lifetime of an execution
    // frame.  Hoist the poll target out of the backedge slow path so periodic
    // safepoints do not pay two dependent pointer loads each time.
    let runtime_api = builder
        .ins()
        .load(pointer_type, flags, frame, layout.runtime_api);
    let poll_helper = builder.ins().load(
        pointer_type,
        flags,
        runtime_api,
        layout.helper_offsets[qjs::JSJitHelperId_JS_JIT_HELPER_POLL as usize],
    );
    for (index, pair) in arguments.iter().copied().enumerate() {
        let value = load_jsvalue(builder, arg_buf, index, layout);
        define_pair(builder, pair, value);
    }
    for (index, pair) in locals.iter().copied().enumerate() {
        let value = load_jsvalue(builder, var_buf, index, layout);
        define_pair(builder, pair, value);
    }
    emit_entry_domain_guards(builder, &arguments, &locals, analysis, retry);
    validate_live_stack_bounds(
        builder,
        frame,
        retry,
        pointer_type,
        layout,
        ir.max_stack_depth,
    );
    for (index, pair) in stack.iter().copied().enumerate() {
        let value = if osr_start.is_some() && index < entry_depth {
            load_jsvalue(builder, stack_base, index, layout)
        } else {
            constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64))
        };
        define_pair(builder, pair, value);
    }

    let helper_signatures = generated_signatures(isa)?
        .into_iter()
        .map(|signature| builder.import_signature(signature))
        .collect::<Vec<_>>();
    // Continuing-path Array values leaf. Absent (or an invalid table) means
    // every `for_of_next` takes the exact ITERATOR_OP helper.
    let array_values_next = if ir.blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .any(|instruction| matches!(instruction.op, IrOp::Iterator(IteratorOp::ForOfNext(_))))
    }) {
        crate::abi::AbiInfo::linked()
            .ok()
            .and_then(|abi| abi.iterator_api().array_values_next)
            .map(|leaf| leaf as usize)
    } else {
        None
    };
    let helper_lowering = HelperLowering {
        ir,
        frame,
        runtime_api,
        sret,
        arg_buf,
        var_buf,
        stack_base,
        arguments: &arguments,
        locals: &locals,
        stack: &stack,
        signatures: &helper_signatures,
        pointer_type,
        layout,
        reload_captured_slots: creates_closures(ir),
        exception_target: std::cell::Cell::new(None),
        element_layout,
        object_fast,
    };
    // One landing pad per handler: every exceptional edge of its try region
    // publishes the exact owned stack and enters it.
    let landing_blocks: BTreeMap<u32, Block> = ir
        .catch_sites
        .keys()
        .map(|handler_pc| (*handler_pc, builder.create_block()))
        .collect();
    let fixed_slots = usize::from(ir.argument_count) + usize::from(ir.local_count);

    macro_rules! invoke_helper {
        ($helper_id:expr, $state:expr, $live_depth:expr, $arguments:expr) => {{
            helper_lowering.invoke(
                builder,
                $helper_id,
                $state,
                $live_depth,
                $live_depth,
                $arguments,
            )?;
        }};
    }

    builder.ins().jump(osr_post_poll.unwrap_or(entry), &[]);
    builder.switch_to_block(entry);

    for (block_index, block) in ir.blocks.iter().enumerate() {
        let clif_block = blocks[&block.start_pc];
        if clif_block != entry || osr_start.is_some() {
            builder.switch_to_block(clif_block);
        }
        let mut depth = block.stack_depth as usize;
        let mut terminated = false;
        let mut entered_osr_continuation = false;
        let mut previous_effectful_op_was_boolean = false;
        let mut fused: Option<FusedRegion> = None;
        for (instruction_index, instruction) in block.instructions.iter().enumerate() {
            if fused.is_none() {
                if let Some(fast) = object_fast {
                    fused = emit_fused_fast_path(
                        builder,
                        &helper_lowering,
                        fast,
                        &block.instructions,
                        instruction_index,
                        depth,
                    )?;
                }
            }
            let mut helper_states = instruction.helper_states.iter().copied();
            let prior_op_was_boolean = previous_effectful_op_was_boolean;
            if !matches!(
                instruction.op,
                IrOp::Poll { .. } | IrOp::Nop | IrOp::OsrLabel { .. }
            ) {
                previous_effectful_op_was_boolean = ir_op_produces_boolean(&instruction.op);
            }
            let exception_target = match ir.exception_handlers.get(&instruction.pc) {
                Some(handler) => Some(
                    *landing_blocks
                        .get(&handler.handler_pc)
                        .ok_or(CompileFailure::InvalidArtifact)?,
                ),
                None => None,
            };
            helper_lowering.exception_target.set(exception_target);
            builder.set_srcloc(SourceLoc::default());
            if !matches!(&instruction.op, IrOp::Poll { .. }) {
                if let Some(state) = instruction.frame_state {
                    emit_frame_state_marker(builder, frame, invariant_trap, state)?;
                }
            }
            match instruction.op {
                IrOp::Poll { state, kind } => {
                    let loop_continuation = if kind == PollKind::LoopHeader {
                        let remaining = builder.use_var(loop_poll_budget);
                        let remaining = builder.ins().iadd_imm(remaining, -1);
                        builder.def_var(loop_poll_budget, remaining);
                        let due = builder.ins().icmp_imm(IntCC::Equal, remaining, 0);
                        let slow = builder.create_block();
                        let continuation = builder.create_block();
                        builder.ins().brif(due, slow, &[], continuation, &[]);
                        builder.seal_block(slow);
                        builder.switch_to_block(slow);
                        let reset = builder.ins().iconst(types::I32, LOOP_POLL_INTERVAL);
                        builder.def_var(loop_poll_budget, reset);
                        Some(continuation)
                    } else {
                        None
                    };
                    materialize_frame(
                        builder,
                        frame,
                        arg_buf,
                        var_buf,
                        stack_base,
                        &arguments,
                        &locals,
                        &stack,
                        depth,
                        depth,
                        ir.frame_states.get(state).pc,
                        pointer_type,
                        layout,
                    )?;
                    emit_poll(
                        builder,
                        frame,
                        sret,
                        poll_helper,
                        helper_signatures[qjs::JSJitHelperId_JS_JIT_HELPER_POLL as usize],
                        PollLocation {
                            bytecode_pc: instruction.pc,
                            source_location: frame_state_source_loc(state)?,
                        },
                        pointer_type,
                        layout,
                    );
                    helper_lowering.reload_captured_slots(builder);
                    if let Some(continuation) = loop_continuation {
                        builder.ins().jump(continuation, &[]);
                        builder.seal_block(continuation);
                        builder.switch_to_block(continuation);
                    }
                    if osr_start == Some(block.start_pc) && !entered_osr_continuation {
                        let continuation = osr_post_poll.ok_or(CompileFailure::InvalidArtifact)?;
                        builder.ins().jump(continuation, &[]);
                        builder.switch_to_block(continuation);
                        entered_osr_continuation = true;
                    }
                }
                IrOp::OsrLabel { .. } => {}
                IrOp::Nop => {}
                IrOp::Push(value) => {
                    let value = constant_pair(builder, value);
                    define_pair(builder, stack[depth], value);
                    depth += 1;
                }
                IrOp::ResolveConstant(index) => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let output = flat_stack_slot(ir, depth)?;
                    invoke_helper!(
                        qjs::JSJitHelperId_JS_JIT_HELPER_RESOLVE_CONST,
                        state,
                        depth,
                        &[output, index]
                    );
                    reload_pair(builder, stack[depth], stack_base, depth, layout);
                    depth += 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::ResolveAtom(atom) => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let output = flat_stack_slot(ir, depth)?;
                    invoke_helper!(
                        qjs::JSJitHelperId_JS_JIT_HELPER_ATOM_VALUE,
                        state,
                        depth,
                        &[output, atom]
                    );
                    reload_pair(builder, stack[depth], stack_base, depth, layout);
                    depth += 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::GetGlobal(atom) => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let output = flat_stack_slot(ir, depth)?;
                    invoke_helper!(
                        qjs::JSJitHelperId_JS_JIT_HELPER_GET_GLOBAL,
                        state,
                        depth,
                        &[output, atom]
                    );
                    reload_pair(builder, stack[depth], stack_base, depth, layout);
                    depth += 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::NewObject => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let output = flat_stack_slot(ir, depth)?;
                    invoke_helper!(
                        qjs::JSJitHelperId_JS_JIT_HELPER_NEW_OBJECT,
                        state,
                        depth,
                        &[output]
                    );
                    reload_pair(builder, stack[depth], stack_base, depth, layout);
                    depth += 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::NewArrayFrom(count) => lower_new_array(
                    builder,
                    &helper_lowering,
                    &mut helper_states,
                    &mut depth,
                    count,
                )?,
                IrOp::GetProperty(atom) => lower_get_property(
                    builder,
                    &helper_lowering,
                    &mut helper_states,
                    depth,
                    atom,
                    properties
                        .iter()
                        .find(|site| site.pc == instruction.pc && !site.store),
                )?,
                IrOp::GetPropertyKeep(atom) => lower_get_property_keep(
                    builder,
                    &helper_lowering,
                    &mut helper_states,
                    &mut depth,
                    atom,
                )?,
                IrOp::SetProperty(atom) => lower_set_property(
                    builder,
                    &helper_lowering,
                    &mut helper_states,
                    &mut depth,
                    atom,
                    properties
                        .iter()
                        .find(|site| site.pc == instruction.pc && site.store),
                )?,
                IrOp::DefineProperty(atom) => lower_define_property(
                    builder,
                    &helper_lowering,
                    &mut helper_states,
                    &mut depth,
                    atom,
                )?,
                IrOp::GetElement => lower_get_element(
                    builder,
                    &helper_lowering,
                    &mut helper_states,
                    &mut depth,
                    element_layout,
                )?,
                IrOp::SetElement => lower_set_element(
                    builder,
                    &helper_lowering,
                    &mut helper_states,
                    &mut depth,
                    element_layout,
                )?,
                IrOp::DefineElement => {
                    lower_define_element(builder, &helper_lowering, &mut helper_states, &mut depth)?
                }
                IrOp::ToPropertyKey => {
                    lower_to_property_key(builder, &helper_lowering, &mut helper_states, depth)?
                }
                IrOp::Call { argc, has_this } => lower_call(
                    builder,
                    &helper_lowering,
                    &mut helper_states,
                    &mut depth,
                    argc,
                    has_this,
                    false,
                    direct_calls
                        .iter()
                        .find(|target| target.pc == instruction.pc),
                )?,
                IrOp::CallConstructor(argc) => lower_call(
                    builder,
                    &helper_lowering,
                    &mut helper_states,
                    &mut depth,
                    argc,
                    true,
                    true,
                    None,
                )?,
                IrOp::Regexp => {
                    let pattern_index = depth
                        .checked_sub(2)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let bytecode_index = depth - 1;
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let pattern = flat_stack_slot(ir, pattern_index)?;
                    let bytecode = flat_stack_slot(ir, bytecode_index)?;
                    invoke_helper!(
                        qjs::JSJitHelperId_JS_JIT_HELPER_REGEXP,
                        state,
                        depth,
                        &[pattern, pattern, bytecode]
                    );
                    reload_pair(
                        builder,
                        stack[pattern_index],
                        stack_base,
                        pattern_index,
                        layout,
                    );
                    clear_pair(
                        builder,
                        stack[bytecode_index],
                        stack_base,
                        bytecode_index,
                        layout,
                    )?;
                    depth = pattern_index + 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::FClosure(index) => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let output = flat_stack_slot(ir, depth)?;
                    invoke_helper!(
                        qjs::JSJitHelperId_JS_JIT_HELPER_FCLOSURE,
                        state,
                        depth,
                        &[output, index]
                    );
                    reload_pair(builder, stack[depth], stack_base, depth, layout);
                    depth += 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::GetVarRef { index, checked } => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let output = flat_stack_slot(ir, depth)?;
                    let mode = if checked {
                        qjs::JSJitVarRefMode_JS_JIT_VAR_REF_CHECK
                    } else {
                        qjs::JSJitVarRefMode_JS_JIT_VAR_REF_PLAIN
                    };
                    invoke_helper!(
                        qjs::JSJitHelperId_JS_JIT_HELPER_GET_VAR_REF,
                        state,
                        depth,
                        &[output, u32::from(index), mode]
                    );
                    reload_pair(builder, stack[depth], stack_base, depth, layout);
                    depth += 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::PutVarRef { index, mode, keep } => {
                    let top = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let mode = match mode {
                        VarRefMode::Plain => qjs::JSJitVarRefMode_JS_JIT_VAR_REF_PLAIN,
                        VarRefMode::Check => qjs::JSJitVarRefMode_JS_JIT_VAR_REF_CHECK,
                        VarRefMode::CheckInit => qjs::JSJitVarRefMode_JS_JIT_VAR_REF_CHECK_INIT,
                    };
                    let value_index = if keep {
                        // `set_var_ref*` keeps the value: store an owned
                        // duplicate from the scratch slot above the stack.
                        let dup_state = helper_states
                            .next()
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let source = use_pair(builder, stack[top]);
                        lower_dup_if_refcounted(
                            builder,
                            &helper_lowering,
                            dup_state,
                            depth,
                            depth,
                            source,
                            flat_stack_slot(ir, top)?,
                            false,
                            false,
                        )?;
                        depth
                    } else {
                        top
                    };
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let value = flat_stack_slot(ir, value_index)?;
                    // The helper consumes the value slot on success and on a
                    // TDZ exception. The exception exit publishes only the
                    // bytecode-visible stack below the consumed operand.
                    helper_lowering.invoke(
                        builder,
                        qjs::JSJitHelperId_JS_JIT_HELPER_PUT_VAR_REF,
                        state,
                        value_index + 1,
                        value_index,
                        &[value, u32::from(index), mode],
                    )?;
                    clear_pair(builder, stack[value_index], stack_base, value_index, layout)?;
                    if !keep {
                        depth = top;
                    }
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::SetName(atom) => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let object = flat_stack_slot(
                        ir,
                        depth
                            .checked_sub(1)
                            .ok_or(CompileFailure::InvalidArtifact)?,
                    )?;
                    // The named object stays borrowed in its stack slot.
                    invoke_helper!(
                        qjs::JSJitHelperId_JS_JIT_HELPER_SET_NAME,
                        state,
                        depth,
                        &[object, atom]
                    );
                }
                IrOp::CloseLocal(index) => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    invoke_helper!(
                        qjs::JSJitHelperId_JS_JIT_HELPER_CLOSE_LOC,
                        state,
                        depth,
                        &[u32::from(index)]
                    );
                }
                IrOp::Generic(operation) => lower_generic(
                    builder,
                    &helper_lowering,
                    &mut helper_states,
                    &mut depth,
                    operation,
                )?,
                IrOp::Iterator(operation) => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    lower_iterator(
                        builder,
                        &helper_lowering,
                        state,
                        &mut depth,
                        operation,
                        array_values_next,
                    )?;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::GetArgument(index) => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let source = use_pair(builder, arguments[index as usize]);
                    lower_dup_if_refcounted(
                        builder,
                        &helper_lowering,
                        state,
                        depth,
                        depth,
                        source,
                        flat_argument_slot(index),
                        false,
                        false,
                    )?;
                    depth += 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::GetLocal(index) => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let source = use_pair(builder, locals[index as usize]);
                    lower_dup_if_refcounted(
                        builder,
                        &helper_lowering,
                        state,
                        depth,
                        depth,
                        source,
                        flat_local_slot(ir, index),
                        false,
                        false,
                    )?;
                    depth += 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::GetLocalChecked(index) => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let source = use_pair(builder, locals[index as usize]);
                    lower_dup_if_refcounted(
                        builder,
                        &helper_lowering,
                        state,
                        depth,
                        depth,
                        source,
                        flat_local_slot(ir, index),
                        true,
                        false,
                    )?;
                    depth += 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::GetLocalPair => {
                    for local_index in 0..2_u16 {
                        let state = helper_states
                            .next()
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let output_index = depth + usize::from(local_index);
                        let source = use_pair(builder, locals[usize::from(local_index)]);
                        lower_dup_if_refcounted(
                            builder,
                            &helper_lowering,
                            state,
                            output_index,
                            output_index,
                            source,
                            flat_local_slot(ir, local_index),
                            false,
                            true,
                        )?;
                    }
                    depth += 2;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::PutArgument { index, keep } => {
                    let source_index = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let destination = flat_argument_slot(index);
                    let free_state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let destination_value = use_pair(builder, arguments[index as usize]);
                    lower_free_if_refcounted(
                        builder,
                        &helper_lowering,
                        free_state,
                        depth,
                        destination_value,
                        destination,
                        arguments[index as usize],
                        arg_buf,
                        index as usize,
                    )?;
                    if keep {
                        let dup_state = helper_states
                            .next()
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        // The DUP helper requires an unoccupied destination.
                        // A primitive previous value is released without a
                        // helper, so clear it explicitly before duplicating.
                        let undefined = constant_pair(
                            builder,
                            TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64),
                        );
                        define_pair(builder, arguments[index as usize], undefined);
                        let source = use_pair(builder, stack[source_index]);
                        lower_dup_local_if_refcounted(
                            builder,
                            &helper_lowering,
                            dup_state,
                            depth,
                            source,
                            flat_stack_slot(ir, source_index)?,
                            destination,
                            arguments[index as usize],
                            arg_buf,
                            index as usize,
                        )?;
                    } else {
                        let value = use_pair(builder, stack[source_index]);
                        define_pair(builder, arguments[index as usize], value);
                        clear_pair(
                            builder,
                            stack[source_index],
                            stack_base,
                            source_index,
                            layout,
                        )?;
                        depth -= 1;
                        set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                    }
                }
                IrOp::PutLocal { index, keep } => {
                    let source_index = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let destination = flat_local_slot(ir, index);
                    let free_state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let destination_value = use_pair(builder, locals[index as usize]);
                    lower_free_if_refcounted(
                        builder,
                        &helper_lowering,
                        free_state,
                        depth,
                        destination_value,
                        destination,
                        locals[index as usize],
                        var_buf,
                        index as usize,
                    )?;
                    if keep {
                        let dup_state = helper_states
                            .next()
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        // See PutArgument: the DUP helper rejects an occupied
                        // destination, and a primitive previous value leaves
                        // the slot occupied because it needs no FREE helper.
                        let undefined = constant_pair(
                            builder,
                            TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64),
                        );
                        define_pair(builder, locals[index as usize], undefined);
                        let source = use_pair(builder, stack[source_index]);
                        lower_dup_local_if_refcounted(
                            builder,
                            &helper_lowering,
                            dup_state,
                            depth,
                            source,
                            flat_stack_slot(ir, source_index)?,
                            destination,
                            locals[index as usize],
                            var_buf,
                            index as usize,
                        )?;
                    } else {
                        let value = use_pair(builder, stack[source_index]);
                        define_pair(builder, locals[index as usize], value);
                        clear_pair(
                            builder,
                            stack[source_index],
                            stack_base,
                            source_index,
                            layout,
                        )?;
                        depth -= 1;
                        set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                    }
                }
                IrOp::PutLocalChecked { index, initialize } => {
                    let _ = initialize;
                    let source_index = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let destination = flat_local_slot(ir, index);
                    let free_state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let destination_value = use_pair(builder, locals[index as usize]);
                    lower_free_if_refcounted(
                        builder,
                        &helper_lowering,
                        free_state,
                        depth,
                        destination_value,
                        destination,
                        locals[index as usize],
                        var_buf,
                        index as usize,
                    )?;
                    let value = use_pair(builder, stack[source_index]);
                    define_pair(builder, locals[index as usize], value);
                    clear_pair(
                        builder,
                        stack[source_index],
                        stack_base,
                        source_index,
                        layout,
                    )?;
                    depth -= 1;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::SetLocalUninitialized(index) => {
                    let destination = flat_local_slot(ir, index);
                    let free_state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let destination_value = use_pair(builder, locals[index as usize]);
                    lower_free_if_refcounted(
                        builder,
                        &helper_lowering,
                        free_state,
                        depth,
                        destination_value,
                        destination,
                        locals[index as usize],
                        var_buf,
                        index as usize,
                    )?;
                    let value = constant_pair(
                        builder,
                        TaggedValue::new(0, qjs::JS_TAG_UNINITIALIZED as i64),
                    );
                    define_pair(builder, locals[index as usize], value);
                }
                IrOp::Drop => {
                    let index = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let slot = flat_stack_slot(ir, index)?;
                    let value = use_pair(builder, stack[index]);
                    lower_free_if_refcounted(
                        builder,
                        &helper_lowering,
                        state,
                        depth,
                        value,
                        slot,
                        stack[index],
                        stack_base,
                        index,
                    )?;
                    depth = index;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::Stack(operation) => {
                    let old_depth = depth;
                    match operation {
                        StackOp::Nip | StackOp::Nip1 => {
                            let (take, _) = stack_operation_order(operation);
                            let start = depth
                                .checked_sub(take)
                                .ok_or(CompileFailure::InvalidArtifact)?;
                            let state = helper_states
                                .next()
                                .ok_or(CompileFailure::InvalidArtifact)?;
                            let value = use_pair(builder, stack[start]);
                            lower_free_always_in_stress(
                                builder,
                                &helper_lowering,
                                state,
                                depth,
                                value,
                                flat_stack_slot(ir, start)?,
                                stack[start],
                                stack_base,
                                start,
                            )?;
                            apply_stack_operation(builder, &stack, &mut depth, operation);
                        }
                        StackOp::Dup
                        | StackOp::Dup1
                        | StackOp::Dup2
                        | StackOp::Dup3
                        | StackOp::Insert2
                        | StackOp::Insert3
                        | StackOp::Insert4 => {
                            let (take, order) = stack_operation_order(operation);
                            let start = depth
                                .checked_sub(take)
                                .ok_or(CompileFailure::InvalidArtifact)?;
                            let duplicated_sources: &[usize] = match operation {
                                StackOp::Dup | StackOp::Dup1 => &[0],
                                StackOp::Dup2 => &[0, 1],
                                StackOp::Dup3 => &[0, 1, 2],
                                StackOp::Insert2 => &[1],
                                StackOp::Insert3 => &[2],
                                StackOp::Insert4 => &[3],
                                _ => unreachable!(),
                            };
                            for (created, source) in duplicated_sources.iter().copied().enumerate()
                            {
                                let state = helper_states
                                    .next()
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                let output_index = old_depth + created;
                                let source_pair = use_pair(builder, stack[start + source]);
                                lower_dup_if_refcounted(
                                    builder,
                                    &helper_lowering,
                                    state,
                                    output_index,
                                    output_index,
                                    source_pair,
                                    flat_stack_slot(ir, start + source)?,
                                    false,
                                    false,
                                )?;
                            }
                            debug_assert_eq!(
                                order.len(),
                                old_depth - start + duplicated_sources.len()
                            );
                            apply_stack_operation(builder, &stack, &mut depth, operation);
                        }
                        _ => apply_stack_operation(builder, &stack, &mut depth, operation),
                    }
                    if depth < old_depth {
                        for (offset, variables) in stack
                            .get(depth..old_depth)
                            .ok_or(CompileFailure::InvalidArtifact)?
                            .iter()
                            .copied()
                            .enumerate()
                        {
                            clear_pair(builder, variables, stack_base, depth + offset, layout)?;
                        }
                    }
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::Unary(operation) => {
                    let index = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    if matches!(
                        operation,
                        UnaryOp::IsUndefinedOrNull | UnaryOp::IsUndefined | UnaryOp::IsNull
                    ) {
                        let state = helper_states
                            .next()
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let slot = flat_stack_slot(ir, index)?;
                        let value = use_pair(builder, stack[index]);
                        let result = match operation {
                            UnaryOp::IsUndefinedOrNull => {
                                let is_undefined =
                                    tag_is(builder, value.tag, qjs::JS_TAG_UNDEFINED);
                                let is_null = tag_is(builder, value.tag, qjs::JS_TAG_NULL);
                                builder.ins().bor(is_undefined, is_null)
                            }
                            UnaryOp::IsUndefined => {
                                tag_is(builder, value.tag, qjs::JS_TAG_UNDEFINED)
                            }
                            UnaryOp::IsNull => tag_is(builder, value.tag, qjs::JS_TAG_NULL),
                            _ => unreachable!(),
                        };
                        lower_free_always_in_stress(
                            builder,
                            &helper_lowering,
                            state,
                            depth,
                            value,
                            slot,
                            stack[index],
                            stack_base,
                            index,
                        )?;
                        let boolean = pair_from_bool(builder, result);
                        define_pair(builder, stack[index], boolean);
                    } else if matches!(operation, UnaryOp::Plus | UnaryOp::LogicalNot) {
                        let state = helper_states
                            .next()
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let slot = flat_stack_slot(ir, index)?;
                        let helper = if operation == UnaryOp::Plus {
                            qjs::JSJitHelperId_JS_JIT_HELPER_TO_NUMERIC
                        } else {
                            qjs::JSJitHelperId_JS_JIT_HELPER_TO_BOOL
                        };
                        invoke_helper!(helper, state, depth, &[slot, slot]);
                        reload_pair(builder, stack[index], stack_base, index, layout);
                        if operation == UnaryOp::LogicalNot {
                            let boolean = use_pair(builder, stack[index]);
                            let payload = builder.ins().bxor_imm(boolean.payload, 1);
                            let result = Pair {
                                payload,
                                tag: boolean.tag,
                            };
                            define_pair(builder, stack[index], result);
                        }
                    } else {
                        // Numeric operands are lowered natively; anything
                        // else takes the exact interpreter slow path
                        // (`js_unary_arith_slow` / `js_not_slow`) through the
                        // UNARY_ARITH_SLOW helper, which owns the coercion,
                        // BigInt, and exception semantics.
                        let state = helper_states
                            .next()
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let slot = flat_stack_slot(ir, index)?;
                        let opcode_name = match operation {
                            UnaryOp::Neg => "neg",
                            UnaryOp::Increment => "inc",
                            UnaryOp::Decrement => "dec",
                            UnaryOp::BitNot => "not",
                            _ => return Err(CompileFailure::InvalidArtifact),
                        };
                        let opcode = quickjs_opcode_id(opcode_name)?;
                        let value = use_pair(builder, stack[index]);
                        let numeric_tag = emit_numeric_tag(builder, value.tag);
                        let numeric = builder.create_block();
                        let generic = builder.create_block();
                        let continuation = builder.create_block();
                        builder.ins().brif(numeric_tag, numeric, &[], generic, &[]);

                        builder.seal_block(numeric);
                        builder.switch_to_block(numeric);
                        let result = emit_unary(builder, value, operation);
                        define_pair(builder, stack[index], result);
                        builder.ins().jump(continuation, &[]);

                        builder.seal_block(generic);
                        builder.switch_to_block(generic);
                        invoke_helper!(
                            qjs::JSJitHelperId_JS_JIT_HELPER_UNARY_ARITH_SLOW,
                            state,
                            depth,
                            &[slot, slot, opcode]
                        );
                        reload_pair(builder, stack[index], stack_base, index, layout);
                        builder.ins().jump(continuation, &[]);

                        builder.seal_block(continuation);
                        builder.switch_to_block(continuation);
                    }
                }
                IrOp::PostUnary(operation) => {
                    let value = use_pair(builder, stack[depth - 1]);
                    helper_lowering.deopt_unless_numeric(
                        builder,
                        &[value],
                        depth,
                        instruction.pc,
                    )?;
                    let result = emit_unary(builder, value, operation);
                    define_pair(builder, stack[depth], result);
                    depth += 1;
                }
                IrOp::LocalUnary { index, op } => {
                    let value = use_pair(builder, locals[index as usize]);
                    helper_lowering.deopt_unless_numeric(
                        builder,
                        &[value],
                        depth,
                        instruction.pc,
                    )?;
                    let result = emit_unary(builder, value, op);
                    define_pair(builder, locals[index as usize], result);
                }
                IrOp::AddLocal(index) => {
                    let left = use_pair(builder, locals[index as usize]);
                    let right = use_pair(builder, stack[depth - 1]);
                    helper_lowering.deopt_unless_numeric(
                        builder,
                        &[left, right],
                        depth,
                        instruction.pc,
                    )?;
                    let result = emit_binary(builder, left, right, BinaryOp::Add);
                    define_pair(builder, locals[index as usize], result);
                    depth -= 1;
                }
                IrOp::Binary(operation) => {
                    let left_index = depth
                        .checked_sub(2)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let right_index = depth - 1;
                    let helper = match operation {
                        BinaryOp::Add => Some((qjs::JSJitHelperId_JS_JIT_HELPER_ADD_SLOW, None)),
                        BinaryOp::Mod => Some((
                            qjs::JSJitHelperId_JS_JIT_HELPER_BINARY_ARITH_SLOW,
                            Some(quickjs_opcode_id("mod")?),
                        )),
                        BinaryOp::LessThan => Some((
                            qjs::JSJitHelperId_JS_JIT_HELPER_COMPARE_SLOW,
                            Some(qjs::JSJitCompareOp_JS_JIT_COMPARE_LT),
                        )),
                        BinaryOp::LessThanOrEqual => Some((
                            qjs::JSJitHelperId_JS_JIT_HELPER_COMPARE_SLOW,
                            Some(qjs::JSJitCompareOp_JS_JIT_COMPARE_LTE),
                        )),
                        BinaryOp::GreaterThan => Some((
                            qjs::JSJitHelperId_JS_JIT_HELPER_COMPARE_SLOW,
                            Some(qjs::JSJitCompareOp_JS_JIT_COMPARE_GT),
                        )),
                        BinaryOp::GreaterThanOrEqual => Some((
                            qjs::JSJitHelperId_JS_JIT_HELPER_COMPARE_SLOW,
                            Some(qjs::JSJitCompareOp_JS_JIT_COMPARE_GTE),
                        )),
                        BinaryOp::Equal => Some((
                            qjs::JSJitHelperId_JS_JIT_HELPER_COMPARE_SLOW,
                            Some(qjs::JSJitCompareOp_JS_JIT_COMPARE_EQ),
                        )),
                        BinaryOp::NotEqual => Some((
                            qjs::JSJitHelperId_JS_JIT_HELPER_COMPARE_SLOW,
                            Some(qjs::JSJitCompareOp_JS_JIT_COMPARE_NEQ),
                        )),
                        BinaryOp::StrictEqual => Some((
                            qjs::JSJitHelperId_JS_JIT_HELPER_COMPARE_SLOW,
                            Some(qjs::JSJitCompareOp_JS_JIT_COMPARE_STRICT_EQ),
                        )),
                        BinaryOp::StrictNotEqual => Some((
                            qjs::JSJitHelperId_JS_JIT_HELPER_COMPARE_SLOW,
                            Some(qjs::JSJitCompareOp_JS_JIT_COMPARE_STRICT_NEQ),
                        )),
                        _ => None,
                    };
                    if let Some((helper, comparison)) = helper {
                        let state = helper_states
                            .next()
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let left = flat_stack_slot(ir, left_index)?;
                        let right = flat_stack_slot(ir, right_index)?;
                        let left_value = use_pair(builder, stack[left_index]);
                        let right_value = use_pair(builder, stack[right_index]);
                        let fast = if operation == BinaryOp::Mod {
                            emit_int_mod_fast_condition(builder, left_value, right_value)
                        } else {
                            let left_numeric = emit_numeric_tag(builder, left_value.tag);
                            let right_numeric = emit_numeric_tag(builder, right_value.tag);
                            builder.ins().band(left_numeric, right_numeric)
                        };
                        let numeric = builder.create_block();
                        let generic = builder.create_block();
                        let continuation = builder.create_block();
                        builder.ins().brif(fast, numeric, &[], generic, &[]);

                        builder.seal_block(numeric);
                        builder.switch_to_block(numeric);
                        let result = if operation == BinaryOp::Mod {
                            emit_int_mod(builder, left_value, right_value)
                        } else {
                            emit_binary(builder, left_value, right_value, operation)
                        };
                        define_pair(builder, stack[left_index], result);
                        builder.ins().jump(continuation, &[]);

                        builder.seal_block(generic);
                        builder.switch_to_block(generic);
                        if let Some(comparison) = comparison {
                            invoke_helper!(helper, state, depth, &[left, left, right, comparison]);
                        } else {
                            invoke_helper!(helper, state, depth, &[left, left, right]);
                        }
                        reload_pair(builder, stack[left_index], stack_base, left_index, layout);
                        reload_pair(builder, stack[right_index], stack_base, right_index, layout);
                        builder.ins().jump(continuation, &[]);

                        builder.seal_block(continuation);
                        builder.switch_to_block(continuation);
                        depth -= 1;
                        set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                    } else {
                        let left = use_pair(builder, stack[left_index]);
                        let right = use_pair(builder, stack[right_index]);
                        helper_lowering.deopt_unless_numeric(
                            builder,
                            &[left, right],
                            depth,
                            instruction.pc,
                        )?;
                        let result = emit_binary(builder, left, right, operation);
                        depth -= 1;
                        define_pair(builder, stack[depth - 1], result);
                    }
                }
                IrOp::Jump(target) => {
                    builder.ins().jump(blocks[&target], &[]);
                    terminated = true;
                }
                IrOp::Branch { target, when_true } => {
                    let condition_index = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let condition_slot = flat_stack_slot(ir, condition_index)?;
                    if !prior_op_was_boolean {
                        invoke_helper!(
                            qjs::JSJitHelperId_JS_JIT_HELPER_TO_BOOL,
                            state,
                            depth,
                            &[condition_slot, condition_slot]
                        );
                        reload_pair(
                            builder,
                            stack[condition_index],
                            stack_base,
                            condition_index,
                            layout,
                        );
                    }
                    let condition = use_pair(builder, stack[condition_index]);
                    depth -= 1;
                    let truthy = builder.ins().ireduce(types::I8, condition.payload);
                    clear_pair(
                        builder,
                        stack[condition_index],
                        stack_base,
                        condition_index,
                        layout,
                    )?;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                    let taken = if when_true {
                        truthy
                    } else {
                        builder.ins().bxor_imm(truthy, 1)
                    };
                    let fallthrough = ir
                        .blocks
                        .get(block_index + 1)
                        .ok_or(CompileFailure::InvalidArtifact)?
                        .start_pc;
                    builder
                        .ins()
                        .brif(taken, blocks[&target], &[], blocks[&fallthrough], &[]);
                    terminated = true;
                }
                IrOp::Catch(handler_pc) => {
                    let offset = constant_pair(
                        builder,
                        TaggedValue::new(u64::from(handler_pc), qjs::JS_TAG_CATCH_OFFSET as i64),
                    );
                    define_pair(
                        builder,
                        *stack.get(depth).ok_or(CompileFailure::InvalidArtifact)?,
                        offset,
                    );
                    depth += 1;
                }
                IrOp::NipCatch => {
                    // `catch_offset value -> value`; a catch offset owns no
                    // reference, so moving the value over it releases nothing.
                    let value_index = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let catch_index = depth
                        .checked_sub(2)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let value = use_pair(builder, stack[value_index]);
                    define_pair(builder, stack[catch_index], value);
                    clear_pair(builder, stack[value_index], stack_base, value_index, layout)?;
                    depth = value_index;
                    set_visible_stack_depth(builder, frame, stack_base, depth, layout)?;
                }
                IrOp::Throw | IrOp::ThrowError { .. } => {
                    let state = helper_states
                        .next()
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let frame_state = ir.frame_states.get(state);
                    let visible_depth = frame_state
                        .slots
                        .len()
                        .checked_sub(fixed_slots)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    materialize_frame(
                        builder,
                        frame,
                        arg_buf,
                        var_buf,
                        stack_base,
                        &arguments,
                        &locals,
                        &stack,
                        depth,
                        visible_depth,
                        frame_state.pc,
                        pointer_type,
                        layout,
                    )?;
                    let exception_depth = match instruction.op {
                        IrOp::ThrowError { atom, kind } => {
                            emit_exception_api_call(
                                builder,
                                frame,
                                qjs::JS_JitThrowError as *const () as usize,
                                state,
                                &[atom, u32::from(kind)],
                                pointer_type,
                            )?;
                            depth
                        }
                        _ => {
                            let value_index = depth
                                .checked_sub(1)
                                .ok_or(CompileFailure::InvalidArtifact)?;
                            emit_exception_api_call(
                                builder,
                                frame,
                                qjs::JS_JitThrowValue as *const () as usize,
                                state,
                                &[flat_stack_slot(ir, value_index)?],
                                pointer_type,
                            )?;
                            // The operand is the pending exception now.
                            clear_pair(
                                builder,
                                stack[value_index],
                                stack_base,
                                value_index,
                                layout,
                            )?;
                            value_index
                        }
                    };
                    // Both calls always leave an exception pending.
                    emit_exception_edge(
                        builder,
                        frame,
                        sret,
                        stack_base,
                        exception_depth,
                        exception_target,
                        pointer_type,
                        layout,
                    )?;
                    terminated = true;
                }
                IrOp::Gosub { target, return_pc } => {
                    let return_offset = constant_pair(
                        builder,
                        TaggedValue::new(i64::from(return_pc) as u64, qjs::JS_TAG_INT as i64),
                    );
                    define_pair(
                        builder,
                        *stack.get(depth).ok_or(CompileFailure::InvalidArtifact)?,
                        return_offset,
                    );
                    builder.ins().jump(
                        *blocks.get(&target).ok_or(CompileFailure::InvalidArtifact)?,
                        &[],
                    );
                    terminated = true;
                }
                IrOp::Ret { ref targets } => {
                    let index = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let value = use_pair(builder, stack[index]);
                    clear_pair(builder, stack[index], stack_base, index, layout)?;
                    let is_int = tag_is(builder, value.tag, qjs::JS_TAG_INT);
                    let offset = builder.ins().ireduce(types::I32, value.payload);
                    let unknown = builder.create_block();
                    for target in targets.iter() {
                        let block = *blocks.get(target).ok_or(CompileFailure::InvalidArtifact)?;
                        let matches = builder.ins().icmp_imm(
                            IntCC::Equal,
                            offset,
                            i64::from(
                                i32::try_from(*target)
                                    .map_err(|_| CompileFailure::InvalidArtifact)?,
                            ),
                        );
                        let hit = builder.ins().band(is_int, matches);
                        let next = builder.create_block();
                        builder.ins().brif(hit, block, &[], next, &[]);
                        builder.seal_block(next);
                        builder.switch_to_block(next);
                    }
                    builder.ins().jump(unknown, &[]);
                    // Only this function's `gosub` sites push return offsets;
                    // anything else is resumed by the interpreter's own `ret`.
                    builder.seal_block(unknown);
                    builder.switch_to_block(unknown);
                    builder.set_cold_block(unknown);
                    define_pair(builder, stack[index], value);
                    materialize_frame(
                        builder,
                        frame,
                        arg_buf,
                        var_buf,
                        stack_base,
                        &arguments,
                        &locals,
                        &stack,
                        depth,
                        depth,
                        instruction.pc,
                        pointer_type,
                        layout,
                    )?;
                    let bytecode = builder.ins().load(
                        pointer_type,
                        MemFlags::new(),
                        frame,
                        layout.bytecode_start,
                    );
                    let resume = builder.ins().iadd_imm(bytecode, i64::from(instruction.pc));
                    emit_exit(
                        builder,
                        sret,
                        qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
                        Some(resume),
                        pointer_type,
                    );
                    terminated = true;
                }
                IrOp::Return => {
                    let result_index = depth
                        .checked_sub(1)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    materialize_frame(
                        builder,
                        frame,
                        arg_buf,
                        var_buf,
                        stack_base,
                        &arguments,
                        &locals,
                        &stack,
                        depth,
                        depth,
                        instruction.pc,
                        pointer_type,
                        layout,
                    )?;
                    let result = use_pair(builder, stack[result_index]);
                    store_jsvalue(builder, frame, layout.result, result, layout);
                    clear_pair(
                        builder,
                        stack[result_index],
                        stack_base,
                        result_index,
                        layout,
                    )?;
                    force_visible_stack_depth(builder, frame, stack_base, result_index, layout)?;
                    emit_exit(
                        builder,
                        sret,
                        qjs::JSJitExitKind_JS_JIT_EXIT_DONE,
                        None,
                        pointer_type,
                    );
                    terminated = true;
                }
                IrOp::ReturnUndefined => {
                    materialize_frame(
                        builder,
                        frame,
                        arg_buf,
                        var_buf,
                        stack_base,
                        &arguments,
                        &locals,
                        &stack,
                        depth,
                        depth,
                        instruction.pc,
                        pointer_type,
                        layout,
                    )?;
                    let result =
                        constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64));
                    store_jsvalue(builder, frame, layout.result, result, layout);
                    emit_exit(
                        builder,
                        sret,
                        qjs::JSJitExitKind_JS_JIT_EXIT_DONE,
                        None,
                        pointer_type,
                    );
                    terminated = true;
                }
            }
            if helper_states.next().is_some() {
                return Err(CompileFailure::InvalidArtifact);
            }
            if let Some(region) = fused {
                if instruction_index == region.end {
                    if terminated {
                        return Err(CompileFailure::InvalidArtifact);
                    }
                    if let (Some(object_index), Some(fast)) = (region.retain_literal, object_fast) {
                        emit_retain_literal_shape(builder, &helper_lowering, fast, object_index)?;
                    }
                    builder.ins().jump(region.join, &[]);
                    builder.seal_block(region.join);
                    builder.switch_to_block(region.join);
                    fused = None;
                }
            }
            if terminated {
                break;
            }
        }
        if fused.is_some() {
            return Err(CompileFailure::InvalidArtifact);
        }
        if !terminated {
            if let Some(next) = ir.blocks.get(block_index + 1) {
                builder.ins().jump(blocks[&next.start_pc], &[]);
            } else {
                return Err(CompileFailure::InvalidArtifact);
            }
        }
    }

    // Landing pads run the interpreter's exception prologue on the published
    // frame. A caught exception resumes the handler natively with the caught
    // value in the catch offset's slot; anything else (uncatchable errors,
    // invariant failures) leaves through the ordinary exception exit so the
    // interpreter unwinds the exact published frame.
    for (handler_pc, landing) in &landing_blocks {
        let site = *ir
            .catch_sites
            .get(handler_pc)
            .ok_or(CompileFailure::InvalidArtifact)?;
        let catch_index = usize::from(site.catch_index);
        let handler = *blocks
            .get(handler_pc)
            .ok_or(CompileFailure::InvalidArtifact)?;
        builder.set_srcloc(SourceLoc::default());
        builder.switch_to_block(*landing);
        builder.set_cold_block(*landing);
        let status = emit_exception_api_call(
            builder,
            frame,
            qjs::JS_JitCatchException as *const () as usize,
            site.state,
            &[flat_stack_slot(ir, catch_index)?, *handler_pc],
            pointer_type,
        )?;
        let caught = builder.ins().icmp_imm(IntCC::Equal, status, 0);
        let resume = builder.create_block();
        let propagate = builder.create_block();
        builder.ins().brif(caught, resume, &[], propagate, &[]);
        builder.seal_block(resume);
        builder.seal_block(propagate);
        builder.switch_to_block(propagate);
        builder.set_cold_block(propagate);
        emit_exit(
            builder,
            sret,
            qjs::JSJitExitKind_JS_JIT_EXIT_EXCEPTION,
            None,
            pointer_type,
        );
        builder.switch_to_block(resume);
        // Every edge into a landing pad published the frame first, so the
        // frame owns the argument/local slots here. In a function that
        // creates closures, the throwing helper (or the catch prologue's
        // backtrace hook) may have run JavaScript that wrote those slots
        // through an attached var ref and released the old values; the
        // handler must resume from the frame, not from stale SSA copies.
        helper_lowering.reload_captured_slots(builder);
        reload_pair(
            builder,
            *stack
                .get(catch_index)
                .ok_or(CompileFailure::InvalidArtifact)?,
            stack_base,
            catch_index,
            layout,
        );
        let undefined = constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64));
        for pair in stack.iter().skip(catch_index + 1) {
            define_pair(builder, *pair, undefined);
        }
        builder.ins().jump(handler, &[]);
    }

    builder.set_srcloc(SourceLoc::default());
    builder.switch_to_block(retry);
    emit_exit(
        builder,
        sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_RETRY_INTERPRETER,
        None,
        pointer_type,
    );
    builder.switch_to_block(invariant_trap);
    builder.ins().trap(TrapCode::unwrap_user(1));
    Ok(())
}

/// Functions that instantiate closures may share argument/local storage with
/// attached var refs (`JSVarRef.pvalue` points into the interpreter frame).
pub(crate) fn creates_closures(ir: &BaselineIr) -> bool {
    ir.blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .any(|instruction| matches!(instruction.op, IrOp::FClosure(_)))
}

fn next_helper_state(
    states: &mut impl Iterator<Item = FrameStateId>,
) -> Result<FrameStateId, CompileFailure> {
    states.next().ok_or(CompileFailure::InvalidArtifact)
}

/// `GENERIC_OP` packs the QuickJS opcode in bits 0..7 and the bytecode's
/// 8-bit immediate in bits 8..15.
fn generic_operation(name: &str, immediate: u8) -> Result<u32, CompileFailure> {
    Ok(quickjs_opcode_id(name)? | (u32::from(immediate) << 8))
}

/// Lowers an opcode whose semantics are executed exactly by `GENERIC_OP`
/// (or `BINARY_ARITH_SLOW` for `pow`). Every operand stays in its frame slot
/// across the helper; the helper clears consumed slots on success and on a
/// language exception, so the exception depth is the pre-instruction depth.
fn lower_generic(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: &mut usize,
    operation: GenericOp,
) -> Result<(), CompileFailure> {
    const NONE: u32 = qjs::JS_JIT_SLOT_NONE;
    let live = *depth;
    let (pop, push) = operation.stack_effect();
    let base = live
        .checked_sub(pop)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let slot = |index: usize| flat_stack_slot(helpers.ir, index);
    let state = next_helper_state(states)?;
    let generic = qjs::JSJitHelperId_JS_JIT_HELPER_GENERIC_OP;
    match operation {
        GenericOp::PushThis
        | GenericOp::SpecialObject(_)
        | GenericOp::GetVarUndef(_)
        | GenericOp::DeleteVar(_) => {
            let (name, immediate, atom) = match operation {
                GenericOp::PushThis => ("push_this", 0, NONE),
                GenericOp::SpecialObject(kind) => ("special_object", kind, NONE),
                GenericOp::GetVarUndef(atom) => ("get_var_undef", 0, atom),
                GenericOp::DeleteVar(atom) => ("delete_var", 0, atom),
                _ => unreachable!(),
            };
            let output = slot(live)?;
            helpers.invoke(
                builder,
                generic,
                state,
                live,
                live,
                &[output, NONE, atom, generic_operation(name, immediate)?],
            )?;
            reload_pair(
                builder,
                helpers.stack[live],
                helpers.stack_base,
                live,
                helpers.layout,
            );
        }
        GenericOp::PutVar(atom) => {
            helpers.invoke(
                builder,
                generic,
                state,
                live,
                live,
                &[NONE, slot(base)?, atom, generic_operation("put_var", 0)?],
            )?;
            clear_pair(
                builder,
                helpers.stack[base],
                helpers.stack_base,
                base,
                helpers.layout,
            )?;
        }
        GenericOp::ToPropertyKey2 => {
            // The interpreter leaves an object-coercible receiver and an
            // Int32/string/symbol key untouched; everything else (including
            // the TypeError) takes the exact helper.
            let object = use_pair(builder, helpers.stack[base]);
            let key = use_pair(builder, helpers.stack[base + 1]);
            let undefined = tag_is(builder, object.tag, qjs::JS_TAG_UNDEFINED);
            let null = tag_is(builder, object.tag, qjs::JS_TAG_NULL);
            let not_coercible = builder.ins().bor(undefined, null);
            let int_key = tag_is(builder, key.tag, qjs::JS_TAG_INT);
            let string_key = tag_is(builder, key.tag, qjs::JS_TAG_STRING);
            let symbol_key = tag_is(builder, key.tag, qjs::JS_TAG_SYMBOL);
            let key_ready = builder.ins().bor(int_key, string_key);
            let key_ready = builder.ins().bor(key_ready, symbol_key);
            let slow_needed = builder.ins().bxor_imm(key_ready, 1);
            let slow_needed = builder.ins().bor(slow_needed, not_coercible);
            let slow = builder.create_block();
            let continuation = builder.create_block();
            builder
                .ins()
                .brif(slow_needed, slow, &[], continuation, &[]);
            builder.seal_block(slow);
            builder.switch_to_block(slow);
            builder.set_cold_block(slow);
            helpers.invoke(
                builder,
                generic,
                state,
                live,
                live,
                &[
                    slot(base + 1)?,
                    slot(base)?,
                    NONE,
                    generic_operation("to_propkey2", 0)?,
                ],
            )?;
            reload_pair(
                builder,
                helpers.stack[base + 1],
                helpers.stack_base,
                base + 1,
                helpers.layout,
            );
            builder.ins().jump(continuation, &[]);
            builder.seal_block(continuation);
            builder.switch_to_block(continuation);
        }
        GenericOp::TypeOf
        | GenericOp::TypeOfIsUndefined
        | GenericOp::TypeOfIsFunction
        | GenericOp::ToObject => {
            let name = match operation {
                GenericOp::TypeOf => "typeof",
                GenericOp::TypeOfIsUndefined => "typeof_is_undefined",
                GenericOp::TypeOfIsFunction => "typeof_is_function",
                _ => "to_object",
            };
            let input = slot(base)?;
            let arguments = [input, input, NONE, generic_operation(name, 0)?];
            if matches!(
                operation,
                GenericOp::TypeOfIsUndefined | GenericOp::TypeOfIsFunction
            ) {
                // Non-negative tags are neither objects nor reference counted:
                // their `typeof` never names a function and only `undefined`
                // names itself, so the answer needs no helper or release.
                let value = use_pair(builder, helpers.stack[base]);
                let primitive =
                    builder
                        .ins()
                        .icmp_imm(IntCC::SignedGreaterThanOrEqual, value.tag, 0);
                let fast = builder.create_block();
                let slow = builder.create_block();
                let continuation = builder.create_block();
                builder.ins().brif(primitive, fast, &[], slow, &[]);
                builder.seal_block(fast);
                builder.switch_to_block(fast);
                let result = if operation == GenericOp::TypeOfIsUndefined {
                    tag_is(builder, value.tag, qjs::JS_TAG_UNDEFINED)
                } else {
                    builder.ins().iconst(types::I8, 0)
                };
                let result = pair_from_bool(builder, result);
                define_pair(builder, helpers.stack[base], result);
                builder.ins().jump(continuation, &[]);
                builder.seal_block(slow);
                builder.switch_to_block(slow);
                builder.set_cold_block(slow);
                helpers.invoke(builder, generic, state, live, live, &arguments)?;
                reload_pair(
                    builder,
                    helpers.stack[base],
                    helpers.stack_base,
                    base,
                    helpers.layout,
                );
                builder.ins().jump(continuation, &[]);
                builder.seal_block(continuation);
                builder.switch_to_block(continuation);
            } else {
                helpers.invoke(builder, generic, state, live, live, &arguments)?;
                reload_pair(
                    builder,
                    helpers.stack[base],
                    helpers.stack_base,
                    base,
                    helpers.layout,
                );
            }
        }
        GenericOp::In | GenericOp::InstanceOf | GenericOp::Delete | GenericOp::Pow => {
            let left = slot(base)?;
            let right = slot(base + 1)?;
            if operation == GenericOp::Pow {
                helpers.invoke(
                    builder,
                    qjs::JSJitHelperId_JS_JIT_HELPER_BINARY_ARITH_SLOW,
                    state,
                    live,
                    live,
                    &[left, left, right, quickjs_opcode_id("pow")?],
                )?;
            } else {
                let name = match operation {
                    GenericOp::In => "in",
                    GenericOp::InstanceOf => "instanceof",
                    _ => "delete",
                };
                helpers.invoke(
                    builder,
                    generic,
                    state,
                    live,
                    live,
                    &[left, left, right, generic_operation(name, 0)?],
                )?;
            }
            reload_pair(
                builder,
                helpers.stack[base],
                helpers.stack_base,
                base,
                helpers.layout,
            );
            clear_pair(
                builder,
                helpers.stack[base + 1],
                helpers.stack_base,
                base + 1,
                helpers.layout,
            )?;
        }
    }
    *depth = base + push;
    helpers.set_depth(builder, *depth)
}

#[allow(clippy::too_many_arguments)]
fn lower_dup_if_refcounted(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    state: FrameStateId,
    live_depth: usize,
    output_index: usize,
    source: Pair,
    source_slot: u32,
    checked: bool,
    stress_primitives: bool,
) -> Result<(), CompileFailure> {
    let output_slot = flat_stack_slot(helpers.ir, output_index)?;
    let primitive = builder.create_block();
    let slow = builder.create_block();
    let continuation = builder.create_block();
    emit_inline_dup_dispatch(
        builder,
        helpers,
        source,
        checked,
        stress_primitives,
        primitive,
        slow,
    );
    builder.seal_block(primitive);
    builder.switch_to_block(primitive);
    define_pair(builder, helpers.stack[output_index], source);
    builder.ins().jump(continuation, &[]);
    builder.seal_block(slow);
    builder.switch_to_block(slow);
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_DUP,
        state,
        live_depth,
        live_depth,
        &[output_slot, source_slot],
    )?;
    reload_pair(
        builder,
        helpers.stack[output_index],
        helpers.stack_base,
        output_index,
        helpers.layout,
    );
    builder.ins().jump(continuation, &[]);
    builder.seal_block(continuation);
    builder.switch_to_block(continuation);
    Ok(())
}

/// Branches to `primitive` (after an inline `ref_count++` for heap values)
/// when DUP needs no helper, else to `slow`. Checked locals send
/// `JS_TAG_UNINITIALIZED` to the helper exactly as before.
fn emit_inline_dup_dispatch(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    source: Pair,
    checked: bool,
    stress_primitives: bool,
    primitive: Block,
    slow: Block,
) {
    if checked {
        let uninitialized = tag_is(builder, source.tag, qjs::JS_TAG_UNINITIALIZED);
        let initialized = builder.create_block();
        builder
            .ins()
            .brif(uninitialized, slow, &[], initialized, &[]);
        builder.seal_block(initialized);
        builder.switch_to_block(initialized);
    }
    let refcounted = super::refcount::emit_has_ref_count(builder, source.tag);
    let duplicate = builder.create_block();
    if stress_primitives {
        let copy = builder.create_block();
        builder.ins().brif(refcounted, duplicate, &[], copy, &[]);
        builder.seal_block(copy);
        builder.switch_to_block(copy);
        let no_stress =
            super::refcount::emit_no_stress(builder, helpers.frame, helpers.layout.flags);
        builder.ins().brif(no_stress, primitive, &[], slow, &[]);
    } else {
        builder
            .ins()
            .brif(refcounted, duplicate, &[], primitive, &[]);
    }
    builder.seal_block(duplicate);
    builder.switch_to_block(duplicate);
    super::refcount::emit_dup_refcounted(
        builder,
        helpers.frame,
        helpers.layout.flags,
        source.payload,
        primitive,
        slow,
    );
}

#[allow(clippy::too_many_arguments)]
fn lower_dup_local_if_refcounted(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    state: FrameStateId,
    live_depth: usize,
    source: Pair,
    source_slot: u32,
    output_slot: u32,
    output: PairVars,
    output_base: Value,
    output_index: usize,
) -> Result<(), CompileFailure> {
    let primitive = builder.create_block();
    let slow = builder.create_block();
    let continuation = builder.create_block();
    emit_inline_dup_dispatch(builder, helpers, source, false, false, primitive, slow);
    builder.seal_block(primitive);
    builder.switch_to_block(primitive);
    define_pair(builder, output, source);
    builder.ins().jump(continuation, &[]);
    builder.seal_block(slow);
    builder.switch_to_block(slow);
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_DUP,
        state,
        live_depth,
        live_depth,
        &[output_slot, source_slot],
    )?;
    reload_pair(builder, output, output_base, output_index, helpers.layout);
    builder.ins().jump(continuation, &[]);
    builder.seal_block(continuation);
    builder.switch_to_block(continuation);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_free_if_refcounted(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    state: FrameStateId,
    live_depth: usize,
    value: Pair,
    value_slot: u32,
    variables: PairVars,
    backing_base: Value,
    backing_index: usize,
) -> Result<(), CompileFailure> {
    lower_free(
        builder,
        helpers,
        state,
        live_depth,
        value,
        value_slot,
        variables,
        backing_base,
        backing_index,
        false,
    )
}

/// FREE for sites that historically invoked the helper for every value:
/// outside stress GC primitives need no work, but stress-GC frames keep the
/// helper (and its collection points) for primitives too.
#[allow(clippy::too_many_arguments)]
fn lower_free_always_in_stress(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    state: FrameStateId,
    live_depth: usize,
    value: Pair,
    value_slot: u32,
    variables: PairVars,
    backing_base: Value,
    backing_index: usize,
) -> Result<(), CompileFailure> {
    lower_free(
        builder,
        helpers,
        state,
        live_depth,
        value,
        value_slot,
        variables,
        backing_base,
        backing_index,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn lower_free(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    state: FrameStateId,
    live_depth: usize,
    value: Pair,
    value_slot: u32,
    variables: PairVars,
    backing_base: Value,
    backing_index: usize,
    stress_primitives: bool,
) -> Result<(), CompileFailure> {
    let refcounted = super::refcount::emit_has_ref_count(builder, value.tag);
    let release = builder.create_block();
    let released = builder.create_block();
    let slow = builder.create_block();
    let continuation = builder.create_block();
    if stress_primitives {
        let primitive = builder.create_block();
        builder.ins().brif(refcounted, release, &[], primitive, &[]);
        builder.seal_block(primitive);
        builder.switch_to_block(primitive);
        let no_stress =
            super::refcount::emit_no_stress(builder, helpers.frame, helpers.layout.flags);
        builder.ins().brif(no_stress, continuation, &[], slow, &[]);
    } else {
        builder
            .ins()
            .brif(refcounted, release, &[], continuation, &[]);
    }
    builder.seal_block(release);
    builder.switch_to_block(release);
    super::refcount::emit_release_refcounted(
        builder,
        helpers.frame,
        helpers.layout.flags,
        value.payload,
        released,
        slow,
    );
    builder.seal_block(released);
    builder.switch_to_block(released);
    // Match the helper's post-state: the consumed slot holds undefined, in
    // the SSA variables and in its backing frame memory.
    let undefined = constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64));
    define_pair(builder, variables, undefined);
    store_jsvalue_slot(
        builder,
        backing_base,
        backing_index,
        undefined,
        helpers.layout,
    )?;
    builder.ins().jump(continuation, &[]);
    builder.seal_block(slow);
    builder.switch_to_block(slow);
    // FREE consumes its slot before it can fail (stress collection only).
    // A local or argument slot is then undefined in the frame while its SSA
    // variable still names the released value; a native handler would resume
    // with that stale alias. Such an edge exits instead, and the interpreter
    // unwinds from the frame itself. Operand slots are safe: the landing pad
    // resets every operand above the catch offset.
    let exception_target = helpers.exception_target.get();
    if backing_base != helpers.stack_base {
        helpers.exception_target.set(None);
    }
    let result = helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_FREE,
        state,
        live_depth,
        live_depth,
        &[value_slot],
    );
    helpers.exception_target.set(exception_target);
    result?;
    reload_pair(
        builder,
        variables,
        backing_base,
        backing_index,
        helpers.layout,
    );
    builder.ins().jump(continuation, &[]);
    builder.seal_block(continuation);
    builder.switch_to_block(continuation);
    Ok(())
}

fn lower_new_array(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: &mut usize,
    count: u16,
) -> Result<(), CompileFailure> {
    let count = usize::from(count);
    let input_base = depth
        .checked_sub(count)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let output_index = *depth;
    let output = flat_stack_slot(helpers.ir, output_index)?;
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_NEW_ARRAY,
        next_helper_state(states)?,
        *depth,
        *depth,
        &[output],
    )?;
    reload_pair(
        builder,
        helpers.stack[output_index],
        helpers.stack_base,
        output_index,
        helpers.layout,
    );
    if count == 0 {
        *depth = input_base + 1;
        return helpers.set_depth(builder, *depth);
    }

    /*
     * Keep the array in the first logical input slot before invoking a
     * setter. The displaced first element occupies the second scratch slot,
     * so either setter exit has no owning value in scratch: SET_PROPERTY
     * consumes that element on both success and exception.
     */
    let displaced_index = output_index
        .checked_add(1)
        .ok_or(CompileFailure::ResourceLimit)?;
    move_stack_pair(
        builder,
        helpers.stack,
        helpers.stack_base,
        input_base,
        displaced_index,
        helpers.layout,
    )?;
    move_stack_pair(
        builder,
        helpers.stack,
        helpers.stack_base,
        output_index,
        input_base,
        helpers.layout,
    )?;
    let array = flat_stack_slot(helpers.ir, input_base)?;

    for index in 0..count {
        let value_index = if index == 0 {
            displaced_index
        } else {
            input_base + index
        };
        let value = flat_stack_slot(helpers.ir, value_index)?;
        let atom =
            (1_u32 << 31) | u32::try_from(index).map_err(|_| CompileFailure::ResourceLimit)?;
        helpers.invoke(
            builder,
            qjs::JSJitHelperId_JS_JIT_HELPER_SET_PROPERTY,
            next_helper_state(states)?,
            *depth + 2,
            *depth,
            &[array, atom, value],
        )?;
        reload_pair(
            builder,
            helpers.stack[value_index],
            helpers.stack_base,
            value_index,
            helpers.layout,
        );
        reload_pair(
            builder,
            helpers.stack[input_base],
            helpers.stack_base,
            input_base,
            helpers.layout,
        );
    }
    for index in (input_base + 1)..=displaced_index {
        clear_pair(
            builder,
            helpers.stack[index],
            helpers.stack_base,
            index,
            helpers.layout,
        )?;
    }
    *depth = input_base + 1;
    helpers.set_depth(builder, *depth)
}

fn lower_get_property(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: usize,
    atom: u32,
    property: Option<&BaselinePropertySite>,
) -> Result<(), CompileFailure> {
    let object_index = depth
        .checked_sub(1)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let output_index = depth;
    let displaced_index = depth.checked_add(1).ok_or(CompileFailure::ResourceLimit)?;
    let object = flat_stack_slot(helpers.ir, object_index)?;
    let output = flat_stack_slot(helpers.ir, output_index)?;
    let get_state = next_helper_state(states)?;
    if let Some(property) = property {
        let generic = builder.create_block();
        let joined = builder.create_block();
        builder.append_block_param(joined, types::I64);
        builder.append_block_param(joined, types::I64);
        let receiver = use_pair(builder, helpers.stack[object_index]);
        let accesses = emit_shape_dispatch(
            builder,
            helpers.pointer_type,
            receiver,
            &property.observations,
            generic,
        )?;
        let layout = crate::abi::AbiInfo::linked()
            .map_err(|_| CompileFailure::InvalidArtifact)?
            .property_layout();
        for (observation, access) in property.observations.iter().copied().zip(accesses) {
            builder.switch_to_block(access);
            let props = builder.ins().load(
                helpers.pointer_type,
                MemFlags::trusted(),
                receiver.payload,
                layout.object_properties_offset,
            );
            let offset = property_slot_offset(observation)?;
            let value = Pair {
                payload: builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), props, offset),
                tag: builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), props, offset + 8),
            };
            let tag_ok = builder.ins().icmp_imm(
                IntCC::Equal,
                value.tag,
                i64::from(property_value_tag(observation.value())?),
            );
            let direct = builder.create_block();
            builder.ins().brif(tag_ok, direct, &[], generic, &[]);
            builder.switch_to_block(direct);
            builder.ins().jump(joined, &[value.payload, value.tag]);
        }
        builder.switch_to_block(generic);
        helpers.invoke(
            builder,
            qjs::JSJitHelperId_JS_JIT_HELPER_GET_PROPERTY,
            get_state,
            depth,
            depth,
            &[output, object, atom],
        )?;
        reload_pair(
            builder,
            helpers.stack[output_index],
            helpers.stack_base,
            output_index,
            helpers.layout,
        );
        clear_reloaded_output(builder, helpers, output_index)?;
        let value = use_pair(builder, helpers.stack[output_index]);
        builder.ins().jump(joined, &[value.payload, value.tag]);
        builder.switch_to_block(joined);
        let params = builder.block_params(joined);
        define_pair(
            builder,
            helpers.stack[output_index],
            Pair {
                payload: params[0],
                tag: params[1],
            },
        );
    } else {
        helpers.invoke(
            builder,
            qjs::JSJitHelperId_JS_JIT_HELPER_GET_PROPERTY,
            get_state,
            depth,
            depth,
            &[output, object, atom],
        )?;
        reload_pair(
            builder,
            helpers.stack[output_index],
            helpers.stack_base,
            output_index,
            helpers.layout,
        );
        clear_reloaded_output(builder, helpers, output_index)?;
    }
    move_stack_pair(
        builder,
        helpers.stack,
        helpers.stack_base,
        object_index,
        displaced_index,
        helpers.layout,
    )?;
    move_stack_pair(
        builder,
        helpers.stack,
        helpers.stack_base,
        output_index,
        object_index,
        helpers.layout,
    )?;
    let displaced = flat_stack_slot(helpers.ir, displaced_index)?;
    let displaced_value = use_pair(builder, helpers.stack[displaced_index]);
    lower_free_if_refcounted(
        builder,
        helpers,
        next_helper_state(states)?,
        depth + 2,
        displaced_value,
        displaced,
        helpers.stack[displaced_index],
        helpers.stack_base,
        displaced_index,
    )?;
    helpers.set_depth(builder, depth)
}

/// Byte offset of an own-property slot in `JSObject::prop`.
fn property_slot_offset(
    observation: crate::runtime::ShapeObservation,
) -> Result<i32, CompileFailure> {
    usize::try_from(observation.offset())
        .ok()
        .and_then(|offset| offset.checked_mul(16))
        .and_then(|offset| i32::try_from(offset).ok())
        .filter(|offset| offset.checked_add(8).is_some())
        .ok_or(CompileFailure::ResourceLimit)
}

/// Inline polymorphic own-property dispatch: one pointer compare per observed
/// shape, then the shape's monotonic layout generation. It is exactly the
/// SHAPE_GUARD helper predicate (the generation is zeroed on any layout
/// change and never reused), without a frame materialization or helper
/// crossing. A non-object receiver, an unknown shape or a stale generation
/// reaches `generic`. Returns one access block per observation, in feedback
/// order; the current block is terminated.
fn emit_shape_dispatch(
    builder: &mut FunctionBuilder<'_>,
    pointer_type: cranelift_codegen::ir::Type,
    receiver: Pair,
    observations: &[crate::runtime::ShapeObservation],
    generic: Block,
) -> Result<Vec<Block>, CompileFailure> {
    if observations.is_empty() || observations.len() > crate::runtime::POLYMORPHIC_PROPERTY_LIMIT {
        return Err(CompileFailure::InvalidArtifact);
    }
    for (index, observation) in observations.iter().enumerate() {
        if observation.shape().identity() == 0
            || observation.shape().generation() == 0
            || observations[..index]
                .iter()
                .any(|prior| prior.shape().identity() == observation.shape().identity())
        {
            return Err(CompileFailure::InvalidArtifact);
        }
    }
    let layout = crate::abi::AbiInfo::linked()
        .map_err(|_| CompileFailure::InvalidArtifact)?
        .property_layout();
    let is_object =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, receiver.tag, i64::from(qjs::JS_TAG_OBJECT));
    let dispatch = builder.create_block();
    builder.ins().brif(is_object, dispatch, &[], generic, &[]);
    builder.switch_to_block(dispatch);
    let shape = builder.ins().load(
        pointer_type,
        MemFlags::trusted(),
        receiver.payload,
        layout.object_shape_offset,
    );
    // A live object's shape is always valid; load its generation once.
    let generation = builder.ins().load(
        types::I64,
        MemFlags::trusted(),
        shape,
        layout.shape_generation_offset,
    );
    let mut accesses = Vec::with_capacity(observations.len());
    for (index, observation) in observations.iter().enumerate() {
        let next = if index + 1 == observations.len() {
            generic
        } else {
            builder.create_block()
        };
        let same_shape =
            builder
                .ins()
                .icmp_imm(IntCC::Equal, shape, observation.shape().identity() as i64);
        let current = builder.ins().icmp_imm(
            IntCC::Equal,
            generation,
            observation.shape().generation() as i64,
        );
        let matches = builder.ins().band(same_shape, current);
        let access = builder.create_block();
        builder.ins().brif(matches, access, &[], next, &[]);
        accesses.push(access);
        if index + 1 != observations.len() {
            builder.switch_to_block(next);
        }
    }
    Ok(accesses)
}

fn property_value_tag(value: crate::runtime::ObservedType) -> Result<i32, CompileFailure> {
    Ok(match value {
        crate::runtime::ObservedType::Int32 => qjs::JS_TAG_INT,
        crate::runtime::ObservedType::Float64 => qjs::JS_TAG_FLOAT64,
        crate::runtime::ObservedType::Bool => qjs::JS_TAG_BOOL,
        crate::runtime::ObservedType::Null => qjs::JS_TAG_NULL,
        crate::runtime::ObservedType::Undefined => qjs::JS_TAG_UNDEFINED,
        _ => return Err(CompileFailure::InvalidArtifact),
    })
}

/// Lower QuickJS `get_field2`: unlike `get_field`, the receiver remains on
/// the operand stack so the following `call_method` can use it as `this`.
/// GET_PROPERTY borrows the receiver and owns only its output, therefore no
/// refcount operation is required for the retained stack owner.
fn lower_get_property_keep(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: &mut usize,
    atom: u32,
) -> Result<(), CompileFailure> {
    let object_index = depth
        .checked_sub(1)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let object = flat_stack_slot(helpers.ir, object_index)?;
    let output = flat_stack_slot(helpers.ir, *depth)?;
    let state = next_helper_state(states)?;
    // `receiver.method` on a fast Array whose lookup resolves to a plain data
    // property of the realm's Array.prototype (for example `push`): the leaf
    // returns the duplicated value without running the generic lookup helper.
    let joined = helpers
        .object_fast
        .map(|fast| emit_array_method_fast_path(builder, helpers, fast, object_index, *depth, atom))
        .transpose()?;
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_GET_PROPERTY,
        state,
        *depth,
        *depth,
        &[output, object, atom],
    )?;
    reload_pair(
        builder,
        helpers.stack[*depth],
        helpers.stack_base,
        *depth,
        helpers.layout,
    );
    if let Some(joined) = joined {
        builder.ins().jump(joined, &[]);
        builder.seal_block(joined);
        builder.switch_to_block(joined);
    }
    *depth += 1;
    helpers.set_depth(builder, *depth)
}

/// Inline fast-Array class test followed by the `array_method` leaf. Leaves
/// the builder in the generic block and returns the join block.
fn emit_array_method_fast_path(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    fast: ObjectFastPaths,
    object_index: usize,
    output_index: usize,
    atom: u32,
) -> Result<Block, CompileFailure> {
    let pointer_type = helpers.pointer_type;
    let receiver = use_pair(builder, helpers.stack[object_index]);
    let class_check = builder.create_block();
    let call = builder.create_block();
    let hit = builder.create_block();
    let generic = builder.create_block();
    let joined = builder.create_block();
    let is_object = tag_is(builder, receiver.tag, qjs::JS_TAG_OBJECT);
    builder
        .ins()
        .brif(is_object, class_check, &[], generic, &[]);
    builder.seal_block(class_check);
    builder.switch_to_block(class_check);
    let class = builder.ins().load(
        types::I16,
        MemFlags::trusted(),
        receiver.payload,
        helpers.element_layout.object_class_id_offset,
    );
    let class = builder.ins().uextend(types::I64, class);
    let is_array =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, class, helpers.element_layout.array_class_id);
    builder.ins().brif(is_array, call, &[], generic, &[]);
    builder.seal_block(call);
    builder.switch_to_block(call);
    let scratch = object_scratch(builder, 2);
    builder.ins().stack_store(receiver.payload, scratch, 0);
    builder
        .ins()
        .stack_store(receiver.tag, scratch, helpers.layout.value_tag);
    let ctx = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        helpers.frame,
        helpers.layout.ctx,
    );
    let receiver_address = builder.ins().stack_addr(pointer_type, scratch, 0);
    let out_offset =
        i32::try_from(mem::size_of::<qjs::JSValue>()).map_err(|_| CompileFailure::ResourceLimit)?;
    let out_address = builder.ins().stack_addr(pointer_type, scratch, out_offset);
    let atom_value = builder.ins().iconst(types::I32, i64::from(atom));
    let status = emit_object_leaf_call(
        builder,
        pointer_type,
        fast.array_method,
        &[ctx, receiver_address, atom_value, out_address],
        &[scratch],
    );
    let succeeded = builder.ins().icmp_imm(
        IntCC::Equal,
        status,
        i64::from(qjs::JSJitObjectStatus_JS_JIT_OBJECT_OK),
    );
    builder.ins().brif(succeeded, hit, &[], generic, &[]);
    builder.seal_block(hit);
    builder.seal_block(generic);
    builder.switch_to_block(hit);
    let method = Pair {
        payload: builder.ins().stack_load(types::I64, scratch, out_offset),
        tag: builder
            .ins()
            .stack_load(types::I64, scratch, out_offset + helpers.layout.value_tag),
    };
    define_pair(builder, helpers.stack[output_index], method);
    builder.ins().jump(joined, &[]);
    builder.switch_to_block(generic);
    Ok(joined)
}

/// `Array.prototype.push` with one argument: the leaf mirrors
/// `js_array_push`'s fast-array path after verifying the exact built-in
/// function, and misses (generic CALL) on anything else. All inputs remain
/// borrowed, so the caller's interpreter-order cleanup is shared by both paths.
#[allow(clippy::too_many_arguments)]
fn emit_array_push_fast_path(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    fast: ObjectFastPaths,
    function_index: usize,
    this_index: usize,
    argument_index: usize,
    output_index: usize,
) -> Result<Block, CompileFailure> {
    let pointer_type = helpers.pointer_type;
    let function = use_pair(builder, helpers.stack[function_index]);
    let receiver = use_pair(builder, helpers.stack[this_index]);
    let argument = use_pair(builder, helpers.stack[argument_index]);
    let call = builder.create_block();
    let hit = builder.create_block();
    let generic = builder.create_block();
    let joined = builder.create_block();
    let function_is_object = tag_is(builder, function.tag, qjs::JS_TAG_OBJECT);
    let receiver_is_object = tag_is(builder, receiver.tag, qjs::JS_TAG_OBJECT);
    let objects = builder.ins().band(function_is_object, receiver_is_object);
    let class_check = builder.create_block();
    builder.ins().brif(objects, class_check, &[], generic, &[]);
    builder.seal_block(class_check);
    builder.switch_to_block(class_check);
    // Only Array receivers can hit; other method calls go straight to CALL.
    let class = builder.ins().load(
        types::I16,
        MemFlags::trusted(),
        receiver.payload,
        helpers.element_layout.object_class_id_offset,
    );
    let class = builder.ins().uextend(types::I64, class);
    let is_array =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, class, helpers.element_layout.array_class_id);
    builder.ins().brif(is_array, call, &[], generic, &[]);
    builder.seal_block(call);
    builder.switch_to_block(call);
    let value_size = mem::size_of::<qjs::JSValue>();
    let scratch = object_scratch(builder, 4);
    for (index, pair) in [function, receiver, argument].into_iter().enumerate() {
        let offset =
            i32::try_from(index * value_size).map_err(|_| CompileFailure::ResourceLimit)?;
        builder.ins().stack_store(pair.payload, scratch, offset);
        builder
            .ins()
            .stack_store(pair.tag, scratch, offset + helpers.layout.value_tag);
    }
    let ctx = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        helpers.frame,
        helpers.layout.ctx,
    );
    let mut addresses = Vec::with_capacity(4);
    for index in 0..4 {
        let offset =
            i32::try_from(index * value_size).map_err(|_| CompileFailure::ResourceLimit)?;
        addresses.push(builder.ins().stack_addr(pointer_type, scratch, offset));
    }
    let status = emit_object_leaf_call(
        builder,
        pointer_type,
        fast.array_push,
        &[ctx, addresses[0], addresses[1], addresses[2], addresses[3]],
        &[scratch],
    );
    let succeeded = builder.ins().icmp_imm(
        IntCC::Equal,
        status,
        i64::from(qjs::JSJitObjectStatus_JS_JIT_OBJECT_OK),
    );
    builder.ins().brif(succeeded, hit, &[], generic, &[]);
    builder.seal_block(hit);
    builder.seal_block(generic);
    builder.switch_to_block(hit);
    let out_offset = i32::try_from(3 * value_size).map_err(|_| CompileFailure::ResourceLimit)?;
    let length = Pair {
        payload: builder.ins().stack_load(types::I64, scratch, out_offset),
        tag: builder
            .ins()
            .stack_load(types::I64, scratch, out_offset + helpers.layout.value_tag),
    };
    define_pair(builder, helpers.stack[output_index], length);
    builder.ins().jump(joined, &[]);
    builder.switch_to_block(generic);
    Ok(joined)
}

fn lower_set_property(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: &mut usize,
    atom: u32,
    property: Option<&BaselinePropertySite>,
) -> Result<(), CompileFailure> {
    let object_index = depth
        .checked_sub(2)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let value_index = *depth - 1;
    let object = flat_stack_slot(helpers.ir, object_index)?;
    let value = flat_stack_slot(helpers.ir, value_index)?;
    let set_state = next_helper_state(states)?;
    if let Some(property) = property {
        let generic = builder.create_block();
        let joined = builder.create_block();
        let receiver = use_pair(builder, helpers.stack[object_index]);
        let accesses = emit_shape_dispatch(
            builder,
            helpers.pointer_type,
            receiver,
            &property.observations,
            generic,
        )?;
        let layout = crate::abi::AbiInfo::linked()
            .map_err(|_| CompileFailure::InvalidArtifact)?
            .property_layout();
        for (observation, access) in property.observations.iter().copied().zip(accesses) {
            builder.switch_to_block(access);
            let props = builder.ins().load(
                helpers.pointer_type,
                MemFlags::trusted(),
                receiver.payload,
                layout.object_properties_offset,
            );
            let offset = property_slot_offset(observation)?;
            let expected_tag = property_value_tag(observation.value())?;
            let current_tag =
                builder
                    .ins()
                    .load(types::I64, MemFlags::trusted(), props, offset + 8);
            let input = use_pair(builder, helpers.stack[value_index]);
            let current_ok =
                builder
                    .ins()
                    .icmp_imm(IntCC::Equal, current_tag, i64::from(expected_tag));
            let input_ok = builder
                .ins()
                .icmp_imm(IntCC::Equal, input.tag, i64::from(expected_tag));
            let tags_ok = builder.ins().band(current_ok, input_ok);
            let direct = builder.create_block();
            builder.ins().brif(tags_ok, direct, &[], generic, &[]);
            builder.switch_to_block(direct);
            builder
                .ins()
                .store(MemFlags::trusted(), input.payload, props, offset);
            builder
                .ins()
                .store(MemFlags::trusted(), input.tag, props, offset + 8);
            clear_pair(
                builder,
                helpers.stack[value_index],
                helpers.stack_base,
                value_index,
                helpers.layout,
            )?;
            builder.ins().jump(joined, &[]);
        }
        builder.switch_to_block(generic);
        helpers.invoke(
            builder,
            qjs::JSJitHelperId_JS_JIT_HELPER_SET_PROPERTY,
            set_state,
            *depth,
            *depth,
            &[object, atom, value],
        )?;
        reload_pair(
            builder,
            helpers.stack[value_index],
            helpers.stack_base,
            value_index,
            helpers.layout,
        );
        builder.ins().jump(joined, &[]);
        builder.switch_to_block(joined);
    } else {
        helpers.invoke(
            builder,
            qjs::JSJitHelperId_JS_JIT_HELPER_SET_PROPERTY,
            set_state,
            *depth,
            *depth,
            &[object, atom, value],
        )?;
        reload_pair(
            builder,
            helpers.stack[value_index],
            helpers.stack_base,
            value_index,
            helpers.layout,
        );
    }
    let object_value = use_pair(builder, helpers.stack[object_index]);
    lower_free_if_refcounted(
        builder,
        helpers,
        next_helper_state(states)?,
        *depth,
        object_value,
        object,
        helpers.stack[object_index],
        helpers.stack_base,
        object_index,
    )?;
    *depth = object_index;
    helpers.set_depth(builder, *depth)
}

fn lower_define_property(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: &mut usize,
    atom: u32,
) -> Result<(), CompileFailure> {
    let object_index = depth
        .checked_sub(2)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let value_index = *depth - 1;
    let object = flat_stack_slot(helpers.ir, object_index)?;
    let value = flat_stack_slot(helpers.ir, value_index)?;
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_SET_PROPERTY,
        next_helper_state(states)?,
        *depth,
        *depth,
        &[object, atom, value],
    )?;
    reload_pair(
        builder,
        helpers.stack[value_index],
        helpers.stack_base,
        value_index,
        helpers.layout,
    );
    *depth = object_index + 1;
    helpers.set_depth(builder, *depth)
}

fn lower_define_element(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: &mut usize,
) -> Result<(), CompileFailure> {
    let object_index = depth
        .checked_sub(3)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let key_index = object_index + 1;
    let value_index = object_index + 2;
    let key_copy_index = *depth;
    let value_copy_index = depth.checked_add(1).ok_or(CompileFailure::ResourceLimit)?;

    move_stack_pair(
        builder,
        helpers.stack,
        helpers.stack_base,
        value_index,
        value_copy_index,
        helpers.layout,
    )?;
    let object = flat_stack_slot(helpers.ir, object_index)?;
    let key = flat_stack_slot(helpers.ir, key_index)?;
    let object_copy = flat_stack_slot(helpers.ir, value_index)?;
    let key_copy = flat_stack_slot(helpers.ir, key_copy_index)?;
    let value_copy = flat_stack_slot(helpers.ir, value_copy_index)?;
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_DUP,
        next_helper_state(states)?,
        *depth + 2,
        *depth,
        &[object_copy, object],
    )?;
    reload_pair(
        builder,
        helpers.stack[value_index],
        helpers.stack_base,
        value_index,
        helpers.layout,
    );
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_DUP,
        next_helper_state(states)?,
        *depth + 2,
        *depth,
        &[key_copy, key],
    )?;
    reload_pair(
        builder,
        helpers.stack[key_copy_index],
        helpers.stack_base,
        key_copy_index,
        helpers.layout,
    );
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_SET_ELEMENT,
        next_helper_state(states)?,
        *depth + 2,
        *depth,
        &[object_copy, key_copy, value_copy],
    )?;
    for index in value_index..=value_copy_index {
        reload_pair(
            builder,
            helpers.stack[index],
            helpers.stack_base,
            index,
            helpers.layout,
        );
    }
    *depth = object_index + 2;
    helpers.set_depth(builder, *depth)
}

fn lower_get_element(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: &mut usize,
    element_layout: crate::abi::ElementLayout,
) -> Result<(), CompileFailure> {
    let object_index = depth
        .checked_sub(2)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let key_index = *depth - 1;
    let object = flat_stack_slot(helpers.ir, object_index)?;
    let key = flat_stack_slot(helpers.ir, key_index)?;
    let state = next_helper_state(states)?;
    let array_free_state = next_helper_state(states)?;
    let int32_free_state = next_helper_state(states)?;
    let float64_free_state = next_helper_state(states)?;
    let object_value = use_pair(builder, helpers.stack[object_index]);
    let key_value = use_pair(builder, helpers.stack[key_index]);
    let direct = builder.create_block();
    let generic = builder.create_block();
    let joined = builder.create_block();
    let object_is_object = tag_is(builder, object_value.tag, qjs::JS_TAG_OBJECT);
    let key_is_int = tag_is(builder, key_value.tag, qjs::JS_TAG_INT);
    let is_integer_key = builder.ins().band(object_is_object, key_is_int);
    builder
        .ins()
        .brif(is_integer_key, direct, &[], generic, &[]);

    builder.switch_to_block(direct);
    let index = builder.ins().ireduce(types::I32, key_value.payload);
    element_guard!(
        builder,
        builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, index, 0),
        generic,
    );
    let flags = builder.ins().load(
        types::I8,
        MemFlags::new(),
        object_value.payload,
        element_layout.object_flags_offset,
    );
    let fast = builder
        .ins()
        .band_imm(flags, element_layout.object_fast_array_mask);
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, fast, 0),
        generic,
    );
    let class = builder.ins().load(
        types::I16,
        MemFlags::new(),
        object_value.payload,
        element_layout.object_class_id_offset,
    );
    let class = builder.ins().uextend(types::I64, class);
    let array = builder.create_block();
    let int32 = builder.create_block();
    let float64 = builder.create_block();
    let not_array = builder.create_block();
    let is_array = builder
        .ins()
        .icmp_imm(IntCC::Equal, class, element_layout.array_class_id);
    builder.ins().brif(is_array, array, &[], not_array, &[]);
    builder.switch_to_block(not_array);
    let is_int32 = builder
        .ins()
        .icmp_imm(IntCC::Equal, class, element_layout.int32_array_class_id);
    let not_int32 = builder.create_block();
    builder.ins().brif(is_int32, int32, &[], not_int32, &[]);
    builder.switch_to_block(not_int32);
    let is_float64 =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, class, element_layout.float64_array_class_id);
    builder.ins().brif(is_float64, float64, &[], generic, &[]);

    builder.switch_to_block(array);
    let count = builder.ins().load(
        types::I32,
        MemFlags::new(),
        object_value.payload,
        element_layout.array_count_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp(IntCC::UnsignedLessThan, index, count),
        generic,
    );
    let data = builder.ins().load(
        helpers.pointer_type,
        MemFlags::new(),
        object_value.payload,
        element_layout.array_data_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, data, 0),
        generic,
    );
    let value = load_element_jsvalue(builder, data, index, helpers.layout);
    element_guard!(
        builder,
        builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, value.tag, 0),
        generic,
    );
    lower_free_if_refcounted(
        builder,
        helpers,
        array_free_state,
        *depth,
        object_value,
        object,
        helpers.stack[object_index],
        helpers.stack_base,
        object_index,
    )?;
    define_pair(builder, helpers.stack[object_index], value);
    builder.ins().jump(joined, &[]);

    lower_typed_element_get(
        builder,
        helpers,
        int32_free_state,
        *depth,
        object_index,
        object,
        object_value,
        index,
        int32,
        generic,
        joined,
        element_layout,
        ElementKind::Int32,
    )?;
    lower_typed_element_get(
        builder,
        helpers,
        float64_free_state,
        *depth,
        object_index,
        object,
        object_value,
        index,
        float64,
        generic,
        joined,
        element_layout,
        ElementKind::Float64,
    )?;

    builder.seal_block(generic);
    builder.switch_to_block(generic);
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_GET_ELEMENT,
        state,
        *depth,
        *depth,
        &[object, object, key],
    )?;
    reload_pair(
        builder,
        helpers.stack[object_index],
        helpers.stack_base,
        object_index,
        helpers.layout,
    );
    reload_pair(
        builder,
        helpers.stack[key_index],
        helpers.stack_base,
        key_index,
        helpers.layout,
    );
    builder.ins().jump(joined, &[]);
    builder.seal_block(joined);
    builder.switch_to_block(joined);
    *depth = key_index;
    helpers.set_depth(builder, *depth)
}

fn lower_set_element(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: &mut usize,
    element_layout: crate::abi::ElementLayout,
) -> Result<(), CompileFailure> {
    let object_index = depth
        .checked_sub(3)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let key_index = object_index + 1;
    let value_index = object_index + 2;
    let object = flat_stack_slot(helpers.ir, object_index)?;
    let key = flat_stack_slot(helpers.ir, key_index)?;
    let value = flat_stack_slot(helpers.ir, value_index)?;
    let state = next_helper_state(states)?;
    let array_free_state = next_helper_state(states)?;
    let int32_free_state = next_helper_state(states)?;
    let float64_free_state = next_helper_state(states)?;
    let object_value = use_pair(builder, helpers.stack[object_index]);
    let key_value = use_pair(builder, helpers.stack[key_index]);
    let value_value = use_pair(builder, helpers.stack[value_index]);
    let direct = builder.create_block();
    let generic = builder.create_block();
    let joined = builder.create_block();
    let object_is_object = tag_is(builder, object_value.tag, qjs::JS_TAG_OBJECT);
    let key_is_int = tag_is(builder, key_value.tag, qjs::JS_TAG_INT);
    let is_integer_key = builder.ins().band(object_is_object, key_is_int);
    builder
        .ins()
        .brif(is_integer_key, direct, &[], generic, &[]);

    builder.switch_to_block(direct);
    let index = builder.ins().ireduce(types::I32, key_value.payload);
    element_guard!(
        builder,
        builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, index, 0),
        generic,
    );
    let flags = builder.ins().load(
        types::I8,
        MemFlags::new(),
        object_value.payload,
        element_layout.object_flags_offset,
    );
    let fast = builder
        .ins()
        .band_imm(flags, element_layout.object_fast_array_mask);
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, fast, 0),
        generic,
    );
    let class = builder.ins().load(
        types::I16,
        MemFlags::new(),
        object_value.payload,
        element_layout.object_class_id_offset,
    );
    let class = builder.ins().uextend(types::I64, class);
    let array = builder.create_block();
    let int32 = builder.create_block();
    let float64 = builder.create_block();
    let not_array = builder.create_block();
    let is_array = builder
        .ins()
        .icmp_imm(IntCC::Equal, class, element_layout.array_class_id);
    builder.ins().brif(is_array, array, &[], not_array, &[]);
    builder.switch_to_block(not_array);
    let is_int32 = builder
        .ins()
        .icmp_imm(IntCC::Equal, class, element_layout.int32_array_class_id);
    let not_int32 = builder.create_block();
    builder.ins().brif(is_int32, int32, &[], not_int32, &[]);
    builder.switch_to_block(not_int32);
    let is_float64 =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, class, element_layout.float64_array_class_id);
    builder.ins().brif(is_float64, float64, &[], generic, &[]);

    builder.switch_to_block(array);
    let count = builder.ins().load(
        types::I32,
        MemFlags::new(),
        object_value.payload,
        element_layout.array_count_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp(IntCC::UnsignedLessThan, index, count),
        generic,
    );
    element_guard!(
        builder,
        builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, value_value.tag, 0),
        generic,
    );
    let data = builder.ins().load(
        helpers.pointer_type,
        MemFlags::new(),
        object_value.payload,
        element_layout.array_data_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, data, 0),
        generic,
    );
    let old = load_element_jsvalue(builder, data, index, helpers.layout);
    element_guard!(
        builder,
        builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, old.tag, 0),
        generic,
    );
    store_element_jsvalue(builder, data, index, value_value, helpers.layout);
    finish_direct_element_set(
        builder,
        helpers,
        array_free_state,
        *depth,
        object_index,
        key_index,
        value_index,
        object,
        object_value,
        joined,
    )?;

    lower_typed_element_set(
        builder,
        helpers,
        int32_free_state,
        *depth,
        object_index,
        key_index,
        value_index,
        object,
        object_value,
        value_value,
        index,
        int32,
        generic,
        joined,
        element_layout,
        ElementKind::Int32,
    )?;
    lower_typed_element_set(
        builder,
        helpers,
        float64_free_state,
        *depth,
        object_index,
        key_index,
        value_index,
        object,
        object_value,
        value_value,
        index,
        float64,
        generic,
        joined,
        element_layout,
        ElementKind::Float64,
    )?;

    builder.seal_block(generic);
    builder.switch_to_block(generic);
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_SET_ELEMENT,
        state,
        *depth,
        *depth,
        &[object, key, value],
    )?;
    for index in object_index..=*depth - 1 {
        reload_pair(
            builder,
            helpers.stack[index],
            helpers.stack_base,
            index,
            helpers.layout,
        );
    }
    builder.ins().jump(joined, &[]);
    builder.seal_block(joined);
    builder.switch_to_block(joined);
    *depth = object_index;
    helpers.set_depth(builder, *depth)
}

#[derive(Clone, Copy)]
enum ElementKind {
    Int32,
    Float64,
}

fn emit_element_guard(builder: &mut FunctionBuilder<'_>, condition: Value, fallback: Block) {
    let success = builder.create_block();
    builder.ins().brif(condition, success, &[], fallback, &[]);
    builder.seal_block(success);
    builder.switch_to_block(success);
}

fn element_address(
    builder: &mut FunctionBuilder<'_>,
    base: Value,
    index: Value,
    scale: i64,
    pointer_type: cranelift_codegen::ir::Type,
) -> Value {
    let index = builder.ins().uextend(pointer_type, index);
    let bytes = builder.ins().imul_imm(index, scale);
    builder.ins().iadd(base, bytes)
}

fn load_element_jsvalue(
    builder: &mut FunctionBuilder<'_>,
    data: Value,
    index: Value,
    layout: FrameLayout,
) -> Pair {
    let address = element_address(builder, data, index, 16, types::I64);
    Pair {
        payload: builder.ins().load(types::I64, MemFlags::new(), address, 0),
        tag: builder
            .ins()
            .load(types::I64, MemFlags::new(), address, layout.value_tag),
    }
}

fn store_element_jsvalue(
    builder: &mut FunctionBuilder<'_>,
    data: Value,
    index: Value,
    value: Pair,
    layout: FrameLayout,
) {
    let address = element_address(builder, data, index, 16, types::I64);
    builder
        .ins()
        .store(MemFlags::new(), value.payload, address, 0);
    builder
        .ins()
        .store(MemFlags::new(), value.tag, address, layout.value_tag);
}

fn typed_element_data(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    object: Pair,
    index: Value,
    fallback: Block,
    element_layout: crate::abi::ElementLayout,
) -> Value {
    let typed = builder.ins().load(
        helpers.pointer_type,
        MemFlags::new(),
        object.payload,
        element_layout.typed_array_ptr_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, typed, 0),
        fallback,
    );
    let tracks_resizable = builder.ins().load(
        types::I8,
        MemFlags::new(),
        typed,
        element_layout.typed_array_track_rab_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::Equal, tracks_resizable, 0),
        fallback,
    );
    let buffer = builder.ins().load(
        helpers.pointer_type,
        MemFlags::new(),
        typed,
        element_layout.typed_array_buffer_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, buffer, 0),
        fallback,
    );
    let array_buffer = builder.ins().load(
        helpers.pointer_type,
        MemFlags::new(),
        buffer,
        element_layout.object_union_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, array_buffer, 0),
        fallback,
    );
    let detached = builder.ins().load(
        types::I8,
        MemFlags::new(),
        array_buffer,
        element_layout.array_buffer_detached_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::Equal, detached, 0),
        fallback,
    );
    let backing_data = builder.ins().load(
        helpers.pointer_type,
        MemFlags::new(),
        array_buffer,
        element_layout.array_buffer_data_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, backing_data, 0),
        fallback,
    );
    let count = builder.ins().load(
        types::I32,
        MemFlags::new(),
        object.payload,
        element_layout.array_count_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp(IntCC::UnsignedLessThan, index, count),
        fallback,
    );
    let data = builder.ins().load(
        helpers.pointer_type,
        MemFlags::new(),
        object.payload,
        element_layout.array_data_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, data, 0),
        fallback,
    );
    data
}

fn typed_element_is_mutable(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    object: Pair,
    fallback: Block,
    element_layout: crate::abi::ElementLayout,
) {
    let typed = builder.ins().load(
        helpers.pointer_type,
        MemFlags::new(),
        object.payload,
        element_layout.typed_array_ptr_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, typed, 0),
        fallback,
    );
    let buffer = builder.ins().load(
        helpers.pointer_type,
        MemFlags::new(),
        typed,
        element_layout.typed_array_buffer_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, buffer, 0),
        fallback,
    );
    let array_buffer = builder.ins().load(
        helpers.pointer_type,
        MemFlags::new(),
        buffer,
        element_layout.object_union_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::NotEqual, array_buffer, 0),
        fallback,
    );
    let immutable = builder.ins().load(
        types::I8,
        MemFlags::new(),
        array_buffer,
        element_layout.array_buffer_immutable_offset,
    );
    element_guard!(
        builder,
        builder.ins().icmp_imm(IntCC::Equal, immutable, 0),
        fallback,
    );
}

#[allow(clippy::too_many_arguments)]
fn lower_typed_element_get(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    state: FrameStateId,
    depth: usize,
    object_index: usize,
    object_slot: u32,
    object: Pair,
    index: Value,
    block: Block,
    fallback: Block,
    joined: Block,
    element_layout: crate::abi::ElementLayout,
    kind: ElementKind,
) -> Result<(), CompileFailure> {
    builder.switch_to_block(block);
    let data = typed_element_data(builder, helpers, object, index, fallback, element_layout);
    let value = match kind {
        ElementKind::Int32 => {
            let address = element_address(builder, data, index, 4, helpers.pointer_type);
            let raw = builder.ins().load(types::I32, MemFlags::new(), address, 0);
            Pair {
                payload: builder.ins().sextend(types::I64, raw),
                tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
            }
        }
        ElementKind::Float64 => {
            let address = element_address(builder, data, index, 8, helpers.pointer_type);
            let raw = builder.ins().load(types::F64, MemFlags::new(), address, 0);
            Pair {
                payload: builder.ins().bitcast(types::I64, MemFlags::new(), raw),
                tag: builder
                    .ins()
                    .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64)),
            }
        }
    };
    lower_free_if_refcounted(
        builder,
        helpers,
        state,
        depth,
        object,
        object_slot,
        helpers.stack[object_index],
        helpers.stack_base,
        object_index,
    )?;
    define_pair(builder, helpers.stack[object_index], value);
    builder.ins().jump(joined, &[]);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_typed_element_set(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    state: FrameStateId,
    depth: usize,
    object_index: usize,
    key_index: usize,
    value_index: usize,
    object_slot: u32,
    object: Pair,
    value: Pair,
    index: Value,
    block: Block,
    fallback: Block,
    joined: Block,
    element_layout: crate::abi::ElementLayout,
    kind: ElementKind,
) -> Result<(), CompileFailure> {
    builder.switch_to_block(block);
    let data = typed_element_data(builder, helpers, object, index, fallback, element_layout);
    typed_element_is_mutable(builder, helpers, object, fallback, element_layout);
    match kind {
        ElementKind::Int32 => {
            element_guard!(
                builder,
                tag_is(builder, value.tag, qjs::JS_TAG_INT),
                fallback
            );
            let address = element_address(builder, data, index, 4, helpers.pointer_type);
            let narrowed = builder.ins().ireduce(types::I32, value.payload);
            builder.ins().store(MemFlags::new(), narrowed, address, 0);
        }
        ElementKind::Float64 => {
            element_guard!(builder, emit_numeric_tag(builder, value.tag), fallback);
            let (_, numeric) = emit_numeric(builder, value);
            let address = element_address(builder, data, index, 8, helpers.pointer_type);
            builder.ins().store(MemFlags::new(), numeric, address, 0);
        }
    }
    finish_direct_element_set(
        builder,
        helpers,
        state,
        depth,
        object_index,
        key_index,
        value_index,
        object_slot,
        object,
        joined,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish_direct_element_set(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    state: FrameStateId,
    depth: usize,
    object_index: usize,
    key_index: usize,
    value_index: usize,
    object_slot: u32,
    object: Pair,
    joined: Block,
) -> Result<(), CompileFailure> {
    lower_free_if_refcounted(
        builder,
        helpers,
        state,
        depth,
        object,
        object_slot,
        helpers.stack[object_index],
        helpers.stack_base,
        object_index,
    )?;
    clear_pair(
        builder,
        helpers.stack[object_index],
        helpers.stack_base,
        object_index,
        helpers.layout,
    )?;
    clear_pair(
        builder,
        helpers.stack[key_index],
        helpers.stack_base,
        key_index,
        helpers.layout,
    )?;
    clear_pair(
        builder,
        helpers.stack[value_index],
        helpers.stack_base,
        value_index,
        helpers.layout,
    )?;
    builder.ins().jump(joined, &[]);
    Ok(())
}

fn lower_to_property_key(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: usize,
) -> Result<(), CompileFailure> {
    let index = depth
        .checked_sub(1)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let slot = flat_stack_slot(helpers.ir, index)?;
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_TO_PROPKEY,
        next_helper_state(states)?,
        depth,
        depth,
        &[slot, slot],
    )?;
    reload_pair(
        builder,
        helpers.stack[index],
        helpers.stack_base,
        index,
        helpers.layout,
    );
    helpers.set_depth(builder, depth)
}

/// Synchronous iteration opcodes run the interpreter's own stack effect on
/// the materialized frame through ITERATOR_OP. `for_of_next` first tries the
/// non-allocating Array values leaf, which revalidates the live iterator, its
/// exact built-in `next` method and the dense in-bounds element on every step;
/// any miss (holes past the dense count, non-Array receivers, user iterators,
/// completion) takes the exact helper instead.
fn lower_iterator(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    state: FrameStateId,
    depth: &mut usize,
    operation: IteratorOp,
    array_values_next: Option<usize>,
) -> Result<(), CompileFailure> {
    let stack = helpers.stack;
    let before = *depth;
    let base = before
        .checked_sub(operation.window_below())
        .ok_or(CompileFailure::InvalidArtifact)?;
    let after = before
        .checked_add_signed(operation.net_push())
        .ok_or(CompileFailure::InvalidArtifact)?;
    if before.max(after) > stack.len() {
        return Err(CompileFailure::InvalidArtifact);
    }
    let (name, operand) = match operation {
        IteratorOp::ForOfStart => ("for_of_start", 0),
        IteratorOp::ForOfNext(extra) => ("for_of_next", u32::from(extra)),
        IteratorOp::ForInStart => ("for_in_start", 0),
        IteratorOp::ForInNext => ("for_in_next", 0),
        IteratorOp::Close => ("iterator_close", 0),
    };
    // The interpreter's `sp` when the operation throws: iterator_close has
    // already dropped the catch offset and the next method.
    let exception_depth = match operation {
        IteratorOp::Close => before - 2,
        _ => before,
    };
    let arguments = [
        u32::try_from(before).map_err(|_| CompileFailure::ResourceLimit)?,
        quickjs_opcode_id(name)?,
        operand,
        0,
    ];

    let continuation = match (operation, array_values_next) {
        (IteratorOp::ForOfNext(_), Some(leaf)) => {
            let continuation = builder.create_block();
            let hit = builder.create_block();
            let miss = builder.create_block();
            let scratch = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                u32::try_from(3 * mem::size_of::<qjs::JSValue>())
                    .map_err(|_| CompileFailure::ResourceLimit)?,
                3,
            ));
            let scratch = builder.ins().stack_addr(helpers.pointer_type, scratch, 0);
            let iterator = use_pair(builder, stack[base]);
            let next_method = use_pair(builder, stack[base + 1]);
            store_jsvalue_slot(builder, scratch, 0, iterator, helpers.layout)?;
            store_jsvalue_slot(builder, scratch, 1, next_method, helpers.layout)?;
            let mut signature = Signature::new(builder.func.signature.call_conv);
            signature.params.extend([
                AbiParam::new(helpers.pointer_type),
                AbiParam::new(helpers.pointer_type),
                AbiParam::new(helpers.pointer_type),
                AbiParam::new(helpers.pointer_type),
            ]);
            signature.returns.push(AbiParam::new(types::I32));
            let signature = builder.import_signature(signature);
            let target = builder.ins().iconst(
                helpers.pointer_type,
                i64::try_from(leaf).map_err(|_| CompileFailure::ResourceLimit)?,
            );
            let ctx = builder.ins().load(
                helpers.pointer_type,
                MemFlags::new(),
                helpers.frame,
                helpers.layout.ctx,
            );
            let value_bytes = i64::try_from(mem::size_of::<qjs::JSValue>())
                .map_err(|_| CompileFailure::ResourceLimit)?;
            let next_address = builder.ins().iadd_imm(scratch, value_bytes);
            let out_address = builder.ins().iadd_imm(scratch, 2 * value_bytes);
            emit_msan_unpoison(
                builder,
                helpers.pointer_type,
                scratch,
                2 * mem::size_of::<qjs::JSValue>(),
            );
            let call = super::emit_external_call(
                builder,
                signature,
                target,
                &[ctx, scratch, next_address, out_address],
                helpers.pointer_type,
                None,
                None,
            );
            let status = builder.inst_results(call)[0];
            let produced = builder.ins().icmp_imm(
                IntCC::Equal,
                status,
                i64::from(qjs::JSJitIteratorQueryStatus_JS_JIT_ITERATOR_QUERY_VALUE),
            );
            builder.ins().brif(produced, hit, &[], miss, &[]);
            builder.seal_block(hit);
            builder.seal_block(miss);

            builder.switch_to_block(hit);
            let value = load_jsvalue(builder, scratch, 2, helpers.layout);
            define_pair(builder, stack[before], value);
            let not_done = constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_BOOL as i64));
            define_pair(builder, stack[before + 1], not_done);
            builder.ins().jump(continuation, &[]);

            builder.switch_to_block(miss);
            Some(continuation)
        }
        _ => None,
    };

    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_ITERATOR_OP,
        state,
        before,
        exception_depth,
        &arguments,
    )?;
    let reload = |builder: &mut FunctionBuilder<'_>, index: usize| {
        reload_pair(
            builder,
            stack[index],
            helpers.stack_base,
            index,
            helpers.layout,
        )
    };
    match operation {
        IteratorOp::ForOfStart => {
            for index in base..after {
                reload(builder, index);
            }
        }
        IteratorOp::ForInStart => reload(builder, base),
        IteratorOp::ForOfNext(_) | IteratorOp::ForInNext => {
            reload(builder, base);
            reload(builder, before);
            reload(builder, before + 1);
        }
        IteratorOp::Close => {
            for (index, pair) in stack.iter().copied().enumerate().take(before).skip(base) {
                clear_pair(builder, pair, helpers.stack_base, index, helpers.layout)?;
            }
        }
    }
    if let Some(continuation) = continuation {
        builder.ins().jump(continuation, &[]);
        builder.seal_block(continuation);
        builder.switch_to_block(continuation);
    }
    *depth = after;
    Ok(())
}

#[allow(clippy::too_many_arguments)] // Call bytecode operands and lowering state are separate by design.
fn lower_call(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    states: &mut impl Iterator<Item = FrameStateId>,
    depth: &mut usize,
    argc: u16,
    has_this: bool,
    is_constructor: bool,
    direct: Option<&BaselineDirectCallSite>,
) -> Result<(), CompileFailure> {
    let argc = usize::from(argc);
    let pop = argc + 1 + usize::from(has_this);
    let base = depth
        .checked_sub(pop)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let this_index = if has_this { base } else { *depth };
    let function_index = if has_this { base + 1 } else { base };
    let argv_index = function_index + 1;
    let output_index = if has_this { *depth } else { *depth + 1 };
    if !has_this {
        let undefined = constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64));
        define_pair(builder, helpers.stack[this_index], undefined);
    }
    let output = flat_stack_slot(helpers.ir, output_index)?;
    let function = flat_stack_slot(helpers.ir, function_index)?;
    let this_value = flat_stack_slot(helpers.ir, this_index)?;
    let argv = if argc == 0 {
        u32::MAX
    } else {
        flat_stack_slot(helpers.ir, argv_index)?
    };
    let call_live_depth = output_index;
    let call_state = next_helper_state(states)?;
    let mut direct_hit = None;
    if let Some(direct) = direct.filter(|target| {
        !has_this
            && !is_constructor
            && target.call.arity() == argc
            && target.call.callee_identity() != 0
            && target.call.callee_bytecode_identity() != 0
    }) {
        use crate::runtime::FeedbackRepresentation;
        use cranelift_codegen::ir::condcodes::IntCC;
        use cranelift_codegen::ir::{AbiParam, Signature, StackSlotData, StackSlotKind};

        let slow = builder.create_block();
        let signature = builder.create_block();
        let invoke = builder.create_block();
        let direct_done = builder.create_block();
        let joined = builder.create_block();
        builder.append_block_param(joined, types::I8);
        let callable = use_pair(builder, helpers.stack[function_index]);
        super::emit_guarded_direct_callee_identity(
            builder,
            callable.tag,
            callable.payload,
            super::DirectCalleeIdentity {
                object: direct.call.callee_identity(),
                bytecode: direct.call.callee_bytecode_identity(),
            },
            helpers.pointer_type,
            signature,
            slow,
        );
        builder.switch_to_block(signature);
        let mut signature_matches = builder.ins().iconst(types::I8, 1);
        for (index, representation) in direct.call.arguments().iter().enumerate() {
            let argument = use_pair(builder, helpers.stack[argv_index + index]);
            let tag = match representation {
                FeedbackRepresentation::Int32 => qjs::JS_TAG_INT,
                FeedbackRepresentation::Float64 => qjs::JS_TAG_FLOAT64,
                FeedbackRepresentation::Bool => qjs::JS_TAG_BOOL,
                FeedbackRepresentation::HeapRef => qjs::JS_TAG_OBJECT,
            };
            let typed = builder
                .ins()
                .icmp_imm(IntCC::Equal, argument.tag, i64::from(tag));
            signature_matches = builder.ins().band(signature_matches, typed);
        }
        builder
            .ins()
            .brif(signature_matches, invoke, &[], slow, &[]);

        builder.switch_to_block(invoke);
        let scalar = match direct.call.result() {
            FeedbackRepresentation::Int32 | FeedbackRepresentation::Bool => types::I32,
            FeedbackRepresentation::Float64 => types::F64,
            FeedbackRepresentation::HeapRef => {
                unreachable!("direct calls cannot return owned heap values")
            }
        };
        let mut signature = Signature::new(builder.func.signature.call_conv);
        signature.params.push(AbiParam::new(helpers.pointer_type));
        for representation in direct.call.arguments() {
            signature.params.push(AbiParam::new(match representation {
                FeedbackRepresentation::Int32 | FeedbackRepresentation::Bool => types::I32,
                FeedbackRepresentation::Float64 => types::F64,
                FeedbackRepresentation::HeapRef => helpers.pointer_type,
            }));
        }
        signature.returns.push(AbiParam::new(types::I32));
        let signature = builder.import_signature(signature);
        let target = builder
            .ins()
            .iconst(helpers.pointer_type, direct.entry as i64);
        let scalar_output = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            scalar.bytes(),
            0,
        ));
        let scalar_output = builder
            .ins()
            .stack_addr(helpers.pointer_type, scalar_output, 0);
        let mut params = Vec::with_capacity(argc + 1);
        params.push(scalar_output);
        for (index, representation) in direct.call.arguments().iter().enumerate() {
            let argument = use_pair(builder, helpers.stack[argv_index + index]);
            params.push(match representation {
                FeedbackRepresentation::Int32 | FeedbackRepresentation::Bool => {
                    builder.ins().ireduce(types::I32, argument.payload)
                }
                FeedbackRepresentation::Float64 => {
                    builder
                        .ins()
                        .bitcast(types::F64, MemFlags::new(), argument.payload)
                }
                FeedbackRepresentation::HeapRef => argument.payload,
            });
        }
        let call = emit_external_call(
            builder,
            signature,
            target,
            &params,
            helpers.pointer_type,
            None,
            None,
        );
        let status = builder.inst_results(call)[0];
        let success = builder.ins().icmp_imm(IntCC::Equal, status, 0);
        builder.ins().brif(success, direct_done, &[], slow, &[]);

        builder.switch_to_block(direct_done);
        let raw = builder
            .ins()
            .load(scalar, MemFlags::new(), scalar_output, 0);
        let result = match direct.call.result() {
            FeedbackRepresentation::Int32 => Pair {
                payload: builder.ins().sextend(types::I64, raw),
                tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
            },
            FeedbackRepresentation::Float64 => Pair {
                payload: builder.ins().bitcast(types::I64, MemFlags::new(), raw),
                tag: builder
                    .ins()
                    .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64)),
            },
            FeedbackRepresentation::Bool => Pair {
                payload: builder.ins().uextend(types::I64, raw),
                tag: builder
                    .ins()
                    .iconst(types::I64, i64::from(qjs::JS_TAG_BOOL)),
            },
            FeedbackRepresentation::HeapRef => {
                unreachable!("direct calls cannot return owned heap values")
            }
        };
        define_pair(builder, helpers.stack[output_index], result);
        let hit = builder.ins().iconst(types::I8, 1);
        builder.ins().jump(joined, &[hit]);

        builder.switch_to_block(slow);
        helpers.invoke(
            builder,
            if is_constructor {
                qjs::JSJitHelperId_JS_JIT_HELPER_CALL_CONSTRUCTOR
            } else {
                qjs::JSJitHelperId_JS_JIT_HELPER_CALL
            },
            call_state,
            call_live_depth,
            *depth,
            &[
                output,
                function,
                this_value,
                argv,
                u32::try_from(argc).map_err(|_| CompileFailure::ResourceLimit)?,
            ],
        )?;
        reload_pair(
            builder,
            helpers.stack[output_index],
            helpers.stack_base,
            output_index,
            helpers.layout,
        );
        clear_reloaded_output(builder, helpers, output_index)?;
        let miss = builder.ins().iconst(types::I8, 0);
        builder.ins().jump(joined, &[miss]);
        builder.switch_to_block(joined);
        direct_hit = Some(builder.block_params(joined)[0]);
    } else {
        let push_joined = match helpers.object_fast {
            Some(fast) if has_this && !is_constructor && argc == 1 => {
                Some(emit_array_push_fast_path(
                    builder,
                    helpers,
                    fast,
                    function_index,
                    this_index,
                    argv_index,
                    output_index,
                )?)
            }
            _ => None,
        };
        helpers.invoke(
            builder,
            if is_constructor {
                qjs::JSJitHelperId_JS_JIT_HELPER_CALL_CONSTRUCTOR
            } else {
                qjs::JSJitHelperId_JS_JIT_HELPER_CALL
            },
            call_state,
            call_live_depth,
            *depth,
            &[
                output,
                function,
                this_value,
                argv,
                u32::try_from(argc).map_err(|_| CompileFailure::ResourceLimit)?,
            ],
        )?;
        reload_pair(
            builder,
            helpers.stack[output_index],
            helpers.stack_base,
            output_index,
            helpers.layout,
        );
        clear_reloaded_output(builder, helpers, output_index)?;
        if let Some(joined) = push_joined {
            builder.ins().jump(joined, &[]);
            builder.seal_block(joined);
            builder.switch_to_block(joined);
        }
    }
    // CALL borrows every input. The bytecode stack effect is separate and is
    // implemented in QuickJS interpreter order, clearing primitive inputs
    // directly and using FREE for owners or stress-GC instrumentation.
    // Move the first input aside and install the result in its logical slot
    // before FREE can finalize or re-enter, leaving at most one scratch owner.
    let displaced_index = if has_this {
        output_index + 1
    } else {
        this_index
    };
    move_stack_pair(
        builder,
        helpers.stack,
        helpers.stack_base,
        base,
        displaced_index,
        helpers.layout,
    )?;
    move_stack_pair(
        builder,
        helpers.stack,
        helpers.stack_base,
        output_index,
        base,
        helpers.layout,
    )?;
    let displaced_free_state = next_helper_state(states)?;
    if let Some(hit) = direct_hit {
        let release_duplicate = builder.create_block();
        let free = builder.create_block();
        let continuation = builder.create_block();
        builder.ins().brif(hit, release_duplicate, &[], free, &[]);
        builder.switch_to_block(release_duplicate);
        // The direct guard proved this is the exact function object, and its
        // argument slot still owns the original reference. GetArgument added
        // precisely one duplicate, so dropping that duplicate can never run a
        // finalizer and is the exact non-finalizing JS_FreeValue fast path.
        let function = use_pair(builder, helpers.stack[displaced_index]);
        let ref_count = builder.ins().load(
            types::I32,
            MemFlags::trusted(),
            function.payload,
            crate::abi::REF_COUNT_OFFSET,
        );
        let ref_count = builder.ins().iadd_imm(ref_count, -1);
        builder.ins().store(
            MemFlags::trusted(),
            ref_count,
            function.payload,
            crate::abi::REF_COUNT_OFFSET,
        );
        clear_pair(
            builder,
            helpers.stack[displaced_index],
            helpers.stack_base,
            displaced_index,
            helpers.layout,
        )?;
        builder.ins().jump(continuation, &[]);
        builder.switch_to_block(free);
        lower_call_input_free(
            builder,
            helpers,
            displaced_free_state,
            *depth,
            displaced_index,
        )?;
        builder.ins().jump(continuation, &[]);
        builder.switch_to_block(continuation);
    } else {
        lower_call_input_free(
            builder,
            helpers,
            displaced_free_state,
            *depth,
            displaced_index,
        )?;
    }
    for index in (base + 1)..(base + pop) {
        let state = next_helper_state(states)?;
        if let Some(hit) = direct_hit {
            let representation = direct
                .and_then(|site| site.call.arguments().get(index - argv_index))
                .copied()
                .ok_or(CompileFailure::InvalidArtifact)?;
            if representation == crate::runtime::FeedbackRepresentation::HeapRef {
                // The linked leaf borrows HeapRef payloads. CALL still consumes
                // the stack's owning reference after a hit, exactly as the
                // generic helper path does.
                lower_call_input_free(builder, helpers, state, *depth, index)?;
                continue;
            }
            // A primitive direct argument can be cleared without a helper.
            let clear = builder.create_block();
            let free = builder.create_block();
            let continuation = builder.create_block();
            builder.ins().brif(hit, clear, &[], free, &[]);
            builder.switch_to_block(clear);
            clear_pair(
                builder,
                helpers.stack[index],
                helpers.stack_base,
                index,
                helpers.layout,
            )?;
            builder.ins().jump(continuation, &[]);
            builder.switch_to_block(free);
            lower_call_input_free(builder, helpers, state, *depth, index)?;
            builder.ins().jump(continuation, &[]);
            builder.switch_to_block(continuation);
        } else {
            lower_call_input_free(builder, helpers, state, *depth, index)?;
        }
    }
    for index in (base + 1)..=output_index {
        clear_pair(
            builder,
            helpers.stack[index],
            helpers.stack_base,
            index,
            helpers.layout,
        )?;
    }
    *depth = base + 1;
    helpers.set_depth(builder, *depth)
}

/// Release one consumed CALL input after the result has been installed. The
/// remaining operands stay rooted until their turn in interpreter order.
fn lower_call_input_free(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    state: FrameStateId,
    depth: usize,
    index: usize,
) -> Result<(), CompileFailure> {
    let clear = builder.create_block();
    let free = builder.create_block();
    let continuation = builder.create_block();
    let input = use_pair(builder, helpers.stack[index]);
    super::call_cleanup::emit_cleanup_dispatch(
        builder,
        helpers.frame,
        input.tag,
        helpers.layout.flags,
        clear,
        free,
    );
    builder.switch_to_block(clear);
    clear_pair(
        builder,
        helpers.stack[index],
        helpers.stack_base,
        index,
        helpers.layout,
    )?;
    builder.ins().jump(continuation, &[]);
    builder.switch_to_block(free);
    // Heap owners, and every operand in stress mode, reach this block. A
    // shared heap reference is released inline; the last one and stress mode
    // keep the exact FREE helper.
    let refcounted = super::refcount::emit_has_ref_count(builder, input.tag);
    let release = builder.create_block();
    let released = builder.create_block();
    let helper = builder.create_block();
    builder.ins().brif(refcounted, release, &[], helper, &[]);
    builder.switch_to_block(release);
    super::refcount::emit_release_refcounted(
        builder,
        helpers.frame,
        helpers.layout.flags,
        input.payload,
        released,
        helper,
    );
    builder.switch_to_block(released);
    // The slot may be a scratch owner that an earlier helper published; the
    // FREE helper would leave undefined there, and exits require it.
    let undefined = constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64));
    store_jsvalue_slot(
        builder,
        helpers.stack_base,
        index,
        undefined,
        helpers.layout,
    )?;
    builder.ins().jump(clear, &[]);
    builder.switch_to_block(helper);
    let slot = flat_stack_slot(helpers.ir, index)?;
    helpers.invoke(
        builder,
        qjs::JSJitHelperId_JS_JIT_HELPER_FREE,
        state,
        depth + 2,
        depth,
        &[slot],
    )?;
    reload_pair(
        builder,
        helpers.stack[index],
        helpers.stack_base,
        index,
        helpers.layout,
    );
    builder.ins().jump(continuation, &[]);
    builder.switch_to_block(continuation);
    Ok(())
}

fn load_jsvalue(
    builder: &mut FunctionBuilder<'_>,
    base: Value,
    index: usize,
    layout: FrameLayout,
) -> Pair {
    let offset = i32::try_from(index * 16).expect("verified frame index fits i32");
    let flags = MemFlags::new();
    Pair {
        payload: builder.ins().load(types::I64, flags, base, offset),
        tag: builder
            .ins()
            .load(types::I64, flags, base, offset + layout.value_tag),
    }
}

fn store_jsvalue(
    builder: &mut FunctionBuilder<'_>,
    base: Value,
    offset: i32,
    value: Pair,
    layout: FrameLayout,
) {
    let flags = MemFlags::new();
    builder.ins().store(flags, value.payload, base, offset);
    builder
        .ins()
        .store(flags, value.tag, base, offset + layout.value_tag);
}

fn store_jsvalue_slot(
    builder: &mut FunctionBuilder<'_>,
    base: Value,
    index: usize,
    value: Pair,
    layout: FrameLayout,
) -> Result<(), CompileFailure> {
    let offset = index
        .checked_mul(mem::size_of::<qjs::JSValue>())
        .and_then(|offset| i32::try_from(offset).ok())
        .ok_or(CompileFailure::ResourceLimit)?;
    store_jsvalue(builder, base, offset, value, layout);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn materialize_frame(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    arg_buf: Value,
    var_buf: Value,
    stack_base: Value,
    arguments: &[PairVars],
    locals: &[PairVars],
    stack: &[PairVars],
    live_depth: usize,
    visible_depth: usize,
    bytecode_pc: u32,
    pointer_type: cranelift_codegen::ir::Type,
    layout: FrameLayout,
) -> Result<(), CompileFailure> {
    if live_depth > visible_depth || visible_depth > stack.len() {
        return Err(CompileFailure::InvalidArtifact);
    }
    for (index, pair) in arguments.iter().copied().enumerate() {
        let value = use_pair(builder, pair);
        store_jsvalue_slot(builder, arg_buf, index, value, layout)?;
    }
    for (index, pair) in locals.iter().copied().enumerate() {
        let value = use_pair(builder, pair);
        store_jsvalue_slot(builder, var_buf, index, value, layout)?;
    }
    for (index, pair) in stack.iter().copied().take(live_depth).enumerate() {
        let value = use_pair(builder, pair);
        store_jsvalue_slot(builder, stack_base, index, value, layout)?;
    }
    let undefined = constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64));
    for (index, pair) in stack
        .iter()
        .copied()
        .enumerate()
        .take(visible_depth)
        .skip(live_depth)
    {
        define_pair(builder, pair, undefined);
        store_jsvalue_slot(builder, stack_base, index, undefined, layout)?;
    }
    let stack_bytes = visible_depth
        .checked_mul(mem::size_of::<qjs::JSValue>())
        .and_then(|bytes| i64::try_from(bytes).ok())
        .ok_or(CompileFailure::ResourceLimit)?;
    let stack_top = builder.ins().iadd_imm(stack_base, stack_bytes);
    let flags = MemFlags::new();
    builder
        .ins()
        .store(flags, stack_top, frame, layout.stack_top);
    let bytecode = builder
        .ins()
        .load(pointer_type, flags, frame, layout.bytecode_start);
    let pc = builder.ins().iadd_imm(bytecode, i64::from(bytecode_pc));
    builder.ins().store(flags, pc, frame, layout.pc);
    Ok(())
}

fn set_visible_stack_depth(
    _builder: &mut FunctionBuilder<'_>,
    _frame: Value,
    _stack_base: Value,
    depth: usize,
    _layout: FrameLayout,
) -> Result<(), CompileFailure> {
    // Native SSA variables are authoritative between safepoints. Helpers and
    // polls call `materialize_frame`, while DONE/EXCEPTION use the forced
    // variant below; eagerly publishing every bytecode stack effect here
    // would put interpreter-frame stores back into the hot loop.
    depth
        .checked_mul(mem::size_of::<qjs::JSValue>())
        .and_then(|bytes| i64::try_from(bytes).ok())
        .ok_or(CompileFailure::ResourceLimit)?;
    Ok(())
}

fn force_visible_stack_depth(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    stack_base: Value,
    depth: usize,
    layout: FrameLayout,
) -> Result<(), CompileFailure> {
    let bytes = depth
        .checked_mul(mem::size_of::<qjs::JSValue>())
        .and_then(|bytes| i64::try_from(bytes).ok())
        .ok_or(CompileFailure::ResourceLimit)?;
    let top = builder.ins().iadd_imm(stack_base, bytes);
    builder
        .ins()
        .store(MemFlags::new(), top, frame, layout.stack_top);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn emit_helper_call(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    runtime_api: Value,
    sret: Value,
    stack_base: Value,
    exception_depth: usize,
    signatures: &[cranelift_codegen::ir::SigRef],
    helper_id: usize,
    state: FrameStateId,
    arguments: &[Value],
    pointer_type: cranelift_codegen::ir::Type,
    layout: FrameLayout,
    exception_target: Option<Block>,
) -> Result<(), CompileFailure> {
    let signature = *signatures
        .get(helper_id)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let offset = *layout
        .helper_offsets
        .get(helper_id)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let flags = MemFlags::new();
    let helper = builder.ins().load(pointer_type, flags, runtime_api, offset);
    let mut params = Vec::with_capacity(arguments.len() + 1);
    params.push(frame);
    params.extend_from_slice(arguments);
    builder.set_srcloc(frame_state_source_loc(state)?);
    let source_location = frame_state_source_loc(state)?;
    let call = emit_external_call(
        builder,
        signature,
        helper,
        &params,
        pointer_type,
        Some(frame),
        Some(source_location),
    );
    builder.set_srcloc(SourceLoc::default());
    let status = builder.inst_results(call)[0];
    let succeeded = builder.ins().icmp_imm(IntCC::Equal, status, 0);
    let continuation = builder.create_block();
    let exception = builder.create_block();
    builder
        .ins()
        .brif(succeeded, continuation, &[], exception, &[]);
    builder.seal_block(exception);
    builder.seal_block(continuation);
    builder.switch_to_block(exception);
    emit_exception_edge(
        builder,
        frame,
        sret,
        stack_base,
        exception_depth,
        exception_target,
        pointer_type,
        layout,
    )?;
    builder.switch_to_block(continuation);
    Ok(())
}

/// Publishes the exact operand stack owned at a pending exception, then
/// either enters the innermost try region's landing pad or exits so the
/// interpreter unwinds the frame.
#[allow(clippy::too_many_arguments)]
fn emit_exception_edge(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    sret: Value,
    stack_base: Value,
    exception_depth: usize,
    exception_target: Option<Block>,
    pointer_type: cranelift_codegen::ir::Type,
    layout: FrameLayout,
) -> Result<(), CompileFailure> {
    force_visible_stack_depth(builder, frame, stack_base, exception_depth, layout)?;
    if let Some(landing) = exception_target {
        builder.ins().jump(landing, &[]);
    } else {
        emit_exit(
            builder,
            sret,
            qjs::JSJitExitKind_JS_JIT_EXIT_EXCEPTION,
            None,
            pointer_type,
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn invoke_frame_helper(
    builder: &mut FunctionBuilder<'_>,
    ir: &BaselineIr,
    frame: Value,
    runtime_api: Value,
    sret: Value,
    arg_buf: Value,
    var_buf: Value,
    stack_base: Value,
    arguments: &[PairVars],
    locals: &[PairVars],
    stack: &[PairVars],
    live_depth: usize,
    exception_depth: usize,
    helper_signatures: &[cranelift_codegen::ir::SigRef],
    helper_id: usize,
    state: FrameStateId,
    helper_arguments: &[u32],
    pointer_type: cranelift_codegen::ir::Type,
    layout: FrameLayout,
    exception_target: Option<Block>,
) -> Result<(), CompileFailure> {
    let frame_state = ir.frame_states.get(state);
    let fixed_slots = usize::from(ir.argument_count) + usize::from(ir.local_count);
    let visible_depth = frame_state
        .slots
        .len()
        .checked_sub(fixed_slots)
        .ok_or(CompileFailure::InvalidArtifact)?;
    materialize_frame(
        builder,
        frame,
        arg_buf,
        var_buf,
        stack_base,
        arguments,
        locals,
        stack,
        live_depth,
        visible_depth,
        frame_state.pc,
        pointer_type,
        layout,
    )?;
    let mut values = Vec::with_capacity(helper_arguments.len() + 1);
    values.push(helper_u32(
        builder,
        u32::try_from(state.index()).map_err(|_| CompileFailure::ResourceLimit)?,
    ));
    values.extend(
        helper_arguments
            .iter()
            .copied()
            .map(|argument| helper_u32(builder, argument)),
    );
    emit_helper_call(
        builder,
        frame,
        runtime_api,
        sret,
        stack_base,
        exception_depth,
        helper_signatures,
        helper_id,
        state,
        &values,
        pointer_type,
        layout,
        exception_target,
    )
}

/// Calls one of the ABI 1.25 exception entry points (`JS_JitThrowValue`,
/// `JS_JitThrowError`, `JS_JitCatchException`). They share the helper
/// convention: frame, stack-map id and u32 operands in, status out. The call
/// carries its frame state's source location so the state maps to exactly
/// this call's return address.
fn emit_exception_api_call(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    address: usize,
    state: FrameStateId,
    operands: &[u32],
    pointer_type: cranelift_codegen::ir::Type,
) -> Result<Value, CompileFailure> {
    let mut signature = Signature::new(builder.func.signature.call_conv);
    signature.params.push(AbiParam::new(pointer_type));
    signature.params.push(AbiParam::new(types::I32));
    for _ in operands {
        signature.params.push(AbiParam::new(types::I32));
    }
    signature.returns.push(AbiParam::new(types::I32));
    let signature = builder.import_signature(signature);
    let target = builder.ins().iconst(
        pointer_type,
        i64::try_from(address).map_err(|_| CompileFailure::InvalidArtifact)?,
    );
    let mut params = Vec::with_capacity(operands.len() + 2);
    params.push(frame);
    params.push(helper_u32(
        builder,
        u32::try_from(state.index()).map_err(|_| CompileFailure::ResourceLimit)?,
    ));
    params.extend(
        operands
            .iter()
            .copied()
            .map(|operand| helper_u32(builder, operand)),
    );
    let source_location = frame_state_source_loc(state)?;
    builder.set_srcloc(source_location);
    let call = emit_external_call(
        builder,
        signature,
        target,
        &params,
        pointer_type,
        Some(frame),
        Some(source_location),
    );
    builder.set_srcloc(SourceLoc::default());
    Ok(builder.inst_results(call)[0])
}

fn helper_u32(builder: &mut FunctionBuilder<'_>, value: u32) -> Value {
    builder.ins().iconst(types::I32, i64::from(value))
}

fn flat_stack_slot(ir: &BaselineIr, index: usize) -> Result<u32, CompileFailure> {
    usize::from(ir.argument_count)
        .checked_add(usize::from(ir.local_count))
        .and_then(|base| base.checked_add(index))
        .and_then(|slot| u32::try_from(slot).ok())
        .ok_or(CompileFailure::ResourceLimit)
}

fn flat_argument_slot(index: u16) -> u32 {
    u32::from(index)
}

fn flat_local_slot(ir: &BaselineIr, index: u16) -> u32 {
    u32::from(ir.argument_count) + u32::from(index)
}

fn reload_pair(
    builder: &mut FunctionBuilder<'_>,
    variables: PairVars,
    base: Value,
    index: usize,
    layout: FrameLayout,
) {
    let value = load_jsvalue(builder, base, index, layout);
    define_pair(builder, variables, value);
}

fn move_stack_pair(
    builder: &mut FunctionBuilder<'_>,
    stack: &[PairVars],
    base: Value,
    source: usize,
    destination: usize,
    layout: FrameLayout,
) -> Result<(), CompileFailure> {
    if source == destination {
        return Ok(());
    }
    let value = use_pair(builder, stack[source]);
    define_pair(builder, stack[destination], value);
    let _ = (base, layout);
    clear_pair(builder, stack[source], base, source, layout)
}

/// After a helper publishes an owned output in frame memory and the value is
/// reloaded into SSA, the SSA variable is the only owner the lowering tracks.
/// A later inline FREE no longer rematerializes the frame, so clear the
/// published copy: CALL/GET_PROPERTY outputs live in scratch slots, which must
/// hold undefined at every exit.
fn clear_reloaded_output(
    builder: &mut FunctionBuilder<'_>,
    helpers: &HelperLowering<'_>,
    index: usize,
) -> Result<(), CompileFailure> {
    let undefined = constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64));
    store_jsvalue_slot(
        builder,
        helpers.stack_base,
        index,
        undefined,
        helpers.layout,
    )
}

fn clear_pair(
    builder: &mut FunctionBuilder<'_>,
    variables: PairVars,
    base: Value,
    index: usize,
    layout: FrameLayout,
) -> Result<(), CompileFailure> {
    let undefined = constant_pair(builder, TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64));
    define_pair(builder, variables, undefined);
    let _ = (base, index, layout);
    Ok(())
}

fn constant_pair(builder: &mut FunctionBuilder<'_>, value: TaggedValue) -> Pair {
    Pair {
        payload: builder.ins().iconst(types::I64, value.payload as i64),
        tag: builder.ins().iconst(types::I64, value.tag),
    }
}

fn define_pair(builder: &mut FunctionBuilder<'_>, variables: PairVars, value: Pair) {
    builder.def_var(variables.payload, value.payload);
    builder.def_var(variables.tag, value.tag);
}

fn use_pair(builder: &mut FunctionBuilder<'_>, variables: PairVars) -> Pair {
    Pair {
        payload: builder.use_var(variables.payload),
        tag: builder.use_var(variables.tag),
    }
}

fn emit_entry_domain_guards(
    builder: &mut FunctionBuilder<'_>,
    arguments: &[PairVars],
    locals: &[PairVars],
    analysis: &EntryAnalysis,
    retry: Block,
) {
    for (root, requirements) in &analysis.requirements {
        debug_assert!(
            !requirements.uninitialized || !(requirements.numeric || requirements.initialized),
            "entry-domain analysis rejected conflicting requirements"
        );
        let variables = match *root {
            EntryRoot::Argument(index) => arguments[usize::from(index)],
            EntryRoot::Local(index) => locals[usize::from(index)],
        };
        let value = use_pair(builder, variables);
        let condition = if requirements.uninitialized {
            tag_is(builder, value.tag, qjs::JS_TAG_UNINITIALIZED)
        } else if requirements.numeric {
            emit_numeric_tag(builder, value.tag)
        } else {
            debug_assert!(requirements.initialized);
            builder.ins().icmp_imm(
                IntCC::NotEqual,
                value.tag,
                i64::from(qjs::JS_TAG_UNINITIALIZED),
            )
        };
        guard(builder, condition, retry);
    }
}

fn validate_live_stack_bounds(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    retry: Block,
    pointer_type: cranelift_codegen::ir::Type,
    layout: FrameLayout,
    max_stack_slots: u16,
) {
    let flags = MemFlags::new();
    let stack_base = builder
        .ins()
        .load(pointer_type, flags, frame, layout.stack_base);
    let stack_top = builder
        .ins()
        .load(pointer_type, flags, frame, layout.stack_top);

    let base_non_null = builder.ins().icmp_imm(IntCC::NotEqual, stack_base, 0);
    let top_non_null = builder.ins().icmp_imm(IntCC::NotEqual, stack_top, 0);
    let non_null = builder.ins().band(base_non_null, top_non_null);
    guard(builder, non_null, retry);

    let combined = builder.ins().bor(stack_base, stack_top);
    let low_bits = builder.ins().band_imm(combined, 15);
    let aligned = builder.ins().icmp_imm(IntCC::Equal, low_bits, 0);
    guard(builder, aligned, retry);

    let ordered = builder
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, stack_top, stack_base);
    guard(builder, ordered, retry);

    let byte_len = builder.ins().isub(stack_top, stack_base);
    let trailing = builder.ins().band_imm(byte_len, 15);
    let whole_values = builder.ins().icmp_imm(IntCC::Equal, trailing, 0);
    guard(builder, whole_values, retry);

    let slot_count = builder.ins().ushr_imm(byte_len, 4);
    let slot_limit = builder
        .ins()
        .iconst(pointer_type, i64::from(max_stack_slots));
    let bounded = builder
        .ins()
        .icmp(IntCC::UnsignedLessThanOrEqual, slot_count, slot_limit);
    guard(builder, bounded, retry);
}

fn emit_exit(
    builder: &mut FunctionBuilder<'_>,
    sret: Value,
    kind: qjs::JSJitExitKind,
    resume_pc: Option<Value>,
    pointer_type: cranelift_codegen::ir::Type,
) {
    let flags = MemFlags::new();
    let kind = builder.ins().iconst(types::I32, i64::from(kind));
    let zero32 = builder.ins().iconst(types::I32, 0);
    let zero_pointer = builder.ins().iconst(pointer_type, 0);
    builder.ins().store(flags, kind, sret, 0);
    builder.ins().store(flags, zero32, sret, 4);
    builder
        .ins()
        .store(flags, resume_pc.unwrap_or(zero_pointer), sret, 8);
    builder.ins().store(flags, zero_pointer, sret, 16);
    builder.ins().return_(&[]);
}

fn emit_frame_state_marker(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    invariant_trap: Block,
    state: FrameStateId,
) -> Result<(), CompileFailure> {
    let frame_is_non_null = builder.ins().icmp_imm(IntCC::NotEqual, frame, 0);
    let continuation = builder.create_block();
    builder.set_srcloc(frame_state_source_loc(state)?);
    // The entry ABI requires a non-null frame. Reasserting that invariant as
    // a control-flow edge is semantically inert for valid entries, cannot be
    // removed as a dead value, and gives each non-call state one exact machine
    // range without mutating the execution frame.
    builder
        .ins()
        .brif(frame_is_non_null, continuation, &[], invariant_trap, &[]);
    builder.set_srcloc(SourceLoc::default());
    builder.seal_block(continuation);
    builder.switch_to_block(continuation);
    Ok(())
}

#[allow(clippy::too_many_arguments)] // Poll ABI parameters mirror the generated helper signature.
fn emit_poll(
    builder: &mut FunctionBuilder<'_>,
    frame: Value,
    sret: Value,
    poll: Value,
    signature: cranelift_codegen::ir::SigRef,
    location: PollLocation,
    pointer_type: cranelift_codegen::ir::Type,
    layout: FrameLayout,
) {
    let flags = MemFlags::new();
    builder.set_srcloc(location.source_location);
    let call = emit_external_call(
        builder,
        signature,
        poll,
        &[frame],
        pointer_type,
        Some(frame),
        Some(location.source_location),
    );
    builder.set_srcloc(SourceLoc::default());
    let interrupted = builder.inst_results(call)[0];
    let interrupted = builder.ins().icmp_imm(IntCC::NotEqual, interrupted, 0);
    let interrupt = builder.create_block();
    let continuation = builder.create_block();
    builder
        .ins()
        .brif(interrupted, interrupt, &[], continuation, &[]);
    builder.seal_block(interrupt);
    builder.seal_block(continuation);
    builder.switch_to_block(interrupt);
    let bytecode = builder
        .ins()
        .load(pointer_type, flags, frame, layout.bytecode_start);
    let resume = builder
        .ins()
        .iadd_imm(bytecode, i64::from(location.bytecode_pc));
    emit_exit(
        builder,
        sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_INTERRUPT,
        Some(resume),
        pointer_type,
    );
    builder.switch_to_block(continuation);
}

fn guard(builder: &mut FunctionBuilder<'_>, condition: Value, retry: Block) {
    let continuation = builder.create_block();
    builder.ins().brif(condition, continuation, &[], retry, &[]);
    builder.seal_block(continuation);
    builder.switch_to_block(continuation);
}

fn tag_is(builder: &mut FunctionBuilder<'_>, tag: Value, expected: i32) -> Value {
    builder
        .ins()
        .icmp_imm(IntCC::Equal, tag, i64::from(expected))
}

fn emit_numeric_tag(builder: &mut FunctionBuilder<'_>, tag: Value) -> Value {
    let is_int = tag_is(builder, tag, qjs::JS_TAG_INT);
    let is_float = tag_is(builder, tag, qjs::JS_TAG_FLOAT64);
    builder.ins().bor(is_int, is_float)
}

fn emit_numeric(builder: &mut FunctionBuilder<'_>, value: Pair) -> (Value, Value) {
    let is_int = tag_is(builder, value.tag, qjs::JS_TAG_INT);
    let int = builder.ins().ireduce(types::I32, value.payload);
    let int_float = builder.ins().fcvt_from_sint(types::F64, int);
    let raw_float = builder
        .ins()
        .bitcast(types::F64, MemFlags::new(), value.payload);
    let number = builder.ins().select(is_int, int_float, raw_float);
    (is_int, number)
}

fn pair_from_bool(builder: &mut FunctionBuilder<'_>, value: Value) -> Pair {
    Pair {
        payload: builder.ins().uextend(types::I64, value),
        tag: builder
            .ins()
            .iconst(types::I64, i64::from(qjs::JS_TAG_BOOL)),
    }
}

fn emit_truthy(builder: &mut FunctionBuilder<'_>, value: Pair) -> Value {
    let is_null = tag_is(builder, value.tag, qjs::JS_TAG_NULL);
    let is_undefined = tag_is(builder, value.tag, qjs::JS_TAG_UNDEFINED);
    let is_float = tag_is(builder, value.tag, qjs::JS_TAG_FLOAT64);
    let empty = builder.ins().bor(is_null, is_undefined);
    let integer = builder.ins().ireduce(types::I32, value.payload);
    let integer_truthy = builder.ins().icmp_imm(IntCC::NotEqual, integer, 0);
    let float_truthy = super::helpers::emit_f64_bits_truthy(builder, value.payload);
    let scalar_truthy = builder.ins().select(is_float, float_truthy, integer_truthy);
    let false_value = builder.ins().iconst(types::I8, 0);
    builder.ins().select(empty, false_value, scalar_truthy)
}

fn emit_unary(builder: &mut FunctionBuilder<'_>, value: Pair, operation: UnaryOp) -> Pair {
    match operation {
        UnaryOp::IsUndefinedOrNull | UnaryOp::IsUndefined | UnaryOp::IsNull => {
            unreachable!("lowered with exact ownership handling")
        }
        UnaryOp::LogicalNot => {
            let truthy = emit_truthy(builder, value);
            let inverse = builder.ins().bxor_imm(truthy, 1);
            pair_from_bool(builder, inverse)
        }
        UnaryOp::BitNot => {
            let int = emit_to_i32(builder, value);
            let int = builder.ins().bnot(int);
            Pair {
                payload: builder.ins().sextend(types::I64, int),
                tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
            }
        }
        UnaryOp::Plus => {
            let _ = emit_numeric(builder, value);
            value
        }
        UnaryOp::Neg | UnaryOp::Increment | UnaryOp::Decrement => {
            let (is_int, number) = emit_numeric(builder, value);
            let int = builder.ins().ireduce(types::I32, value.payload);
            let (int_result, overflow) = match operation {
                UnaryOp::Neg => {
                    let zero = builder.ins().iconst(types::I32, 0);
                    builder.ins().ssub_overflow(zero, int)
                }
                UnaryOp::Increment => {
                    let one = builder.ins().iconst(types::I32, 1);
                    builder.ins().sadd_overflow(int, one)
                }
                UnaryOp::Decrement => {
                    let one = builder.ins().iconst(types::I32, 1);
                    builder.ins().ssub_overflow(int, one)
                }
                _ => unreachable!(),
            };
            let no_overflow = builder.ins().bxor_imm(overflow, 1);
            let mut int_fast = builder.ins().band(is_int, no_overflow);
            if operation == UnaryOp::Neg {
                let nonzero = builder.ins().icmp_imm(IntCC::NotEqual, int, 0);
                int_fast = builder.ins().band(int_fast, nonzero);
            }
            let float_result = match operation {
                UnaryOp::Neg => builder.ins().fneg(number),
                UnaryOp::Increment => {
                    let one = builder.ins().f64const(1.0);
                    builder.ins().fadd(number, one)
                }
                UnaryOp::Decrement => {
                    let one = builder.ins().f64const(1.0);
                    builder.ins().fsub(number, one)
                }
                _ => unreachable!(),
            };
            select_int_or_float(builder, int_fast, int_result, float_result)
        }
    }
}

fn emit_binary(
    builder: &mut FunctionBuilder<'_>,
    left: Pair,
    right: Pair,
    operation: BinaryOp,
) -> Pair {
    match operation {
        BinaryOp::BitAnd
        | BinaryOp::BitOr
        | BinaryOp::BitXor
        | BinaryOp::ShiftLeft
        | BinaryOp::ShiftRight
        | BinaryOp::ShiftRightUnsigned => emit_integer_binary(builder, left, right, operation),
        BinaryOp::LessThan
        | BinaryOp::LessThanOrEqual
        | BinaryOp::GreaterThan
        | BinaryOp::GreaterThanOrEqual
        | BinaryOp::Equal
        | BinaryOp::NotEqual
        | BinaryOp::StrictEqual
        | BinaryOp::StrictNotEqual => emit_comparison(builder, left, right, operation),
        _ => emit_arithmetic(builder, left, right, operation),
    }
}

fn emit_integer_binary(
    builder: &mut FunctionBuilder<'_>,
    left: Pair,
    right: Pair,
    operation: BinaryOp,
) -> Pair {
    let left = emit_to_i32(builder, left);
    let right = emit_to_i32(builder, right);
    let shift = builder.ins().band_imm(right, 31);
    let result = match operation {
        BinaryOp::BitAnd => builder.ins().band(left, right),
        BinaryOp::BitOr => builder.ins().bor(left, right),
        BinaryOp::BitXor => builder.ins().bxor(left, right),
        BinaryOp::ShiftLeft => builder.ins().ishl(left, shift),
        BinaryOp::ShiftRight => builder.ins().sshr(left, shift),
        BinaryOp::ShiftRightUnsigned => builder.ins().ushr(left, shift),
        _ => unreachable!(),
    };
    if operation == BinaryOp::ShiftRightUnsigned {
        let fits_i32 = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, result, 0);
        let int_payload = builder.ins().sextend(types::I64, result);
        let uint_float = builder.ins().fcvt_from_uint(types::F64, result);
        let float_payload = builder
            .ins()
            .bitcast(types::I64, MemFlags::new(), uint_float);
        let int_tag = builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT));
        let float_tag = builder
            .ins()
            .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64));
        Pair {
            payload: builder.ins().select(fits_i32, int_payload, float_payload),
            tag: builder.ins().select(fits_i32, int_tag, float_tag),
        }
    } else {
        Pair {
            payload: builder.ins().sextend(types::I64, result),
            tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
        }
    }
}

/// QuickJS's own interpreter fast path for `%`: both operands Int32, a
/// non-negative dividend, and a positive divisor. That excludes division by
/// zero, `INT32_MIN % -1`, and every result that must be `-0`, so `srem` is
/// exact and cannot trap. Everything else takes the runtime slow path.
fn emit_int_mod_fast_condition(
    builder: &mut FunctionBuilder<'_>,
    left: Pair,
    right: Pair,
) -> Value {
    let left_int = tag_is(builder, left.tag, qjs::JS_TAG_INT);
    let right_int = tag_is(builder, right.tag, qjs::JS_TAG_INT);
    let both_int = builder.ins().band(left_int, right_int);
    let left_i32 = builder.ins().ireduce(types::I32, left.payload);
    let right_i32 = builder.ins().ireduce(types::I32, right.payload);
    let left_non_negative = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, left_i32, 0);
    let right_positive = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThan, right_i32, 0);
    let operands_ok = builder.ins().band(left_non_negative, right_positive);
    builder.ins().band(both_int, operands_ok)
}

fn emit_int_mod(builder: &mut FunctionBuilder<'_>, left: Pair, right: Pair) -> Pair {
    let left_i32 = builder.ins().ireduce(types::I32, left.payload);
    let right_i32 = builder.ins().ireduce(types::I32, right.payload);
    let remainder = builder.ins().srem(left_i32, right_i32);
    Pair {
        payload: builder.ins().sextend(types::I64, remainder),
        tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
    }
}

fn emit_arithmetic(
    builder: &mut FunctionBuilder<'_>,
    left: Pair,
    right: Pair,
    operation: BinaryOp,
) -> Pair {
    debug_assert_ne!(operation, BinaryOp::Mod);
    let (left_int, left_float) = emit_numeric(builder, left);
    let (right_int, right_float) = emit_numeric(builder, right);
    let left_i32 = builder.ins().ireduce(types::I32, left.payload);
    let right_i32 = builder.ins().ireduce(types::I32, right.payload);
    if operation == BinaryOp::Div {
        let result = builder.ins().fdiv(left_float, right_float);
        return pair_from_number(builder, result);
    }
    let (int_result, overflow) = match operation {
        BinaryOp::Add => builder.ins().sadd_overflow(left_i32, right_i32),
        BinaryOp::Sub => builder.ins().ssub_overflow(left_i32, right_i32),
        BinaryOp::Mul => builder.ins().smul_overflow(left_i32, right_i32),
        _ => unreachable!(),
    };
    let both_int = builder.ins().band(left_int, right_int);
    let no_overflow = builder.ins().bxor_imm(overflow, 1);
    let mut int_fast = builder.ins().band(both_int, no_overflow);
    if operation == BinaryOp::Mul {
        let result_zero = builder.ins().icmp_imm(IntCC::Equal, int_result, 0);
        let left_negative = builder.ins().icmp_imm(IntCC::SignedLessThan, left_i32, 0);
        let right_negative = builder.ins().icmp_imm(IntCC::SignedLessThan, right_i32, 0);
        let negative = builder.ins().bxor(left_negative, right_negative);
        let negative_zero = builder.ins().band(result_zero, negative);
        let safe_zero = builder.ins().bxor_imm(negative_zero, 1);
        int_fast = builder.ins().band(int_fast, safe_zero);
    }
    let float_result = match operation {
        BinaryOp::Add => builder.ins().fadd(left_float, right_float),
        BinaryOp::Sub => builder.ins().fsub(left_float, right_float),
        BinaryOp::Mul => builder.ins().fmul(left_float, right_float),
        _ => unreachable!(),
    };
    select_int_or_float(builder, int_fast, int_result, float_result)
}

fn select_int_or_float(
    builder: &mut FunctionBuilder<'_>,
    use_int: Value,
    integer: Value,
    float: Value,
) -> Pair {
    let int_payload = builder.ins().sextend(types::I64, integer);
    let number = pair_from_number(builder, float);
    let int_tag = builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT));
    Pair {
        payload: builder.ins().select(use_int, int_payload, number.payload),
        tag: builder.ins().select(use_int, int_tag, number.tag),
    }
}

fn pair_from_number(builder: &mut FunctionBuilder<'_>, value: Value) -> Pair {
    let integer = builder.ins().fcvt_to_sint_sat(types::I32, value);
    let roundtrip = builder.ins().fcvt_from_sint(types::F64, integer);
    let exact = builder.ins().fcmp(FloatCC::Equal, value, roundtrip);
    let bits = builder.ins().bitcast(types::I64, MemFlags::new(), value);
    let negative_zero = builder.ins().icmp_imm(IntCC::Equal, bits, i64::MIN);
    let not_negative_zero = builder.ins().bxor_imm(negative_zero, 1);
    let use_int = builder.ins().band(exact, not_negative_zero);
    let int_payload = builder.ins().sextend(types::I64, integer);
    let int_tag = builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT));
    let float_tag = builder
        .ins()
        .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64));
    Pair {
        payload: builder.ins().select(use_int, int_payload, bits),
        tag: builder.ins().select(use_int, int_tag, float_tag),
    }
}

fn emit_to_i32(builder: &mut FunctionBuilder<'_>, value: Pair) -> Value {
    let (is_int, number) = emit_numeric(builder, value);
    let direct = builder.ins().ireduce(types::I32, value.payload);
    let modulus = builder.ins().f64const(4_294_967_296.0);
    let quotient = builder.ins().fdiv(number, modulus);
    let quotient = builder.ins().trunc(quotient);
    let multiple = builder.ins().fmul(quotient, modulus);
    let remainder = builder.ins().fsub(number, multiple);
    let converted = builder.ins().fcvt_to_sint_sat(types::I64, remainder);
    let converted = builder.ins().ireduce(types::I32, converted);
    builder.ins().select(is_int, direct, converted)
}

fn emit_comparison(
    builder: &mut FunctionBuilder<'_>,
    left: Pair,
    right: Pair,
    operation: BinaryOp,
) -> Pair {
    let (_, left) = emit_numeric(builder, left);
    let (_, right) = emit_numeric(builder, right);
    let condition = match operation {
        BinaryOp::LessThan => builder.ins().fcmp(FloatCC::LessThan, left, right),
        BinaryOp::LessThanOrEqual => builder.ins().fcmp(FloatCC::LessThanOrEqual, left, right),
        BinaryOp::GreaterThan => builder.ins().fcmp(FloatCC::GreaterThan, left, right),
        BinaryOp::GreaterThanOrEqual => {
            builder.ins().fcmp(FloatCC::GreaterThanOrEqual, left, right)
        }
        BinaryOp::Equal | BinaryOp::StrictEqual => builder.ins().fcmp(FloatCC::Equal, left, right),
        BinaryOp::NotEqual | BinaryOp::StrictNotEqual => {
            builder.ins().fcmp(FloatCC::NotEqual, left, right)
        }
        _ => unreachable!(),
    };
    pair_from_bool(builder, condition)
}

fn apply_stack_operation(
    builder: &mut FunctionBuilder<'_>,
    stack: &[PairVars],
    depth: &mut usize,
    operation: StackOp,
) {
    let (take, order) = stack_operation_order(operation);
    let start = *depth - take;
    let values: Vec<Pair> = (0..take)
        .map(|index| use_pair(builder, stack[start + index]))
        .collect();
    for (destination, &source) in order.iter().enumerate() {
        define_pair(builder, stack[start + destination], values[source]);
    }
    *depth = start + order.len();
}

fn stack_operation_order(operation: StackOp) -> (usize, &'static [usize]) {
    match operation {
        StackOp::Nip => (2, &[1]),
        StackOp::Nip1 => (3, &[1, 2]),
        StackOp::Dup => (1, &[0, 0]),
        StackOp::Dup1 => (2, &[0, 0, 1]),
        StackOp::Dup2 => (2, &[0, 1, 0, 1]),
        StackOp::Dup3 => (3, &[0, 1, 2, 0, 1, 2]),
        StackOp::Insert2 => (2, &[1, 0, 1]),
        StackOp::Insert3 => (3, &[2, 0, 1, 2]),
        StackOp::Insert4 => (4, &[3, 0, 1, 2, 3]),
        StackOp::Perm3 => (3, &[1, 0, 2]),
        StackOp::Perm4 => (4, &[2, 0, 1, 3]),
        StackOp::Perm5 => (5, &[3, 0, 1, 2, 4]),
        StackOp::Swap => (2, &[1, 0]),
        StackOp::Swap2 => (4, &[2, 3, 0, 1]),
        StackOp::Rot3Left => (3, &[1, 2, 0]),
        StackOp::Rot3Right => (3, &[2, 0, 1]),
        StackOp::Rot4Left => (4, &[1, 2, 3, 0]),
        StackOp::Rot5Left => (5, &[1, 2, 3, 4, 0]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{FrameStateTable, IrBlock, IrInstruction};

    fn instruction(pc: u32, op: IrOp) -> IrInstruction {
        IrInstruction {
            pc,
            frame_state: None,
            helper_states: Box::new([]),
            op,
        }
    }

    fn numeric_push() -> IrInstruction {
        instruction(0, IrOp::Push(TaggedValue::new(1, qjs::JS_TAG_INT as i64)))
    }

    fn linear_ir(mut prefix: Vec<IrInstruction>, operation: IrOp) -> BaselineIr {
        prefix.push(instruction(0, operation));
        if !matches!(prefix.last().map(|item| &item.op), Some(IrOp::Return)) {
            prefix.push(instruction(1, IrOp::ReturnUndefined));
        }
        BaselineIr {
            blocks: vec![IrBlock {
                start_pc: 0,
                stack_depth: 0,
                instructions: prefix,
            }],
            frame_states: FrameStateTable::default(),
            max_stack_depth: 16,
            argument_count: 4,
            local_count: 4,
            exception_handlers: BTreeMap::new(),
            catch_sites: BTreeMap::new(),
        }
    }

    fn ir_op_variant(operation: &IrOp) -> &'static str {
        match operation {
            IrOp::Poll { .. } => "poll",
            IrOp::OsrLabel { .. } => "osr_label",
            IrOp::Nop => "nop",
            IrOp::Push(_) => "push",
            IrOp::ResolveConstant(_) => "resolve_constant",
            IrOp::ResolveAtom(_) => "resolve_atom",
            IrOp::GetGlobal(_) => "get_global",
            IrOp::NewObject => "new_object",
            IrOp::NewArrayFrom(_) => "new_array_from",
            IrOp::GetProperty(_) => "get_property",
            IrOp::GetPropertyKeep(_) => "get_property_keep",
            IrOp::SetProperty(_) => "set_property",
            IrOp::DefineProperty(_) => "define_property",
            IrOp::GetElement => "get_element",
            IrOp::SetElement => "set_element",
            IrOp::DefineElement => "define_element",
            IrOp::ToPropertyKey => "to_property_key",
            IrOp::Call { .. } => "call",
            IrOp::CallConstructor(_) => "call_constructor",
            IrOp::Regexp => "regexp",
            IrOp::FClosure(_) => "fclosure",
            IrOp::GetVarRef { .. } => "get_var_ref",
            IrOp::PutVarRef { .. } => "put_var_ref",
            IrOp::CloseLocal(_) => "close_local",
            IrOp::SetName(_) => "set_name",
            IrOp::Generic(_) => "generic",
            IrOp::Iterator(_) => "iterator",
            IrOp::GetArgument(_) => "get_argument",
            IrOp::GetLocal(_) => "get_local",
            IrOp::GetLocalChecked(_) => "get_local_checked",
            IrOp::GetLocalPair => "get_local_pair",
            IrOp::PutArgument { .. } => "put_argument",
            IrOp::PutLocal { .. } => "put_local",
            IrOp::PutLocalChecked { .. } => "put_local_checked",
            IrOp::SetLocalUninitialized(_) => "set_local_uninitialized",
            IrOp::Drop => "drop",
            IrOp::Stack(_) => "stack",
            IrOp::Unary(_) => "unary",
            IrOp::PostUnary(_) => "post_unary",
            IrOp::LocalUnary { .. } => "local_unary",
            IrOp::AddLocal(_) => "add_local",
            IrOp::Binary(_) => "binary",
            IrOp::Jump(_) => "jump",
            IrOp::Branch { .. } => "branch",
            IrOp::Return => "return",
            IrOp::ReturnUndefined => "return_undefined",
            IrOp::Catch(_) => "catch",
            IrOp::NipCatch => "nip_catch",
            IrOp::Throw => "throw",
            IrOp::ThrowError { .. } => "throw_error",
            IrOp::Gosub { .. } => "gosub",
            IrOp::Ret { .. } => "ret",
        }
    }

    #[test]
    fn unresolved_numeric_constant_is_guarded_at_use_instead_of_retrying_at_entry() {
        let ir = linear_ir(
            vec![instruction(0, IrOp::ResolveConstant(0)), numeric_push()],
            IrOp::Binary(BinaryOp::Mul),
        );
        let analysis = analyze_entry_domains(&ir).expect("constant arithmetic IR is valid");
        assert!(!analysis.retry_before_entry);
        assert!(analysis.requirements.is_empty());
    }

    #[test]
    fn entry_domain_analysis_exhaustively_handles_every_ir_op_variant() {
        let state = FrameStateId::from_index(0).unwrap();
        let mut cases = vec![
            linear_ir(
                Vec::new(),
                IrOp::Poll {
                    state,
                    kind: crate::ir::PollKind::Entry,
                },
            ),
            linear_ir(Vec::new(), IrOp::OsrLabel { state }),
            linear_ir(Vec::new(), IrOp::Nop),
            linear_ir(
                Vec::new(),
                IrOp::Push(TaggedValue::new(1, qjs::JS_TAG_INT as i64)),
            ),
            linear_ir(Vec::new(), IrOp::ResolveConstant(0)),
            linear_ir(Vec::new(), IrOp::GetGlobal(0)),
            linear_ir(Vec::new(), IrOp::NewObject),
            linear_ir(vec![numeric_push(), numeric_push()], IrOp::NewArrayFrom(2)),
            linear_ir(vec![numeric_push()], IrOp::GetProperty(1)),
            linear_ir(vec![numeric_push()], IrOp::GetPropertyKeep(1)),
            linear_ir(vec![numeric_push(), numeric_push()], IrOp::SetProperty(1)),
            linear_ir(
                vec![numeric_push(), numeric_push()],
                IrOp::Call {
                    argc: 1,
                    has_this: false,
                },
            ),
            linear_ir(
                vec![numeric_push(), numeric_push()],
                IrOp::CallConstructor(0),
            ),
            linear_ir(vec![numeric_push(), numeric_push()], IrOp::Regexp),
            linear_ir(Vec::new(), IrOp::FClosure(0)),
            linear_ir(
                Vec::new(),
                IrOp::GetVarRef {
                    index: 0,
                    checked: true,
                },
            ),
            linear_ir(
                vec![numeric_push()],
                IrOp::PutVarRef {
                    index: 0,
                    mode: VarRefMode::Check,
                    keep: false,
                },
            ),
            linear_ir(
                vec![numeric_push()],
                IrOp::PutVarRef {
                    index: 0,
                    mode: VarRefMode::Plain,
                    keep: true,
                },
            ),
            linear_ir(Vec::new(), IrOp::CloseLocal(0)),
            linear_ir(vec![numeric_push()], IrOp::SetName(1)),
            linear_ir(Vec::new(), IrOp::GetArgument(0)),
            linear_ir(Vec::new(), IrOp::GetLocal(0)),
            linear_ir(Vec::new(), IrOp::GetLocalChecked(0)),
            linear_ir(Vec::new(), IrOp::GetLocalPair),
            linear_ir(
                vec![numeric_push()],
                IrOp::PutArgument {
                    index: 0,
                    keep: false,
                },
            ),
            linear_ir(
                vec![numeric_push()],
                IrOp::PutLocal {
                    index: 0,
                    keep: false,
                },
            ),
            linear_ir(
                vec![numeric_push()],
                IrOp::PutLocalChecked {
                    index: 0,
                    initialize: true,
                },
            ),
            linear_ir(Vec::new(), IrOp::SetLocalUninitialized(0)),
            linear_ir(vec![numeric_push()], IrOp::Drop),
            linear_ir(vec![numeric_push()], IrOp::Unary(UnaryOp::Plus)),
            linear_ir(vec![numeric_push()], IrOp::PostUnary(UnaryOp::Increment)),
            linear_ir(
                Vec::new(),
                IrOp::LocalUnary {
                    index: 0,
                    op: UnaryOp::Increment,
                },
            ),
            linear_ir(vec![numeric_push()], IrOp::AddLocal(0)),
            linear_ir(vec![numeric_push()], IrOp::Return),
            linear_ir(Vec::new(), IrOp::ReturnUndefined),
            linear_ir(Vec::new(), IrOp::Catch(1)),
            linear_ir(
                vec![instruction(0, IrOp::Catch(1)), numeric_push()],
                IrOp::NipCatch,
            ),
            linear_ir(vec![numeric_push()], IrOp::Throw),
            linear_ir(Vec::new(), IrOp::ThrowError { atom: 1, kind: 2 }),
        ];
        cases.push(BaselineIr {
            blocks: vec![
                IrBlock {
                    start_pc: 0,
                    stack_depth: 0,
                    instructions: vec![instruction(
                        0,
                        IrOp::Gosub {
                            target: 2,
                            return_pc: 1,
                        },
                    )],
                },
                IrBlock {
                    start_pc: 1,
                    stack_depth: 0,
                    instructions: vec![instruction(1, IrOp::ReturnUndefined)],
                },
                IrBlock {
                    start_pc: 2,
                    stack_depth: 1,
                    instructions: vec![instruction(
                        2,
                        IrOp::Ret {
                            targets: Box::new([1]),
                        },
                    )],
                },
            ],
            frame_states: FrameStateTable::default(),
            max_stack_depth: 1,
            argument_count: 0,
            local_count: 0,
            exception_handlers: BTreeMap::new(),
            catch_sites: BTreeMap::new(),
        });
        for operation in [
            StackOp::Nip,
            StackOp::Nip1,
            StackOp::Dup,
            StackOp::Dup1,
            StackOp::Dup2,
            StackOp::Dup3,
            StackOp::Insert2,
            StackOp::Insert3,
            StackOp::Insert4,
            StackOp::Perm3,
            StackOp::Perm4,
            StackOp::Perm5,
            StackOp::Swap,
            StackOp::Swap2,
            StackOp::Rot3Left,
            StackOp::Rot3Right,
            StackOp::Rot4Left,
            StackOp::Rot5Left,
        ] {
            let (take, _) = stack_operation_order(operation);
            cases.push(linear_ir(
                (0..take).map(|_| numeric_push()).collect(),
                IrOp::Stack(operation),
            ));
        }
        for operation in [
            UnaryOp::IsUndefinedOrNull,
            UnaryOp::IsUndefined,
            UnaryOp::IsNull,
            UnaryOp::Plus,
            UnaryOp::Neg,
            UnaryOp::Increment,
            UnaryOp::Decrement,
            UnaryOp::BitNot,
            UnaryOp::LogicalNot,
        ] {
            cases.push(linear_ir(vec![numeric_push()], IrOp::Unary(operation)));
        }
        for operation in [
            BinaryOp::Add,
            BinaryOp::Sub,
            BinaryOp::Mul,
            BinaryOp::Div,
            BinaryOp::Mod,
            BinaryOp::BitAnd,
            BinaryOp::BitOr,
            BinaryOp::BitXor,
            BinaryOp::ShiftLeft,
            BinaryOp::ShiftRight,
            BinaryOp::ShiftRightUnsigned,
            BinaryOp::LessThan,
            BinaryOp::LessThanOrEqual,
            BinaryOp::GreaterThan,
            BinaryOp::GreaterThanOrEqual,
            BinaryOp::Equal,
            BinaryOp::NotEqual,
            BinaryOp::StrictEqual,
            BinaryOp::StrictNotEqual,
        ] {
            cases.push(linear_ir(
                vec![numeric_push(), numeric_push()],
                IrOp::Binary(operation),
            ));
        }
        cases.push(BaselineIr {
            blocks: vec![
                IrBlock {
                    start_pc: 0,
                    stack_depth: 0,
                    instructions: vec![instruction(0, IrOp::Jump(1))],
                },
                IrBlock {
                    start_pc: 1,
                    stack_depth: 0,
                    instructions: vec![instruction(1, IrOp::ReturnUndefined)],
                },
            ],
            frame_states: FrameStateTable::default(),
            max_stack_depth: 0,
            argument_count: 0,
            local_count: 0,
            exception_handlers: BTreeMap::new(),
            catch_sites: BTreeMap::new(),
        });
        cases.push(BaselineIr {
            blocks: vec![
                IrBlock {
                    start_pc: 0,
                    stack_depth: 0,
                    instructions: vec![
                        numeric_push(),
                        instruction(
                            0,
                            IrOp::Branch {
                                target: 2,
                                when_true: true,
                            },
                        ),
                    ],
                },
                IrBlock {
                    start_pc: 1,
                    stack_depth: 0,
                    instructions: vec![instruction(1, IrOp::ReturnUndefined)],
                },
                IrBlock {
                    start_pc: 2,
                    stack_depth: 0,
                    instructions: vec![instruction(2, IrOp::ReturnUndefined)],
                },
            ],
            frame_states: FrameStateTable::default(),
            max_stack_depth: 1,
            argument_count: 0,
            local_count: 0,
            exception_handlers: BTreeMap::new(),
            catch_sites: BTreeMap::new(),
        });

        let mut seen = BTreeSet::new();
        for ir in cases {
            for operation in ir
                .blocks
                .iter()
                .flat_map(|block| &block.instructions)
                .map(|instruction| &instruction.op)
            {
                seen.insert(ir_op_variant(operation));
            }
            let analysis = analyze_entry_domains(&ir).expect("synthetic IR is structurally valid");
            // Every operation with an unknown operand domain is either
            // helper-backed or guarded at its use site, so no synthetic case
            // forces the whole function to retry before entry.
            assert!(!analysis.retry_before_entry, "{ir:?}");
        }
        assert_eq!(
            seen.len(),
            44,
            "every IrOp variant is represented: {seen:?}"
        );
    }
}

#[cfg(test)]
mod post_lowering_tests {
    use super::{canonicalize_integer_constants, fold_bitcast_round_trips, fold_constant_branches};
    use cranelift_codegen::ir::{
        condcodes::IntCC, types, AbiParam, Function, InstBuilder, Signature,
    };
    use cranelift_codegen::isa::CallConv;
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};

    fn verify(function: &Function) {
        cranelift_codegen::verify_function(
            function,
            &cranelift_codegen::settings::Flags::new(cranelift_codegen::settings::builder()),
        )
        .unwrap_or_else(|errors| panic!("{errors}: {}", function.display()));
    }

    fn signature(params: &[types::Type], returns: &[types::Type]) -> Signature {
        let mut signature = Signature::new(CallConv::SystemV);
        signature
            .params
            .extend(params.iter().map(|&ty| AbiParam::new(ty)));
        signature
            .returns
            .extend(returns.iter().map(|&ty| AbiParam::new(ty)));
        signature
    }

    /// A loop whose `tag` variable is redefined to a fresh `iconst 0` on the
    /// backedge, as the lowering does for Int32 tags.
    #[test]
    fn loop_invariant_constants_share_one_definition() {
        let mut function = Function::with_name_signature(
            Default::default(),
            signature(&[types::I64], &[types::I64]),
        );
        let mut context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        let header = builder.create_block();
        let body = builder.create_block();
        let exit = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        let counter = Variable::from_u32(0);
        let tag = Variable::from_u32(1);
        builder.declare_var(counter, types::I64);
        builder.declare_var(tag, types::I64);
        builder.switch_to_block(entry);
        let x = builder.block_params(entry)[0];
        builder.def_var(counter, x);
        let zero = builder.ins().iconst(types::I64, 0);
        builder.def_var(tag, zero);
        builder.ins().jump(header, &[]);
        builder.switch_to_block(header);
        let current = builder.use_var(counter);
        let done = builder.ins().icmp_imm(IntCC::Equal, current, 0);
        builder.ins().brif(done, exit, &[], body, &[]);
        builder.switch_to_block(body);
        let next = builder.ins().iadd_imm(current, -1);
        builder.def_var(counter, next);
        let again = builder.ins().iconst(types::I64, 0);
        builder.def_var(tag, again);
        builder.ins().jump(header, &[]);
        builder.switch_to_block(exit);
        let result = builder.use_var(tag);
        builder.ins().return_(&[result]);
        builder.seal_all_blocks();
        builder.finalize();

        canonicalize_integer_constants(&mut function);
        verify(&function);
        let text = function.display().to_string();
        assert_eq!(text.matches("iconst.i64 0").count(), 1, "{text}");
        // Every edge into the header now passes the same zero, which is what
        // lets Cranelift's constant-phi removal delete the tag parameter.
        let jumps = text
            .lines()
            .filter(|line| line.trim_start().starts_with("jump block1"))
            .collect::<Vec<_>>();
        assert_eq!(jumps.len(), 2, "{text}");
        let zero = text
            .lines()
            .find_map(|line| {
                let line = line.trim();
                line.ends_with("iconst.i64 0")
                    .then(|| line.split_whitespace().next().unwrap().to_owned())
            })
            .unwrap();
        assert!(
            jumps.iter().all(|jump| jump.contains(&format!("{zero})"))),
            "{text}"
        );
    }

    #[test]
    fn constant_conditions_become_jumps_and_drop_dead_exits() {
        let mut function = Function::with_name_signature(
            Default::default(),
            signature(&[types::I64], &[types::I64]),
        );
        let mut context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        let pass = builder.create_block();
        let deopt = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.append_block_param(pass, types::I64);
        builder.switch_to_block(entry);
        let x = builder.block_params(entry)[0];
        let tag = builder.ins().iconst(types::I64, 0);
        let int = builder.ins().icmp_imm(IntCC::Equal, tag, 0);
        let float = builder.ins().icmp_imm(IntCC::Equal, tag, 8);
        let one = builder.ins().iconst(types::I8, 1);
        let numeric = builder.ins().bor(int, float);
        let numeric = builder.ins().band(one, numeric);
        builder.ins().brif(numeric, pass, &[x], deopt, &[]);
        builder.switch_to_block(pass);
        let value = builder.block_params(pass)[0];
        builder.ins().return_(&[value]);
        builder.switch_to_block(deopt);
        let marker = builder.ins().iconst(types::I64, -1);
        builder.ins().return_(&[marker]);
        builder.seal_all_blocks();
        builder.finalize();

        canonicalize_integer_constants(&mut function);
        fold_constant_branches(&mut function);
        verify(&function);
        let folded = function.display().to_string();
        assert!(!folded.contains("brif"), "{folded}");
        assert!(folded.contains("jump block1(v0)"), "{folded}");
        assert!(!folded.contains("block2"), "dead exit removed: {folded}");
    }

    #[test]
    fn runtime_conditions_are_left_alone() {
        let mut function = Function::with_name_signature(
            Default::default(),
            signature(&[types::I64], &[types::I64]),
        );
        let mut context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        let left = builder.create_block();
        let right = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let x = builder.block_params(entry)[0];
        let small = builder.ins().icmp_imm(IntCC::UnsignedLessThan, x, 8);
        let zero = builder.ins().iconst(types::I8, 0);
        // `small | 0` still depends on x; `x & 0` would not.
        let condition = builder.ins().bor(small, zero);
        builder.ins().brif(condition, left, &[], right, &[]);
        builder.switch_to_block(left);
        builder.ins().return_(&[x]);
        builder.switch_to_block(right);
        let zero = builder.ins().iconst(types::I64, 0);
        builder.ins().return_(&[zero]);
        builder.seal_all_blocks();
        builder.finalize();
        fold_constant_branches(&mut function);
        verify(&function);
        assert!(function.display().to_string().contains("brif"));
    }

    #[test]
    fn narrow_comparisons_fold_with_their_type_width() {
        let mut function =
            Function::with_name_signature(Default::default(), signature(&[], &[types::I32]));
        let mut context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        let taken = builder.create_block();
        let other = builder.create_block();
        builder.switch_to_block(entry);
        // -1 as i32 is below zero signed but the largest value unsigned, and
        // its zero extension is positive.
        let minus_one = builder.ins().iconst(types::I32, -1);
        let wide = builder.ins().uextend(types::I64, minus_one);
        let unsigned_big = builder
            .ins()
            .icmp_imm(IntCC::UnsignedGreaterThan, minus_one, 7);
        let signed_negative = builder.ins().icmp_imm(IntCC::SignedLessThan, minus_one, 0);
        let widened_positive = builder.ins().icmp_imm(IntCC::SignedGreaterThan, wide, 0);
        let both = builder.ins().band(unsigned_big, widened_positive);
        let all = builder.ins().band(both, signed_negative);
        builder.ins().brif(all, taken, &[], other, &[]);
        builder.switch_to_block(taken);
        let one = builder.ins().iconst(types::I32, 1);
        builder.ins().return_(&[one]);
        builder.switch_to_block(other);
        let two = builder.ins().iconst(types::I32, 2);
        builder.ins().return_(&[two]);
        builder.seal_all_blocks();
        builder.finalize();
        fold_constant_branches(&mut function);
        verify(&function);
        let folded = function.display().to_string();
        assert!(folded.contains("jump block1"), "{folded}");
        assert!(!folded.contains("block2"), "{folded}");
    }

    #[test]
    fn float_payload_round_trips_forward_the_original_value() {
        use cranelift_codegen::ir::MemFlags;
        let mut function = Function::with_name_signature(
            Default::default(),
            signature(&[types::F64, types::I64], &[types::F64, types::I64]),
        );
        let mut context = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut function, &mut context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let float = builder.block_params(entry)[0];
        let bits = builder.block_params(entry)[1];
        let payload = builder.ins().bitcast(types::I64, MemFlags::new(), float);
        let back = builder.ins().bitcast(types::F64, MemFlags::new(), payload);
        let sum = builder.ins().fadd(back, back);
        // An integer payload viewed as f64 and back is also the identity.
        let viewed = builder.ins().bitcast(types::F64, MemFlags::new(), bits);
        let raw = builder.ins().bitcast(types::I64, MemFlags::new(), viewed);
        builder.ins().return_(&[sum, raw]);
        builder.seal_all_blocks();
        builder.finalize();
        fold_bitcast_round_trips(&mut function);
        verify(&function);
        let text = function.display().to_string();
        assert!(text.contains("fadd v0, v0"), "{text}");
        assert!(text.contains("return v4, v1"), "{text}");
    }
}
