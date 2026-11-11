//! Target-only linked leaf entry for heterogeneous, guarded arguments.
//!
//! The entry is a transaction over borrowed arguments. Every speculation
//! (arithmetic overflow, property shape/generation/tag guards) is checked
//! before the first observable write. Guarded own-field stores are buffered in
//! SSA and committed in program order only at `return`, together with the
//! scalar result. Status `1` therefore always means "nothing happened": the
//! caller executes its original generic CALL with an untouched frame and heap.
//! This is the JSC/V8 "exit before the first effect" rule applied to a whole,
//! bounded, acyclic callee, so no callee frame ever needs to be materialized.

use crate::{
    abi::PropertyLayout,
    bytecode::VerifiedFunction,
    compiler::{CompileControl, CompileFailure},
    ir::{eligible_property_observation, BaselineIr, BinaryOp, IrOp, StackOp, UnaryOp},
    runtime::{
        BoundedSpecializationSignature, FeedbackRepresentation, FeedbackSnapshot, ObservedType,
        ShapeFeedbackState, ShapeObservation,
    },
};
use cranelift_codegen::{
    ir::{
        condcodes::IntCC, types, AbiParam, Block, Function, InstBuilder, MemFlags, Signature, Value,
    },
    isa::OwnedTargetIsa,
};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use rquickjs_core::qjs;
use std::collections::BTreeMap;

/// Bounds shared with the bounded inline planner. Admission happens before
/// BaselineIr translation or Cranelift block creation.
const MAX_INSTRUCTIONS: usize = 128;
const MAX_SLOTS: usize = 64;
const MAX_RETAINED_BYTES: usize = 16 * 1024;
const MAX_BLOCKS: usize = 16;
const MAX_BUFFERED_STORES: usize = 8;

#[derive(Clone, Copy)]
struct TypedValue {
    value: Value,
    representation: FeedbackRepresentation,
}

/// One guarded own-field store, buffered until the successful `return`.
struct BufferedStore {
    pc: u32,
    dirty: Variable,
    address: Variable,
    payload: Variable,
    tag: i32,
}

fn observed_representation(value: ObservedType) -> Option<FeedbackRepresentation> {
    match value {
        ObservedType::Int32 => Some(FeedbackRepresentation::Int32),
        ObservedType::Float64 => Some(FeedbackRepresentation::Float64),
        ObservedType::Bool => Some(FeedbackRepresentation::Bool),
        _ => None,
    }
}

fn representation_tag(representation: FeedbackRepresentation) -> Result<i32, CompileFailure> {
    match representation {
        FeedbackRepresentation::Int32 => Ok(qjs::JS_TAG_INT),
        FeedbackRepresentation::Float64 => Ok(qjs::JS_TAG_FLOAT64),
        FeedbackRepresentation::Bool => Ok(qjs::JS_TAG_BOOL),
        FeedbackRepresentation::HeapRef => Err(CompileFailure::UnsupportedOpcode),
    }
}

fn scalar_type(representation: FeedbackRepresentation, pointer: types::Type) -> types::Type {
    match representation {
        FeedbackRepresentation::Int32 | FeedbackRepresentation::Bool => types::I32,
        FeedbackRepresentation::Float64 => types::F64,
        FeedbackRepresentation::HeapRef => pointer,
    }
}

/// A property site is admitted only for one stable own writable data field
/// holding a representable primitive. Anything else keeps the generic CALL.
fn linked_property(
    feedback: Option<&FeedbackSnapshot>,
    pc: u32,
) -> Result<(ShapeObservation, FeedbackRepresentation), CompileFailure> {
    let site = feedback
        .and_then(|feedback| feedback.property_at(pc))
        .ok_or(CompileFailure::UnsupportedOpcode)?;
    let [observation] = site.observations() else {
        return Err(CompileFailure::UnsupportedOpcode);
    };
    if site.state() != ShapeFeedbackState::Monomorphic
        || !eligible_property_observation(*observation)
    {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    let representation =
        observed_representation(observation.value()).ok_or(CompileFailure::UnsupportedOpcode)?;
    Ok((*observation, representation))
}

/// Guards `object` (an already tag-checked, borrowed object pointer) against
/// the observed shape identity and generation, then the field's current tag.
/// Returns the field address. No memory is written on any path.
fn emit_guarded_field(
    builder: &mut FunctionBuilder<'_>,
    layout: PropertyLayout,
    pointer_type: types::Type,
    object: Value,
    observation: ShapeObservation,
    expected_tag: i32,
    miss: Block,
) -> Result<Value, CompileFailure> {
    let offset = observation
        .offset()
        .checked_mul(16)
        .map(i64::from)
        .filter(|offset| *offset <= i64::from(i32::MAX) - 8)
        .ok_or(CompileFailure::ResourceLimit)?;
    let shape = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        object,
        layout.object_shape_offset,
    );
    let same_shape =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, shape, observation.shape().identity() as i64);
    let generation = builder.ins().load(
        types::I64,
        MemFlags::new(),
        shape,
        layout.shape_generation_offset,
    );
    let same_generation = builder.ins().icmp_imm(
        IntCC::Equal,
        generation,
        observation.shape().generation() as i64,
    );
    let shape_ok = builder.ins().band(same_shape, same_generation);
    let field = builder.create_block();
    builder.ins().brif(shape_ok, field, &[], miss, &[]);
    builder.switch_to_block(field);
    let properties = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        object,
        layout.object_properties_offset,
    );
    let address = builder.ins().iadd_imm(properties, offset);
    let tag = builder.ins().load(types::I64, MemFlags::new(), address, 8);
    let tag_ok = builder
        .ins()
        .icmp_imm(IntCC::Equal, tag, i64::from(expected_tag));
    let checked = builder.create_block();
    builder.ins().brif(tag_ok, checked, &[], miss, &[]);
    builder.switch_to_block(checked);
    Ok(address)
}

fn emit_int32_arithmetic(
    builder: &mut FunctionBuilder<'_>,
    operation: BinaryOp,
    lhs: Value,
    rhs: Value,
    miss: Block,
) -> Value {
    let (value, overflow) = match operation {
        BinaryOp::Add => builder.ins().sadd_overflow(lhs, rhs),
        BinaryOp::Sub => builder.ins().ssub_overflow(lhs, rhs),
        BinaryOp::Mul => builder.ins().smul_overflow(lhs, rhs),
        _ => unreachable!("caller admits only add/sub/mul"),
    };
    let must_miss = if operation == BinaryOp::Mul {
        let zero = builder.ins().icmp_imm(IntCC::Equal, value, 0);
        let lhs_negative = builder.ins().icmp_imm(IntCC::SignedLessThan, lhs, 0);
        let rhs_negative = builder.ins().icmp_imm(IntCC::SignedLessThan, rhs, 0);
        let opposite_signs = builder.ins().bxor(lhs_negative, rhs_negative);
        let negative_zero = builder.ins().band(zero, opposite_signs);
        builder.ins().bor(overflow, negative_zero)
    } else {
        overflow
    };
    let ok = builder.create_block();
    builder.ins().brif(must_miss, miss, &[], ok, &[]);
    builder.switch_to_block(ok);
    value
}

fn truth(builder: &mut FunctionBuilder<'_>, value: TypedValue) -> Result<Value, CompileFailure> {
    match value.representation {
        FeedbackRepresentation::Int32 | FeedbackRepresentation::Bool => {
            Ok(builder.ins().icmp_imm(IntCC::NotEqual, value.value, 0))
        }
        FeedbackRepresentation::Float64 | FeedbackRepresentation::HeapRef => {
            Err(CompileFailure::UnsupportedOpcode)
        }
    }
}

/// Build a JSC-style linked call entry over guarded, borrowed arguments.
///
/// Target identity and argument-tag guards live at the call site. This entry
/// performs no allocation, ownership transfer, helper call, or frame recovery.
/// Status `0` stores the scalar result after committing buffered field stores;
/// status `1` means the caller must execute the original generic CALL with its
/// untouched frame, and guarantees that no heap write happened.
pub(crate) fn lower_target_only_linked_leaf(
    isa: &OwnedTargetIsa,
    function: &VerifiedFunction,
    signature: &BoundedSpecializationSignature,
    feedback: Option<&FeedbackSnapshot>,
    control: Option<&CompileControl>,
) -> Result<super::baseline::RelocatableCode, CompileFailure> {
    let snapshot = function.snapshot();
    let slots = usize::from(snapshot.arg_count())
        .checked_add(usize::from(snapshot.local_count()))
        .and_then(|count| count.checked_add(usize::from(snapshot.stack_size())))
        .ok_or(CompileFailure::ResourceLimit)?;
    // Match the existing bounded inline planner. This admission happens before
    // BaselineIr translation or Cranelift block creation, so rejected input
    // cannot turn a stable call-link observation into unbounded compilation.
    if function.instructions().len() > MAX_INSTRUCTIONS
        || slots > MAX_SLOTS
        || snapshot.retained_bytes() > MAX_RETAINED_BYTES
        || function.control_flow_graph().blocks().len() > MAX_BLOCKS
    {
        return Err(CompileFailure::ResourceLimit);
    }
    if signature.function().id != snapshot.function_id()
        || signature.function().generation != snapshot.generation()
        || signature.arity() != usize::from(snapshot.arg_count())
        || !snapshot.exception_map().is_empty()
        || function.has_exception_regions()
        || !matches!(
            signature.result(),
            FeedbackRepresentation::Int32
                | FeedbackRepresentation::Float64
                | FeedbackRepresentation::Bool
        )
        || !signature
            .arguments()
            .contains(&FeedbackRepresentation::HeapRef)
    {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    if let Some(control) = control {
        control.check()?;
    }

    let ir = BaselineIr::translate(function)?;
    if ir.blocks.is_empty() || ir.blocks.len() > MAX_BLOCKS {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    // Structural prepass: forward-only control flow with an empty operand
    // stack at every block boundary, and no field read after any field store.
    // In a forward-only CFG every path orders instructions by PC, so a read
    // with a PC above the first store PC is the only way to observe a buffered
    // write; rejecting it keeps the deferred-commit transaction exact.
    let block_starts = ir
        .blocks
        .iter()
        .map(|block| block.start_pc)
        .collect::<Vec<_>>();
    if block_starts.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(CompileFailure::InvalidArtifact);
    }
    let mut first_store = None::<u32>;
    let mut store_pcs = Vec::new();
    for block in &ir.blocks {
        if block.stack_depth != 0 {
            return Err(CompileFailure::UnsupportedOpcode);
        }
        for instruction in &block.instructions {
            match instruction.op {
                IrOp::Jump(target) | IrOp::Branch { target, .. } => {
                    if target <= instruction.pc || block_starts.binary_search(&target).is_err() {
                        return Err(CompileFailure::UnsupportedOpcode);
                    }
                }
                IrOp::SetProperty(_) => {
                    first_store.get_or_insert(instruction.pc);
                    store_pcs.push(instruction.pc);
                }
                IrOp::GetProperty(_) | IrOp::GetPropertyKeep(_)
                    if first_store.is_some_and(|store| store < instruction.pc) =>
                {
                    return Err(CompileFailure::UnsupportedOpcode);
                }
                _ => {}
            }
        }
    }
    if store_pcs.len() > MAX_BUFFERED_STORES {
        return Err(CompileFailure::ResourceLimit);
    }
    let layout = if store_pcs.is_empty()
        && !ir.blocks.iter().any(|block| {
            block.instructions.iter().any(|instruction| {
                matches!(
                    instruction.op,
                    IrOp::GetProperty(_) | IrOp::GetPropertyKeep(_)
                )
            })
        }) {
        None
    } else {
        Some(
            crate::abi::AbiInfo::linked()
                .map_err(|_| CompileFailure::InvalidArtifact)?
                .property_layout(),
        )
    };
    let pointer_type = isa.pointer_type();

    let mut abi = Signature::new(isa.default_call_conv());
    abi.params.push(AbiParam::new(pointer_type));
    abi.params.extend(
        signature
            .arguments()
            .iter()
            .map(|representation| AbiParam::new(scalar_type(*representation, pointer_type))),
    );
    abi.returns.push(AbiParam::new(types::I32));

    let mut function_ir = Function::with_name_signature(Default::default(), abi);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut function_ir, &mut context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let output = builder.block_params(entry)[0];
        let arguments = builder.block_params(entry)[1..].to_vec();
        let miss = builder.create_block();
        builder.set_cold_block(miss);

        let mut stores = Vec::with_capacity(store_pcs.len());
        let mut next_variable = 0u32;
        for &pc in &store_pcs {
            let (_, representation) = linked_property(feedback, pc)?;
            let store = BufferedStore {
                pc,
                dirty: Variable::from_u32(next_variable),
                address: Variable::from_u32(next_variable + 1),
                payload: Variable::from_u32(next_variable + 2),
                tag: representation_tag(representation)?,
            };
            next_variable += 3;
            builder.declare_var(store.dirty, types::I8);
            builder.declare_var(store.address, pointer_type);
            builder.declare_var(store.payload, types::I64);
            let clean = builder.ins().iconst(types::I8, 0);
            builder.def_var(store.dirty, clean);
            let null = builder.ins().iconst(pointer_type, 0);
            builder.def_var(store.address, null);
            let zero = builder.ins().iconst(types::I64, 0);
            builder.def_var(store.payload, zero);
            stores.push(store);
        }

        let blocks = ir
            .blocks
            .iter()
            .map(|block| (block.start_pc, builder.create_block()))
            .collect::<BTreeMap<_, _>>();
        builder.ins().jump(blocks[&ir.blocks[0].start_pc], &[]);

        for (block_index, block) in ir.blocks.iter().enumerate() {
            builder.switch_to_block(blocks[&block.start_pc]);
            let mut stack = Vec::<TypedValue>::new();
            let mut terminated = false;
            for instruction in &block.instructions {
                if terminated {
                    return Err(CompileFailure::InvalidArtifact);
                }
                match &instruction.op {
                    IrOp::Poll { .. } | IrOp::OsrLabel { .. } | IrOp::Nop => {}
                    IrOp::Push(tagged) if tagged.tag == i64::from(qjs::JS_TAG_INT) => {
                        stack.push(TypedValue {
                            value: builder
                                .ins()
                                .iconst(types::I32, tagged.payload as i32 as i64),
                            representation: FeedbackRepresentation::Int32,
                        });
                    }
                    IrOp::Push(tagged) if tagged.tag == i64::from(qjs::JS_TAG_BOOL) => {
                        stack.push(TypedValue {
                            value: builder
                                .ins()
                                .iconst(types::I32, i64::from(tagged.payload != 0)),
                            representation: FeedbackRepresentation::Bool,
                        });
                    }
                    IrOp::Push(tagged) if tagged.tag == i64::from(qjs::JS_TAG_FLOAT64) => {
                        stack.push(TypedValue {
                            value: builder.ins().f64const(f64::from_bits(tagged.payload)),
                            representation: FeedbackRepresentation::Float64,
                        });
                    }
                    IrOp::GetArgument(index) => {
                        let index = usize::from(*index);
                        stack.push(TypedValue {
                            value: *arguments
                                .get(index)
                                .ok_or(CompileFailure::InvalidArtifact)?,
                            representation: *signature
                                .arguments()
                                .get(index)
                                .ok_or(CompileFailure::InvalidArtifact)?,
                        });
                    }
                    IrOp::Drop => {
                        stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    }
                    // Typed values are borrowed or immediate: stack shuffles
                    // carry no ownership and need no DUP/FREE.
                    IrOp::Stack(StackOp::Dup) => {
                        let top = *stack.last().ok_or(CompileFailure::InvalidArtifact)?;
                        stack.push(top);
                    }
                    IrOp::Stack(StackOp::Swap) => {
                        let len = stack.len();
                        if len < 2 {
                            return Err(CompileFailure::InvalidArtifact);
                        }
                        stack.swap(len - 1, len - 2);
                    }
                    IrOp::Binary(operation @ (BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul)) => {
                        let rhs = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        let lhs = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        if lhs.representation != rhs.representation {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        let value = match lhs.representation {
                            FeedbackRepresentation::Int32 => emit_int32_arithmetic(
                                &mut builder,
                                *operation,
                                lhs.value,
                                rhs.value,
                                miss,
                            ),
                            FeedbackRepresentation::Float64 => match operation {
                                BinaryOp::Add => builder.ins().fadd(lhs.value, rhs.value),
                                BinaryOp::Sub => builder.ins().fsub(lhs.value, rhs.value),
                                BinaryOp::Mul => builder.ins().fmul(lhs.value, rhs.value),
                                _ => unreachable!(),
                            },
                            FeedbackRepresentation::Bool | FeedbackRepresentation::HeapRef => {
                                return Err(CompileFailure::UnsupportedOpcode)
                            }
                        };
                        stack.push(TypedValue {
                            value,
                            representation: lhs.representation,
                        });
                    }
                    IrOp::Binary(
                        operation @ (BinaryOp::LessThan
                        | BinaryOp::LessThanOrEqual
                        | BinaryOp::GreaterThan
                        | BinaryOp::GreaterThanOrEqual
                        | BinaryOp::StrictEqual
                        | BinaryOp::StrictNotEqual),
                    ) => {
                        let rhs = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        let lhs = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        let equality =
                            matches!(operation, BinaryOp::StrictEqual | BinaryOp::StrictNotEqual);
                        let admitted = lhs.representation == rhs.representation
                            && (lhs.representation == FeedbackRepresentation::Int32
                                || (equality
                                    && lhs.representation == FeedbackRepresentation::Bool));
                        if !admitted {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        let condition = match operation {
                            BinaryOp::LessThan => IntCC::SignedLessThan,
                            BinaryOp::LessThanOrEqual => IntCC::SignedLessThanOrEqual,
                            BinaryOp::GreaterThan => IntCC::SignedGreaterThan,
                            BinaryOp::GreaterThanOrEqual => IntCC::SignedGreaterThanOrEqual,
                            BinaryOp::StrictEqual => IntCC::Equal,
                            _ => IntCC::NotEqual,
                        };
                        let result = builder.ins().icmp(condition, lhs.value, rhs.value);
                        stack.push(TypedValue {
                            value: builder.ins().uextend(types::I32, result),
                            representation: FeedbackRepresentation::Bool,
                        });
                    }
                    IrOp::Unary(UnaryOp::LogicalNot) => {
                        let input = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        let truthy = truth(&mut builder, input)?;
                        let falsy = builder.ins().bxor_imm(truthy, 1);
                        stack.push(TypedValue {
                            value: builder.ins().uextend(types::I32, falsy),
                            representation: FeedbackRepresentation::Bool,
                        });
                    }
                    IrOp::Unary(operation @ (UnaryOp::Increment | UnaryOp::Decrement))
                    | IrOp::PostUnary(operation @ (UnaryOp::Increment | UnaryOp::Decrement)) => {
                        let input = *stack.last().ok_or(CompileFailure::InvalidArtifact)?;
                        if input.representation != FeedbackRepresentation::Int32 {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        let one = builder.ins().iconst(types::I32, 1);
                        let arithmetic = if *operation == UnaryOp::Increment {
                            BinaryOp::Add
                        } else {
                            BinaryOp::Sub
                        };
                        let value =
                            emit_int32_arithmetic(&mut builder, arithmetic, input.value, one, miss);
                        let updated = TypedValue {
                            value,
                            representation: FeedbackRepresentation::Int32,
                        };
                        // PostUnary keeps the old numeric value below the update.
                        if matches!(instruction.op, IrOp::Unary(_)) {
                            stack.pop();
                        }
                        stack.push(updated);
                    }
                    IrOp::GetProperty(_) | IrOp::GetPropertyKeep(_) => {
                        let receiver = *stack.last().ok_or(CompileFailure::InvalidArtifact)?;
                        if receiver.representation != FeedbackRepresentation::HeapRef {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        let (observation, representation) =
                            linked_property(feedback, instruction.pc)?;
                        let layout = layout.ok_or(CompileFailure::InvalidArtifact)?;
                        let address = emit_guarded_field(
                            &mut builder,
                            layout,
                            pointer_type,
                            receiver.value,
                            observation,
                            representation_tag(representation)?,
                            miss,
                        )?;
                        let value = builder.ins().load(
                            scalar_type(representation, pointer_type),
                            MemFlags::new(),
                            address,
                            0,
                        );
                        if matches!(instruction.op, IrOp::GetProperty(_)) {
                            stack.pop();
                        }
                        stack.push(TypedValue {
                            value,
                            representation,
                        });
                    }
                    IrOp::SetProperty(_) => {
                        let value = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        let receiver = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        let (observation, representation) =
                            linked_property(feedback, instruction.pc)?;
                        if receiver.representation != FeedbackRepresentation::HeapRef
                            || value.representation != representation
                        {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        let store = stores
                            .iter()
                            .find(|store| store.pc == instruction.pc)
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let layout = layout.ok_or(CompileFailure::InvalidArtifact)?;
                        // The current tag equals the stored tag, so the old
                        // value is a primitive: replacing it frees nothing.
                        let address = emit_guarded_field(
                            &mut builder,
                            layout,
                            pointer_type,
                            receiver.value,
                            observation,
                            store.tag,
                            miss,
                        )?;
                        let payload = match representation {
                            FeedbackRepresentation::Int32 => {
                                builder.ins().sextend(types::I64, value.value)
                            }
                            FeedbackRepresentation::Bool => {
                                builder.ins().uextend(types::I64, value.value)
                            }
                            FeedbackRepresentation::Float64 => {
                                builder
                                    .ins()
                                    .bitcast(types::I64, MemFlags::new(), value.value)
                            }
                            FeedbackRepresentation::HeapRef => {
                                return Err(CompileFailure::InvalidArtifact)
                            }
                        };
                        builder.def_var(store.address, address);
                        builder.def_var(store.payload, payload);
                        let dirty = builder.ins().iconst(types::I8, 1);
                        builder.def_var(store.dirty, dirty);
                    }
                    IrOp::Jump(target) => {
                        builder.ins().jump(blocks[target], &[]);
                        terminated = true;
                    }
                    IrOp::Branch { target, when_true } => {
                        let condition = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        if !stack.is_empty() {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        let truthy = truth(&mut builder, condition)?;
                        let fallthrough = ir
                            .blocks
                            .get(block_index + 1)
                            .map(|next| blocks[&next.start_pc])
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let (taken, other) = if *when_true {
                            (blocks[target], fallthrough)
                        } else {
                            (fallthrough, blocks[target])
                        };
                        builder.ins().brif(truthy, taken, &[], other, &[]);
                        terminated = true;
                    }
                    IrOp::Return => {
                        let result = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        if !stack.is_empty() || result.representation != signature.result() {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        // Commit in program order: on a forward-only CFG the
                        // dirty stores of this path are exactly those executed,
                        // and later same-field stores win as in the interpreter.
                        for store in stores.iter().filter(|store| store.pc < instruction.pc) {
                            let dirty = builder.use_var(store.dirty);
                            let commit = builder.create_block();
                            let done = builder.create_block();
                            builder.ins().brif(dirty, commit, &[], done, &[]);
                            builder.switch_to_block(commit);
                            let address = builder.use_var(store.address);
                            let payload = builder.use_var(store.payload);
                            builder.ins().store(MemFlags::new(), payload, address, 0);
                            let tag = builder.ins().iconst(types::I64, i64::from(store.tag));
                            builder.ins().store(MemFlags::new(), tag, address, 8);
                            builder.ins().jump(done, &[]);
                            builder.switch_to_block(done);
                        }
                        builder
                            .ins()
                            .store(MemFlags::new(), result.value, output, 0);
                        let status = builder.ins().iconst(types::I32, 0);
                        builder.ins().return_(&[status]);
                        terminated = true;
                    }
                    _ => return Err(CompileFailure::UnsupportedOpcode),
                }
            }
            if !terminated {
                if !stack.is_empty() {
                    return Err(CompileFailure::UnsupportedOpcode);
                }
                let next = ir
                    .blocks
                    .get(block_index + 1)
                    .map(|next| blocks[&next.start_pc])
                    .ok_or(CompileFailure::UnsupportedOpcode)?;
                builder.ins().jump(next, &[]);
            }
        }
        builder.switch_to_block(miss);
        let status = builder.ins().iconst(types::I32, 1);
        builder.ins().return_(&[status]);
        builder.seal_all_blocks();
        builder.finalize();
    }
    super::baseline::finalize_optimized_machine(isa, function_ir, control, false)
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use crate::{
        bytecode::VerifyLimits,
        runtime::{
            BinaryFeedbackFlags, FeedbackRepresentation, FeedbackTable, FunctionKey, ObservedType,
        },
        test_support::SnapshotFixture,
    };

    fn fixture_and_signature(
        source: &str,
        argument_types: &[ObservedType],
        binary_name: &str,
    ) -> (
        SnapshotFixture,
        crate::bytecode::VerifiedFunction,
        crate::runtime::BoundedSpecializationSignature,
    ) {
        let fixture = SnapshotFixture::compile(source);
        let verified = fixture
            .snapshot()
            .verify(VerifyLimits::default())
            .expect("verified linked leaf");
        let key = FunctionKey::new(
            verified.snapshot().function_id(),
            verified.snapshot().generation(),
        );
        let binary_pc = verified
            .instructions()
            .iter()
            .find(|instruction| instruction.opcode().name() == binary_name)
            .expect("binary opcode")
            .pc();
        let return_pc = verified
            .instructions()
            .iter()
            .find(|instruction| instruction.opcode().name() == "return")
            .expect("return opcode")
            .pc();
        let mut feedback = FeedbackTable::new(32, 2);
        for _ in 0..32 {
            feedback.observe_call(key, argument_types);
            feedback.observe_binary(
                key,
                binary_pc,
                ObservedType::Int32,
                ObservedType::Int32,
                ObservedType::Int32,
                BinaryFeedbackFlags::NONE,
            );
            feedback.observe_return(key, return_pc, ObservedType::Int32);
        }
        let signature = feedback
            .snapshot(91)
            .bounded_specialization(key)
            .expect("heterogeneous bounded signature");
        (fixture, verified, signature)
    }

    #[test]
    fn heapref_argument_leaf_executes_without_a_frame_or_helper() {
        let (_fixture, verified, signature) = fixture_and_signature(
            "(function(object, value){return value+1})",
            &[ObservedType::Object, ObservedType::Int32],
            "add",
        );
        assert_eq!(
            signature.arguments(),
            &[
                FeedbackRepresentation::HeapRef,
                FeedbackRepresentation::Int32
            ]
        );
        let isa = cranelift_native::builder()
            .expect("host ISA")
            .finish(cranelift_codegen::settings::Flags::new(
                cranelift_codegen::settings::builder(),
            ))
            .expect("host flags");
        let code = super::lower_target_only_linked_leaf(&isa, &verified, &signature, None, None)
            .expect("target-only linked leaf");
        assert!(
            code.clif().contains("(i64, i64, i32) -> i32"),
            "{}",
            code.clif()
        );
        assert!(!code.clif().contains("call_indirect"), "{}", code.clif());
        let published = code.publish().expect("publish linked leaf");
        let mut output = -1i32;
        let status = unsafe {
            core::mem::transmute::<*const u8, extern "C" fn(*mut i32, usize, i32) -> i32>(
                published.as_ptr(),
            )(&mut output, 0x1234usize, 41)
        };
        assert_eq!((status, output), (0, 42));
    }

    #[test]
    fn overflow_returns_miss_without_committing_output() {
        let (_fixture, verified, signature) = fixture_and_signature(
            "(function(object, value){return value+1})",
            &[ObservedType::Object, ObservedType::Int32],
            "add",
        );
        let isa = cranelift_native::builder()
            .expect("host ISA")
            .finish(cranelift_codegen::settings::Flags::new(
                cranelift_codegen::settings::builder(),
            ))
            .expect("host flags");
        let code = super::lower_target_only_linked_leaf(&isa, &verified, &signature, None, None)
            .expect("target-only linked leaf")
            .publish()
            .expect("publish linked leaf");
        let mut output = 77i32;
        let status = unsafe {
            core::mem::transmute::<*const u8, extern "C" fn(*mut i32, usize, i32) -> i32>(
                code.as_ptr(),
            )(&mut output, 0x1234usize, i32::MAX)
        };
        assert_eq!(
            status, 1,
            "miss must select the caller's generic CALL block"
        );
        assert_eq!(output, 77, "a miss must not expose a partial result");
    }

    #[test]
    fn int32_multiply_negative_zero_returns_miss() {
        let (_fixture, verified, signature) = fixture_and_signature(
            "(function(object, left, right){return left*right})",
            &[
                ObservedType::Object,
                ObservedType::Int32,
                ObservedType::Int32,
            ],
            "mul",
        );
        let isa = cranelift_native::builder()
            .expect("host ISA")
            .finish(cranelift_codegen::settings::Flags::new(
                cranelift_codegen::settings::builder(),
            ))
            .expect("host flags");
        let code = super::lower_target_only_linked_leaf(&isa, &verified, &signature, None, None)
            .expect("target-only linked leaf")
            .publish()
            .expect("publish linked leaf");
        let mut output = 77i32;
        let status = unsafe {
            core::mem::transmute::<*const u8, extern "C" fn(*mut i32, usize, i32, i32) -> i32>(
                code.as_ptr(),
            )(&mut output, 0x1234usize, -1, 0)
        };
        assert_eq!(status, 1, "-0 needs the generic JavaScript multiply path");
        assert_eq!(output, 77, "a miss must not expose Int32 zero");
    }

    #[test]
    fn heapref_link_can_return_a_guarded_bool_without_a_frame() {
        let fixture = SnapshotFixture::compile("(function(object, enabled){return enabled})");
        let verified = fixture
            .snapshot()
            .verify(VerifyLimits::default())
            .expect("verified linked bool leaf");
        let key = FunctionKey::new(
            verified.snapshot().function_id(),
            verified.snapshot().generation(),
        );
        let return_pc = verified
            .instructions()
            .iter()
            .find(|instruction| instruction.opcode().name() == "return")
            .expect("return opcode")
            .pc();
        let mut feedback = FeedbackTable::new(32, 2);
        for _ in 0..32 {
            feedback.observe_call(key, &[ObservedType::Object, ObservedType::Bool]);
            feedback.observe_return(key, return_pc, ObservedType::Bool);
        }
        let signature = feedback
            .snapshot(92)
            .bounded_specialization(key)
            .expect("heterogeneous bool signature");
        assert_eq!(signature.result(), FeedbackRepresentation::Bool);
        let isa = cranelift_native::builder()
            .expect("host ISA")
            .finish(cranelift_codegen::settings::Flags::new(
                cranelift_codegen::settings::builder(),
            ))
            .expect("host flags");
        let code = super::lower_target_only_linked_leaf(&isa, &verified, &signature, None, None)
            .expect("target-only linked bool leaf")
            .publish()
            .expect("publish linked bool leaf");
        let mut output = -1i32;
        let invoke = unsafe {
            core::mem::transmute::<*const u8, extern "C" fn(*mut i32, usize, i32) -> i32>(
                code.as_ptr(),
            )
        };
        assert_eq!(invoke(&mut output, 0x1234usize, 1), 0);
        assert_eq!(output, 1);
        assert_eq!(invoke(&mut output, 0x1234usize, 0), 0);
        assert_eq!(output, 0);
    }

    #[test]
    fn oversized_straight_line_leaf_is_rejected_before_codegen() {
        let mut source = String::from("(function(object, value){return value");
        for _ in 0..130 {
            source.push_str("+1");
        }
        source.push_str("})");
        let (_fixture, verified, signature) =
            fixture_and_signature(&source, &[ObservedType::Object, ObservedType::Int32], "add");
        assert!(verified.instructions().len() > 128);
        let isa = cranelift_native::builder()
            .expect("host ISA")
            .finish(cranelift_codegen::settings::Flags::new(
                cranelift_codegen::settings::builder(),
            ))
            .expect("host flags");
        assert_eq!(
            super::lower_target_only_linked_leaf(&isa, &verified, &signature, None, None)
                .expect_err("oversized target-only leaf"),
            crate::compiler::CompileFailure::ResourceLimit
        );
    }
    mod effectful {
        use crate::{
            bytecode::VerifyLimits,
            compiler::CompileFailure,
            runtime::{
                BinaryFeedbackFlags, FeedbackSnapshot, FeedbackTable, FunctionKey, ObservedType,
                PropertyAttributes, PrototypeDependencyToken, ShapeFeedbackTable, ShapeObservation,
                ShapeToken,
            },
            test_support::SnapshotFixture,
        };
        use rquickjs_core::qjs;

        const GENERATION: u64 = 7;

        /// Just enough of a QuickJS object for the guarded field protocol:
        /// the shape pointer, the shape generation and the property array.
        struct FakeObject {
            object: Box<[u64; 64]>,
            shape: Box<[u64; 64]>,
            properties: Box<[u64; 16]>,
        }

        impl FakeObject {
            fn new(fields: &[(i32, u64)]) -> Self {
                let layout = crate::abi::AbiInfo::linked()
                    .expect("linked ABI")
                    .property_layout();
                let mut fake = Self {
                    object: Box::new([0; 64]),
                    shape: Box::new([0; 64]),
                    properties: Box::new([0; 16]),
                };
                for (index, (tag, payload)) in fields.iter().enumerate() {
                    fake.properties[index * 2] = *payload;
                    fake.properties[index * 2 + 1] = i64::from(*tag) as u64;
                }
                let shape = fake.shape.as_mut_ptr() as u64;
                let properties = fake.properties.as_mut_ptr() as u64;
                fake.shape[layout.shape_generation_offset as usize / 8] = GENERATION;
                fake.object[layout.object_shape_offset as usize / 8] = shape;
                fake.object[layout.object_properties_offset as usize / 8] = properties;
                fake
            }
            fn address(&self) -> usize {
                self.object.as_ptr() as usize
            }
            fn shape_identity(&self) -> u64 {
                self.shape.as_ptr() as u64
            }
            fn field(&self, index: usize) -> (i32, u64) {
                (
                    self.properties[index * 2 + 1] as i64 as i32,
                    self.properties[index * 2],
                )
            }
            fn set_generation(&mut self, generation: u64) {
                let layout = crate::abi::AbiInfo::linked().unwrap().property_layout();
                self.shape[layout.shape_generation_offset as usize / 8] = generation;
            }
        }

        fn isa() -> cranelift_codegen::isa::OwnedTargetIsa {
            cranelift_native::builder()
                .expect("host ISA")
                .finish(cranelift_codegen::settings::Flags::new(
                    cranelift_codegen::settings::builder(),
                ))
                .expect("host flags")
        }

        /// Compiles `source` with Int32 arithmetic/return feedback and one
        /// monomorphic own-field observation per property site; `offsets`
        /// gives each site's field index in bytecode order.
        fn compile(
            source: &str,
            arguments: &[ObservedType],
            shape: u64,
            offsets: &[u32],
        ) -> Result<crate::compiler::baseline::PublishedBaselineCode, CompileFailure> {
            let fixture = SnapshotFixture::compile(source);
            let verified = fixture
                .snapshot()
                .verify(VerifyLimits::default())
                .expect("verified effectful leaf");
            let key = FunctionKey::new(
                verified.snapshot().function_id(),
                verified.snapshot().generation(),
            );
            let mut feedback = FeedbackTable::new(32, 2);
            let mut shapes = ShapeFeedbackTable::new(4);
            let mut property_sites = 0;
            for instruction in verified.instructions() {
                match instruction.opcode().name() {
                    "add" | "sub" | "inc" | "post_inc" => {
                        for _ in 0..32 {
                            feedback.observe_binary(
                                key,
                                instruction.pc(),
                                ObservedType::Int32,
                                ObservedType::Int32,
                                ObservedType::Int32,
                                BinaryFeedbackFlags::NONE,
                            );
                        }
                    }
                    "return" => feedback.observe_return(key, instruction.pc(), ObservedType::Int32),
                    "get_field" | "get_field2" | "put_field" => {
                        let offset = offsets[property_sites];
                        property_sites += 1;
                        shapes.observe(
                            key,
                            instruction.pc(),
                            ShapeObservation::new(
                                ShapeToken::new(shape, GENERATION),
                                PrototypeDependencyToken::new(0, 0),
                                offset,
                                PropertyAttributes::WRITABLE
                                    | PropertyAttributes::ENUMERABLE
                                    | PropertyAttributes::CONFIGURABLE,
                                ObservedType::Int32,
                            ),
                        );
                    }
                    _ => {}
                }
            }
            assert_eq!(property_sites, offsets.len(), "{source}");
            for _ in 0..32 {
                feedback.observe_call(key, arguments);
            }
            let snapshot: FeedbackSnapshot =
                feedback.snapshot(93).with_properties(shapes.snapshot(key));
            let signature = snapshot
                .bounded_specialization(key)
                .expect("bounded heterogeneous signature");
            super::super::lower_target_only_linked_leaf(
                &isa(),
                &verified,
                &signature,
                Some(&snapshot),
                None,
            )
            .map(|code| code.publish().expect("publish effectful leaf"))
        }

        const INCREMENT_AND_RECORD: &str = "(function(value, enabled, state){
             state.calls = state.calls + 1;
             if (enabled) return value + 1;
             return value;})";

        type Record = extern "C" fn(*mut i32, i32, i32, usize) -> i32;

        fn record_code(state: &FakeObject) -> crate::compiler::baseline::PublishedBaselineCode {
            compile(
                INCREMENT_AND_RECORD,
                &[
                    ObservedType::Int32,
                    ObservedType::Bool,
                    ObservedType::Object,
                ],
                state.shape_identity(),
                &[0, 0],
            )
            .expect("multi-block effectful leaf")
        }

        #[test]
        fn guarded_field_store_commits_with_the_result_on_every_branch() {
            let state = FakeObject::new(&[(qjs::JS_TAG_INT, 5)]);
            let code = record_code(&state);
            // SAFETY: the signature is (Int32, Bool, HeapRef) -> Int32 and the
            // fake object outlives every call.
            let record = unsafe { core::mem::transmute::<*const u8, Record>(code.as_ptr()) };
            let mut output = -1;
            assert_eq!(record(&mut output, 41, 1, state.address()), 0);
            assert_eq!(output, 42);
            assert_eq!(state.field(0), (qjs::JS_TAG_INT, 6));
            assert_eq!(record(&mut output, 41, 0, state.address()), 0);
            assert_eq!(output, 41);
            assert_eq!(state.field(0), (qjs::JS_TAG_INT, 7));
        }

        #[test]
        fn late_overflow_misses_before_the_buffered_store_is_committed() {
            let state = FakeObject::new(&[(qjs::JS_TAG_INT, 5)]);
            let code = record_code(&state);
            // SAFETY: see guarded_field_store_commits_with_the_result_on_every_branch.
            let record = unsafe { core::mem::transmute::<*const u8, Record>(code.as_ptr()) };
            let mut output = 77;
            // `value + 1` overflows after `state.calls` was (virtually) written.
            assert_eq!(record(&mut output, i32::MAX, 1, state.address()), 1);
            assert_eq!(output, 77, "a miss must not expose a result");
            assert_eq!(
                state.field(0),
                (qjs::JS_TAG_INT, 5),
                "the generic CALL must observe the original field"
            );
            // The counter itself overflowing is also a pre-commit miss.
            let full = FakeObject::new(&[(qjs::JS_TAG_INT, i32::MAX as u32 as u64)]);
            let code = record_code(&full);
            // SAFETY: as above.
            let record = unsafe { core::mem::transmute::<*const u8, Record>(code.as_ptr()) };
            assert_eq!(record(&mut output, 1, 1, full.address()), 1);
            assert_eq!(full.field(0), (qjs::JS_TAG_INT, i32::MAX as u32 as u64));
            assert_eq!(output, 77);
        }

        #[test]
        fn shape_generation_and_field_tag_guards_miss_without_writing() {
            let mut state = FakeObject::new(&[(qjs::JS_TAG_INT, 5)]);
            let code = record_code(&state);
            // SAFETY: as above.
            let record = unsafe { core::mem::transmute::<*const u8, Record>(code.as_ptr()) };
            let mut output = 77;
            state.set_generation(GENERATION + 1);
            assert_eq!(record(&mut output, 1, 1, state.address()), 1);
            assert_eq!(state.field(0), (qjs::JS_TAG_INT, 5));
            state.set_generation(GENERATION);
            let other = FakeObject::new(&[(qjs::JS_TAG_INT, 5)]);
            assert_eq!(record(&mut output, 1, 1, other.address()), 1);
            assert_eq!(other.field(0), (qjs::JS_TAG_INT, 5));
            let double = FakeObject::new(&[(qjs::JS_TAG_FLOAT64, 1.5f64.to_bits())]);
            let code = record_code(&double);
            // SAFETY: as above.
            let record = unsafe { core::mem::transmute::<*const u8, Record>(code.as_ptr()) };
            assert_eq!(record(&mut output, 1, 1, double.address()), 1);
            assert_eq!(double.field(0), (qjs::JS_TAG_FLOAT64, 1.5f64.to_bits()));
            assert_eq!(output, 77);
        }

        #[test]
        fn aliased_arguments_commit_stores_in_program_order() {
            let source = "(function(value, left, right){
                 left.x = value; right.x = value + 1; return value;})";
            let object = FakeObject::new(&[(qjs::JS_TAG_INT, 0)]);
            let code = compile(
                source,
                &[
                    ObservedType::Int32,
                    ObservedType::Object,
                    ObservedType::Object,
                ],
                object.shape_identity(),
                &[0, 0],
            )
            .unwrap();
            // SAFETY: (Int32, HeapRef, HeapRef) -> Int32 over a live fake.
            let run = unsafe {
                core::mem::transmute::<*const u8, extern "C" fn(*mut i32, i32, usize, usize) -> i32>(
                    code.as_ptr(),
                )
            };
            let mut output = 0;
            assert_eq!(run(&mut output, 9, object.address(), object.address()), 0);
            assert_eq!(output, 9);
            assert_eq!(object.field(0), (qjs::JS_TAG_INT, 10));
        }

        #[test]
        fn post_increment_statement_is_a_guarded_read_modify_write() {
            let source = "(function(value, state){state.calls++; return value + 1;})";
            let state = FakeObject::new(&[(qjs::JS_TAG_INT, 1), (qjs::JS_TAG_INT, 40)]);
            let code = compile(
                source,
                &[ObservedType::Int32, ObservedType::Object],
                state.shape_identity(),
                &[1, 1],
            )
            .unwrap();
            // SAFETY: (Int32, HeapRef) -> Int32 over a live fake.
            let run = unsafe {
                core::mem::transmute::<*const u8, extern "C" fn(*mut i32, i32, usize) -> i32>(
                    code.as_ptr(),
                )
            };
            let mut output = 0;
            assert_eq!(run(&mut output, 2, state.address()), 0);
            assert_eq!(output, 3);
            assert_eq!(state.field(0), (qjs::JS_TAG_INT, 1));
            assert_eq!(state.field(1), (qjs::JS_TAG_INT, 41));
        }

        #[test]
        fn a_field_read_after_a_store_is_rejected() {
            let source = "(function(value, state){state.a = value; return state.b + 1;})";
            let state = FakeObject::new(&[(qjs::JS_TAG_INT, 0)]);
            assert_eq!(
                compile(
                    source,
                    &[ObservedType::Int32, ObservedType::Object],
                    state.shape_identity(),
                    &[0, 1],
                )
                .err(),
                Some(CompileFailure::UnsupportedOpcode)
            );
        }
    }
}
