#[cfg(feature = "test-support")]
use std::cell::Cell;
use std::collections::{BTreeMap, VecDeque};

use rquickjs_core::qjs;

use crate::{
    bytecode::{Instruction, OperandFormat, VerifiedFunction},
    compiler::CompileFailure,
};

use super::{
    BinaryOp, FrameSlot, FrameState, FrameStateId, FrameStateKind, FrameStateTable, GenericOp,
    IrOp, IteratorOp, PollKind, StackOp, TaggedValue, UnaryOp, VarRefMode,
};

const POLL_INTERVAL: usize = 1_024;

#[cfg(feature = "test-support")]
thread_local! {
    static TRACE_COMPILATION: Cell<bool> = const { Cell::new(false) };
}

#[cfg(feature = "test-support")]
pub(crate) fn with_execution_trace<T>(f: impl FnOnce() -> T) -> T {
    TRACE_COMPILATION.with(|enabled| {
        let previous = enabled.replace(true);
        let result = f();
        enabled.set(previous);
        result
    })
}
pub(crate) const MAX_HELPER_SCRATCH_SLOTS: usize = 2;

const _: () = assert!(
    MAX_HELPER_SCRATCH_SLOTS == qjs::JS_JIT_HELPER_SCRATCH_SLOTS as usize,
    "the compiler scratch proof must match the native frame ABI"
);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrInstruction {
    pub pc: u32,
    pub frame_state: Option<FrameStateId>,
    pub helper_states: Box<[FrameStateId]>,
    pub op: IrOp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IrBlock {
    pub start_pc: u32,
    pub stack_depth: u16,
    pub instructions: Vec<IrInstruction>,
}

/// The handler an exception raised by an instruction resumes in this frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IrExceptionHandler {
    /// Operand-stack index of the innermost live catch offset.
    pub catch_index: u16,
    /// First PC of the handler block, entered with the caught value at
    /// `catch_index` and stack depth `catch_index + 1`.
    pub handler_pc: u32,
}

/// The CATCH call of one handler's landing pad.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IrCatchSite {
    pub catch_index: u16,
    pub state: FrameStateId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaselineIr {
    pub blocks: Vec<IrBlock>,
    pub frame_states: FrameStateTable,
    pub max_stack_depth: u16,
    pub argument_count: u16,
    pub local_count: u16,
    /// Innermost handler of every instruction PC inside a try region.
    pub exception_handlers: BTreeMap<u32, IrExceptionHandler>,
    /// One landing-pad CATCH call per handler PC.
    pub catch_sites: BTreeMap<u32, IrCatchSite>,
}

impl BaselineIr {
    pub fn translate(function: &VerifiedFunction) -> Result<Self, CompileFailure> {
        Self::translate_with_policy(function, true)
    }

    #[cfg(feature = "test-support")]
    pub fn translate_implemented_for_test(
        function: &VerifiedFunction,
    ) -> Result<Self, CompileFailure> {
        Self::translate_with_policy(function, false)
    }

    fn translate_with_policy(
        function: &VerifiedFunction,
        enforce_advertised_policy: bool,
    ) -> Result<Self, CompileFailure> {
        if enforce_advertised_policy {
            if let Err(rejection) = function.tier1_eligibility() {
                return Err(CompileFailure::Tier1Rejected(rejection.reason()));
            }
        }
        let block_depths = block_depths(function)?;
        let snapshot = function.snapshot();
        let mut states = FrameStateTable::default();
        let mut exception_handlers = BTreeMap::new();
        for instruction in function.instructions() {
            let Some(handler) = function.exception_handler(instruction.pc()) else {
                continue;
            };
            // An iterator-close offset is unwound by the interpreter itself;
            // native dispatch only resumes ordinary catch handlers. An
            // exception raised under a for-of close offset takes the ordinary
            // exception exit, which publishes the exact frame: the
            // interpreter closes the iterator with a throw completion and,
            // inside a try region, then resumes the enclosing handler itself.
            let Some(handler_pc) = handler.handler_pc() else {
                continue;
            };
            exception_handlers.insert(
                instruction.pc(),
                IrExceptionHandler {
                    catch_index: handler.catch_index(),
                    handler_pc,
                },
            );
        }
        let mut catch_sites = BTreeMap::new();
        for handler in exception_handlers.values() {
            if catch_sites.contains_key(&handler.handler_pc) {
                continue;
            }
            if block_depths.get(&handler.handler_pc).copied()
                != Some(usize::from(handler.catch_index) + 1)
            {
                return Err(CompileFailure::InvalidArtifact);
            }
            let state = record_state(
                &mut states,
                snapshot.arg_count(),
                snapshot.local_count(),
                usize::from(handler.catch_index) + 1,
                FrameStateKind::Helper,
                handler.handler_pc,
            )?;
            catch_sites.insert(
                handler.handler_pc,
                IrCatchSite {
                    catch_index: handler.catch_index,
                    state,
                },
            );
        }
        let mut blocks = Vec::with_capacity(function.control_flow_graph().blocks().len());
        let mut max_stack_depth = 0_usize;
        let mut emitted_since_poll = 0_usize;
        let loop_bodies = loop_body_blocks(function);

        for (block_index, block) in function.control_flow_graph().blocks().iter().enumerate() {
            // Dead code after a finally block's `ret` has no predecessor and
            // no proof; it is never lowered.
            if !function.is_reachable(block.start_pc()) {
                continue;
            }
            let mut depth = *block_depths
                .get(&block.start_pc())
                .ok_or(CompileFailure::InvalidArtifact)?;
            let mut instructions = Vec::new();
            let block_instructions = &function.instructions()[block.instruction_range()];
            let reaches_nearby_return = block_instructions.len() <= POLL_INTERVAL
                && block_instructions.last().is_some_and(|instruction| {
                    matches!(instruction.opcode().name(), "return" | "return_undef")
                });
            if block_index != 0
                && !reaches_nearby_return
                && (!loop_bodies.contains(&block.start_pc())
                    || function
                        .control_flow_graph()
                        .is_loop_header(block.start_pc()))
            {
                let state = record_state(
                    &mut states,
                    snapshot.arg_count(),
                    snapshot.local_count(),
                    depth,
                    FrameStateKind::Poll,
                    block.start_pc(),
                )?;
                instructions.push(IrInstruction {
                    pc: block.start_pc(),
                    frame_state: Some(state),
                    helper_states: Box::new([]),
                    op: IrOp::Poll {
                        state,
                        kind: if function
                            .control_flow_graph()
                            .is_loop_header(block.start_pc())
                        {
                            PollKind::LoopHeader
                        } else {
                            PollKind::Edge
                        },
                    },
                });
                emitted_since_poll = 0;
            }
            if function
                .control_flow_graph()
                .is_loop_header(block.start_pc())
            {
                let state = record_state(
                    &mut states,
                    snapshot.arg_count(),
                    snapshot.local_count(),
                    depth,
                    FrameStateKind::Marker,
                    block.start_pc(),
                )?;
                instructions.push(IrInstruction {
                    pc: block.start_pc(),
                    frame_state: Some(state),
                    helper_states: Box::new([]),
                    op: IrOp::OsrLabel { state },
                });
            }
            for instruction in &function.instructions()[block.instruction_range()] {
                let pc = instruction.pc();
                let needs_entry_poll = block_index == 0 && instruction.pc() == 0;
                let needs_periodic_poll = emitted_since_poll >= POLL_INTERVAL;
                let needs_return_poll =
                    matches!(instruction.opcode().name(), "return" | "return_undef");
                if needs_entry_poll {
                    let state = record_state(
                        &mut states,
                        snapshot.arg_count(),
                        snapshot.local_count(),
                        depth,
                        FrameStateKind::Poll,
                        pc,
                    )?;
                    instructions.push(IrInstruction {
                        pc,
                        frame_state: Some(state),
                        helper_states: Box::new([]),
                        op: IrOp::Poll {
                            state,
                            kind: PollKind::Entry,
                        },
                    });
                    emitted_since_poll = 0;
                }
                if needs_periodic_poll {
                    let state = record_state(
                        &mut states,
                        snapshot.arg_count(),
                        snapshot.local_count(),
                        depth,
                        FrameStateKind::Poll,
                        pc,
                    )?;
                    instructions.push(IrInstruction {
                        pc,
                        frame_state: Some(state),
                        helper_states: Box::new([]),
                        op: IrOp::Poll {
                            state,
                            kind: PollKind::Periodic,
                        },
                    });
                    emitted_since_poll = 0;
                }
                if needs_return_poll {
                    let state = record_state(
                        &mut states,
                        snapshot.arg_count(),
                        snapshot.local_count(),
                        depth,
                        FrameStateKind::Poll,
                        pc,
                    )?;
                    instructions.push(IrInstruction {
                        pc,
                        frame_state: Some(state),
                        helper_states: Box::new([]),
                        op: IrOp::Poll {
                            state,
                            kind: PollKind::Return,
                        },
                    });
                    emitted_since_poll = 0;
                }

                #[cfg(feature = "test-support")]
                if TRACE_COMPILATION.with(Cell::get) {
                    let state = record_state(
                        &mut states,
                        snapshot.arg_count(),
                        snapshot.local_count(),
                        depth,
                        FrameStateKind::Poll,
                        pc,
                    )?;
                    instructions.push(IrInstruction {
                        pc,
                        frame_state: Some(state),
                        helper_states: Box::new([]),
                        op: IrOp::Poll {
                            state,
                            kind: PollKind::Periodic,
                        },
                    });
                }

                let op = match instruction.opcode().name() {
                    // The verifier resolved each `ret` to the exact return
                    // points of its finally block.
                    "ret" => IrOp::Ret {
                        targets: block.successors().to_vec().into_boxed_slice(),
                    },
                    // Values between the kept operand and its catch offset
                    // would need ordered releases; only the direct form is
                    // lowered.
                    // The innermost catch offset may also be a for-of
                    // iterator close offset (`return` inside for-of).
                    "nip_catch"
                        if function
                            .exception_handler(pc)
                            .map(|handler| usize::from(handler.catch_index()) + 2)
                            == Some(depth) =>
                    {
                        IrOp::NipCatch
                    }
                    "nip_catch" => return Err(CompileFailure::UnsupportedOpcode),
                    _ => translate_instruction(instruction)?,
                };
                let depth_before = depth;
                let helper_call_count = operation_helper_call_count(&op);
                let frame_state = if operation_may_exit(&op) && helper_call_count == 0 {
                    Some(record_state(
                        &mut states,
                        snapshot.arg_count(),
                        snapshot.local_count(),
                        depth,
                        FrameStateKind::Marker,
                        pc,
                    )?)
                } else {
                    None
                };
                let pop = effective_pop(instruction);
                if depth < pop {
                    return Err(CompileFailure::InvalidArtifact);
                }
                let next_depth = depth - pop + effective_push(instruction);
                max_stack_depth = max_stack_depth.max(next_depth).max(depth);
                let helper_depth = helper_stack_depth(&op, depth, next_depth)?;
                max_stack_depth = max_stack_depth.max(helper_depth);
                let next_pc = pc
                    .checked_add(
                        u32::try_from(instruction.size())
                            .map_err(|_| CompileFailure::ResourceLimit)?,
                    )
                    .ok_or(CompileFailure::ResourceLimit)?;
                let helper_states = (0..helper_call_count)
                    .map(|_| {
                        record_state(
                            &mut states,
                            snapshot.arg_count(),
                            snapshot.local_count(),
                            helper_depth,
                            FrameStateKind::Helper,
                            next_pc,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_boxed_slice();
                instructions.push(IrInstruction {
                    pc,
                    frame_state,
                    helper_states,
                    op,
                });
                emitted_since_poll = emitted_since_poll.saturating_add(1);
                if is_tail_call(instruction) {
                    // The interpreter returns the call result directly
                    // (`goto done`). The call lowering leaves that result as
                    // the single value above the popped callee/arguments, so
                    // the return sequence observes depth `next_depth + 1`
                    // even though the opcode's own `n_push` is zero.
                    let return_depth = next_depth
                        .checked_add(1)
                        .ok_or(CompileFailure::ResourceLimit)?;
                    max_stack_depth = max_stack_depth.max(return_depth);
                    let poll_state = record_state(
                        &mut states,
                        snapshot.arg_count(),
                        snapshot.local_count(),
                        return_depth,
                        FrameStateKind::Poll,
                        pc,
                    )?;
                    instructions.push(IrInstruction {
                        pc,
                        frame_state: Some(poll_state),
                        helper_states: Box::new([]),
                        op: IrOp::Poll {
                            state: poll_state,
                            kind: PollKind::Return,
                        },
                    });
                    let return_state = record_state(
                        &mut states,
                        snapshot.arg_count(),
                        snapshot.local_count(),
                        return_depth,
                        FrameStateKind::Marker,
                        pc,
                    )?;
                    instructions.push(IrInstruction {
                        pc,
                        frame_state: Some(return_state),
                        helper_states: Box::new([]),
                        op: IrOp::Return,
                    });
                    emitted_since_poll = 0;
                }
                depth = next_depth;

                if let Some(target) = instruction.branch_target() {
                    if target <= i64::from(pc)
                        && !u32::try_from(target).is_ok_and(|target| {
                            function.control_flow_graph().is_loop_header(target)
                        })
                    {
                        let state = record_state(
                            &mut states,
                            snapshot.arg_count(),
                            snapshot.local_count(),
                            depth_before,
                            FrameStateKind::Poll,
                            pc,
                        )?;
                        let insert_at = instructions.len() - 1;
                        instructions.insert(
                            insert_at,
                            IrInstruction {
                                pc,
                                frame_state: Some(state),
                                helper_states: Box::new([]),
                                op: IrOp::Poll {
                                    state,
                                    kind: PollKind::Edge,
                                },
                            },
                        );
                        emitted_since_poll = 0;
                    }
                }
            }
            blocks.push(IrBlock {
                start_pc: block.start_pc(),
                stack_depth: *block_depths
                    .get(&block.start_pc())
                    .ok_or(CompileFailure::InvalidArtifact)? as u16,
                instructions,
            });
        }

        // Tier 1 proves `put_loc_check_init` statically from the entry
        // analysis. Once this frame creates closures, a callee can initialize
        // a captured lexical slot (for example a derived constructor's `this`
        // through `put_var_ref_check_init`), which that proof cannot observe.
        let ops = || {
            blocks
                .iter()
                .flat_map(|block: &IrBlock| &block.instructions)
        };
        if ops().any(|instruction| matches!(instruction.op, IrOp::FClosure(_)))
            && ops().any(|instruction| {
                matches!(
                    instruction.op,
                    IrOp::PutLocalChecked {
                        initialize: true,
                        ..
                    }
                )
            })
        {
            return Err(CompileFailure::Tier1Rejected(
                crate::bytecode::FallbackReason::ClosureFrame,
            ));
        }

        Ok(Self {
            blocks,
            frame_states: states,
            max_stack_depth: u16::try_from(max_stack_depth)
                .map_err(|_| CompileFailure::ResourceLimit)?,
            argument_count: snapshot.arg_count(),
            local_count: snapshot.local_count(),
            exception_handlers,
            catch_sites,
        })
    }
}

fn loop_body_blocks(function: &VerifiedFunction) -> std::collections::BTreeSet<u32> {
    let cfg = function.control_flow_graph();
    let mut predecessors: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for block in cfg.blocks() {
        for successor in block.successors() {
            predecessors
                .entry(*successor)
                .or_default()
                .push(block.start_pc());
        }
    }

    let mut bodies = std::collections::BTreeSet::new();
    for latch in cfg.blocks() {
        for header in
            latch.successors().iter().copied().filter(|successor| {
                *successor <= latch.start_pc() && cfg.is_loop_header(*successor)
            })
        {
            let mut pending = vec![latch.start_pc()];
            let mut natural_loop = std::collections::BTreeSet::from([header]);
            while let Some(pc) = pending.pop() {
                if natural_loop.insert(pc) && pc != header {
                    if let Some(block_predecessors) = predecessors.get(&pc) {
                        pending.extend_from_slice(block_predecessors);
                    }
                }
            }
            natural_loop.remove(&header);
            bodies.extend(natural_loop);
        }
    }
    bodies
}

fn operation_may_exit(operation: &IrOp) -> bool {
    matches!(
        operation,
        IrOp::Unary(_)
            | IrOp::PostUnary(_)
            | IrOp::LocalUnary { .. }
            | IrOp::AddLocal(_)
            | IrOp::Binary(_)
            | IrOp::GetLocalChecked(_)
            | IrOp::PutLocalChecked { .. }
            | IrOp::Branch { .. }
            | IrOp::Return
            | IrOp::ReturnUndefined
            | IrOp::Ret { .. }
    )
}

fn operation_helper_call_count(operation: &IrOp) -> usize {
    match operation {
        IrOp::ResolveConstant(_) | IrOp::ResolveAtom(_) | IrOp::GetGlobal(_) | IrOp::NewObject => 1,
        IrOp::FClosure(_) | IrOp::GetVarRef { .. } | IrOp::CloseLocal(_) | IrOp::SetName(_) => 1,
        // `set_var_ref*` first duplicates the kept value into the operand
        // slot above the stack, then the store consumes that duplicate.
        IrOp::PutVarRef { keep, .. } => 1 + usize::from(*keep),
        IrOp::NewArrayFrom(count) => 1 + usize::from(*count),
        IrOp::GetProperty(_) | IrOp::SetProperty(_) => 2,
        IrOp::DefineProperty(_) => 1,
        IrOp::GetPropertyKeep(_) => 1,
        // Element lowering reserves one state for the generic helper and one
        // for each direct ownership-release edge (packed, Int32, Float64).
        // All of these branches can survive codegen, so no two return PCs may
        // share a stack-map record.
        IrOp::GetElement | IrOp::SetElement => 4,
        IrOp::DefineElement => 3,
        IrOp::ToPropertyKey => 1,
        IrOp::Call { argc, has_this } => 1 + usize::from(*argc) + 1 + usize::from(*has_this),
        IrOp::CallConstructor(argc) => 1 + usize::from(*argc) + 2,
        IrOp::Regexp => 1,
        IrOp::Generic(_) => 1,
        IrOp::Iterator(_) => 1,
        IrOp::GetArgument(_) | IrOp::GetLocal(_) | IrOp::GetLocalChecked(_) => 1,
        IrOp::GetLocalPair => 2,
        IrOp::PutArgument { keep, .. } | IrOp::PutLocal { keep, .. } => 1 + usize::from(*keep),
        IrOp::PutLocalChecked { .. } | IrOp::SetLocalUninitialized(_) | IrOp::Drop => 1,
        IrOp::Stack(operation) => match operation {
            StackOp::Nip | StackOp::Nip1 => 1,
            StackOp::Dup2 => 2,
            StackOp::Dup3 => 3,
            StackOp::Dup
            | StackOp::Dup1
            | StackOp::Insert2
            | StackOp::Insert3
            | StackOp::Insert4 => 1,
            _ => 0,
        },
        IrOp::Unary(
            UnaryOp::Plus
            | UnaryOp::LogicalNot
            | UnaryOp::IsUndefinedOrNull
            | UnaryOp::IsUndefined
            | UnaryOp::IsNull
            | UnaryOp::Neg
            | UnaryOp::Increment
            | UnaryOp::Decrement
            | UnaryOp::BitNot,
        ) => 1,
        IrOp::Binary(
            BinaryOp::Add
            | BinaryOp::Mod
            | BinaryOp::LessThan
            | BinaryOp::LessThanOrEqual
            | BinaryOp::GreaterThan
            | BinaryOp::GreaterThanOrEqual
            | BinaryOp::Equal
            | BinaryOp::NotEqual
            | BinaryOp::StrictEqual
            | BinaryOp::StrictNotEqual,
        ) => 1,
        IrOp::Branch { .. } => 1,
        IrOp::Throw | IrOp::ThrowError { .. } => 1,
        _ => 0,
    }
}

fn helper_stack_depth(
    operation: &IrOp,
    depth: usize,
    next_depth: usize,
) -> Result<usize, CompileFailure> {
    /*
     * These are the only lowering families that keep values above the
     * bytecode-visible stack.  The exhaustive match is the compiler's static
     * proof that no instruction can require more than the ABI's two scratch
     * slots at one time.
     */
    let extra = match operation {
        IrOp::GetProperty(_) | IrOp::Call { .. } | IrOp::CallConstructor(_) => 2,
        IrOp::GetPropertyKeep(_) => 1,
        IrOp::PutVarRef { keep: true, .. } => 1,
        IrOp::NewArrayFrom(count) if *count != 0 => 2,
        IrOp::DefineElement => 2,
        IrOp::NewArrayFrom(_) => 0,
        _ => 0,
    };
    debug_assert!(extra <= MAX_HELPER_SCRATCH_SLOTS);
    depth
        .max(next_depth)
        .checked_add(extra)
        .ok_or(CompileFailure::ResourceLimit)
}

fn record_state(
    table: &mut FrameStateTable,
    arguments: u16,
    locals: u16,
    stack_depth: usize,
    kind: FrameStateKind,
    pc: u32,
) -> Result<FrameStateId, CompileFailure> {
    let slot_count = usize::from(arguments)
        .checked_add(usize::from(locals))
        .and_then(|count| count.checked_add(stack_depth))
        .filter(|count| *count <= usize::from(u16::MAX))
        .ok_or(CompileFailure::ResourceLimit)?;
    let stack_depth = u16::try_from(stack_depth).map_err(|_| CompileFailure::ResourceLimit)?;
    let mut slots = Vec::with_capacity(slot_count);
    slots.extend((0..arguments).map(FrameSlot::Argument));
    slots.extend((0..locals).map(FrameSlot::Local));
    slots.extend((0..stack_depth).map(FrameSlot::Stack));
    Ok(table.push(FrameState {
        pc,
        slots: slots.into_boxed_slice(),
        kind,
    }))
}

fn is_tail_call(instruction: &Instruction) -> bool {
    matches!(
        instruction.opcode().name(),
        "tail_call" | "tail_call_method"
    )
}

fn effective_pop(instruction: &Instruction) -> usize {
    let base = instruction.opcode().n_pop() as usize;
    match instruction.opcode().format() {
        OperandFormat::NPop | OperandFormat::NPopU16 => {
            base.saturating_add(instruction.operand_u16(1) as usize)
        }
        OperandFormat::NPopFixed => instruction
            .opcode()
            .name()
            .as_bytes()
            .last()
            .and_then(|value| value.is_ascii_digit().then_some((value - b'0') as usize))
            .map_or(base, |count| base.saturating_add(count)),
        _ => base,
    }
}

/// `gosub` is declared without stack effects in the QuickJS opcode table but
/// pushes the Int32 return offset that its finally block's `ret` consumes.
fn effective_push(instruction: &Instruction) -> usize {
    instruction.opcode().n_push() as usize + usize::from(instruction.opcode().name() == "gosub")
}

fn block_depths(function: &VerifiedFunction) -> Result<BTreeMap<u32, usize>, CompileFailure> {
    let cfg = function.control_flow_graph();
    let mut depths = BTreeMap::from([(0_u32, 0_usize)]);
    let mut queue = VecDeque::from([0_u32]);
    while let Some(pc) = queue.pop_front() {
        let block = cfg.block(pc).ok_or(CompileFailure::InvalidArtifact)?;
        let mut depth = depths[&pc];
        for instruction in &function.instructions()[block.instruction_range()] {
            let pop = effective_pop(instruction);
            if depth < pop {
                return Err(CompileFailure::InvalidArtifact);
            }
            depth = depth - pop + effective_push(instruction);
        }
        for &successor in block.successors() {
            match depths.get(&successor) {
                Some(existing) if *existing != depth => {
                    return Err(CompileFailure::InvalidArtifact)
                }
                Some(_) => {}
                None => {
                    depths.insert(successor, depth);
                    queue.push_back(successor);
                }
            }
        }
    }
    Ok(depths)
}

fn short_index(instruction: &Instruction) -> Option<u16> {
    instruction
        .opcode()
        .name()
        .as_bytes()
        .last()
        .and_then(|value| value.is_ascii_digit().then_some(u16::from(value - b'0')))
}

fn indexed_operand(instruction: &Instruction) -> Option<u16> {
    match instruction.opcode().format() {
        OperandFormat::Local | OperandFormat::Argument => Some(instruction.operand_u16(1)),
        OperandFormat::Local8 => Some(u16::from(instruction.operand_u8(1))),
        OperandFormat::NoneLocal | OperandFormat::NoneArgument => short_index(instruction),
        _ => None,
    }
}

fn constant_operand(instruction: &Instruction) -> Option<u32> {
    match instruction.opcode().format() {
        OperandFormat::Constant => Some(instruction.operand_u32(1)),
        OperandFormat::Constant8 => Some(u32::from(instruction.operand_u8(1))),
        _ => None,
    }
}

fn closure_operand(instruction: &Instruction) -> Option<u16> {
    match instruction.opcode().format() {
        OperandFormat::Closure => Some(instruction.operand_u16(1)),
        OperandFormat::NoneClosure => short_index(instruction),
        _ => None,
    }
}

fn atom_operand(instruction: &Instruction) -> Option<u32> {
    matches!(instruction.opcode().format(), OperandFormat::Atom).then(|| instruction.operand_u32(1))
}

fn translate_instruction(instruction: &Instruction) -> Result<IrOp, CompileFailure> {
    let name = instruction.opcode().name();
    let int = |value: i32| TaggedValue::new(value as i64 as u64, qjs::JS_TAG_INT as i64);
    let operation = match name {
        "nop" => IrOp::Nop,
        "undefined" | "push_undefined" => {
            IrOp::Push(TaggedValue::new(0, qjs::JS_TAG_UNDEFINED as i64))
        }
        "null" | "push_null" => IrOp::Push(TaggedValue::new(0, qjs::JS_TAG_NULL as i64)),
        "push_false" => IrOp::Push(TaggedValue::new(0, qjs::JS_TAG_BOOL as i64)),
        "push_true" => IrOp::Push(TaggedValue::new(1, qjs::JS_TAG_BOOL as i64)),
        "push_minus1" => IrOp::Push(int(-1)),
        "push_0" | "push_1" | "push_2" | "push_3" | "push_4" | "push_5" | "push_6" | "push_7" => {
            IrOp::Push(int(
                short_index(instruction).expect("numeric compact opcode") as i32,
            ))
        }
        "push_i8" => IrOp::Push(int(instruction.operand_u8(1) as i8 as i32)),
        "push_i16" => IrOp::Push(int(instruction.operand_i16(1) as i32)),
        "push_i32" => IrOp::Push(int(instruction.operand_i32(1))),
        "push_const" | "push_const8" => IrOp::ResolveConstant(
            constant_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
        ),
        "push_atom_value" => {
            IrOp::ResolveAtom(atom_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?)
        }
        // QuickJS `js_empty_string(rt)` is
        // `js_dup(JS_MKPTR(JS_TAG_STRING, rt->atom_array[JS_ATOM_empty_string]))`,
        // which is exactly `JS_AtomToValue(JS_ATOM_empty_string)`.
        "push_empty_string" => IrOp::ResolveAtom(qjs::JS_ATOM_empty_string),
        "get_var" => {
            IrOp::GetGlobal(atom_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?)
        }
        "object" => IrOp::NewObject,
        "array_from" => IrOp::NewArrayFrom(instruction.operand_u16(1)),
        "get_field" => IrOp::GetProperty(instruction.operand_u32(1)),
        "get_length" => IrOp::GetProperty(qjs::JS_ATOM_length),
        "get_field2" => IrOp::GetPropertyKeep(instruction.operand_u32(1)),
        "put_field" => IrOp::SetProperty(instruction.operand_u32(1)),
        "define_field" => IrOp::DefineProperty(instruction.operand_u32(1)),
        "get_array_el" => IrOp::GetElement,
        "put_array_el" => IrOp::SetElement,
        "define_array_el" => IrOp::DefineElement,
        "to_propkey" => IrOp::ToPropertyKey,
        // `tail_call` / `tail_call_method` are `call` / `call_method` whose
        // result is returned immediately (`goto done` in the interpreter).
        // `translate_with_policy` appends the return sequence after the call.
        "call" | "tail_call" => IrOp::Call {
            argc: instruction.operand_u16(1),
            has_this: false,
        },
        "call0" | "call1" | "call2" | "call3" => IrOp::Call {
            argc: u16::from(
                *name
                    .as_bytes()
                    .last()
                    .ok_or(CompileFailure::InvalidArtifact)?
                    - b'0',
            ),
            has_this: false,
        },
        "call_method" | "tail_call_method" => IrOp::Call {
            argc: instruction.operand_u16(1),
            has_this: true,
        },
        "call_constructor" => IrOp::CallConstructor(instruction.operand_u16(1)),
        "regexp" => IrOp::Regexp,
        "fclosure" | "fclosure8" => {
            IrOp::FClosure(constant_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?)
        }
        "get_var_ref" | "get_var_ref0" | "get_var_ref1" | "get_var_ref2" | "get_var_ref3"
        | "get_var_ref_check" => IrOp::GetVarRef {
            index: closure_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
            checked: name == "get_var_ref_check",
        },
        "put_var_ref"
        | "put_var_ref0"
        | "put_var_ref1"
        | "put_var_ref2"
        | "put_var_ref3"
        | "set_var_ref"
        | "set_var_ref0"
        | "set_var_ref1"
        | "set_var_ref2"
        | "set_var_ref3"
        | "put_var_ref_check"
        | "put_var_ref_check_init" => IrOp::PutVarRef {
            index: closure_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
            mode: match name {
                "put_var_ref_check" => VarRefMode::Check,
                "put_var_ref_check_init" => VarRefMode::CheckInit,
                _ => VarRefMode::Plain,
            },
            keep: name.starts_with("set_var_ref"),
        },
        "set_name" => {
            IrOp::SetName(atom_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?)
        }
        "close_loc" => {
            IrOp::CloseLocal(indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?)
        }
        "push_this" => IrOp::Generic(GenericOp::PushThis),
        "special_object" => match instruction.operand_u8(1) {
            crate::bytecode::SPECIAL_OBJECT_MAPPED_ARGUMENTS
            | crate::bytecode::SPECIAL_OBJECT_IMPORT_META => {
                return Err(CompileFailure::UnsupportedOpcode)
            }
            kind if kind > crate::bytecode::SPECIAL_OBJECT_MAX => {
                return Err(CompileFailure::UnsupportedOpcode)
            }
            kind => IrOp::Generic(GenericOp::SpecialObject(kind)),
        },
        "get_var_undef" => IrOp::Generic(GenericOp::GetVarUndef(
            atom_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
        )),
        "delete_var" => IrOp::Generic(GenericOp::DeleteVar(
            atom_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
        )),
        "put_var" => IrOp::Generic(GenericOp::PutVar(
            atom_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
        )),
        "typeof" => IrOp::Generic(GenericOp::TypeOf),
        "typeof_is_undefined" => IrOp::Generic(GenericOp::TypeOfIsUndefined),
        "typeof_is_function" => IrOp::Generic(GenericOp::TypeOfIsFunction),
        "to_object" => IrOp::Generic(GenericOp::ToObject),
        "to_propkey2" => IrOp::Generic(GenericOp::ToPropertyKey2),
        "in" => IrOp::Generic(GenericOp::In),
        "instanceof" => IrOp::Generic(GenericOp::InstanceOf),
        "delete" => IrOp::Generic(GenericOp::Delete),
        "pow" => IrOp::Generic(GenericOp::Pow),
        "for_of_start" => IrOp::Iterator(IteratorOp::ForOfStart),
        "for_of_next" => IrOp::Iterator(IteratorOp::ForOfNext(instruction.operand_u8(1))),
        "for_in_start" => IrOp::Iterator(IteratorOp::ForInStart),
        "for_in_next" => IrOp::Iterator(IteratorOp::ForInNext),
        "iterator_close" => IrOp::Iterator(IteratorOp::Close),
        "get_arg" | "get_arg0" | "get_arg1" | "get_arg2" | "get_arg3" => {
            IrOp::GetArgument(indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?)
        }
        "get_loc" | "get_loc8" | "get_loc0" | "get_loc1" | "get_loc2" | "get_loc3" => {
            IrOp::GetLocal(indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?)
        }
        "get_loc_check" => IrOp::GetLocalChecked(
            indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
        ),
        "get_loc0_loc1" => IrOp::GetLocalPair,
        "put_arg" | "put_arg0" | "put_arg1" | "put_arg2" | "put_arg3" => IrOp::PutArgument {
            index: indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
            keep: false,
        },
        "set_arg" | "set_arg0" | "set_arg1" | "set_arg2" | "set_arg3" => IrOp::PutArgument {
            index: indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
            keep: true,
        },
        "put_loc" | "put_loc8" | "put_loc0" | "put_loc1" | "put_loc2" | "put_loc3" => {
            IrOp::PutLocal {
                index: indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
                keep: false,
            }
        }
        "set_loc" | "set_loc8" | "set_loc0" | "set_loc1" | "set_loc2" | "set_loc3" => {
            IrOp::PutLocal {
                index: indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
                keep: true,
            }
        }
        "put_loc_check" | "put_loc_check_init" => IrOp::PutLocalChecked {
            index: indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
            initialize: name == "put_loc_check_init",
        },
        "set_loc_uninitialized" => IrOp::SetLocalUninitialized(
            indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
        ),
        "drop" => IrOp::Drop,
        "nip" => IrOp::Stack(StackOp::Nip),
        "nip1" => IrOp::Stack(StackOp::Nip1),
        "dup" => IrOp::Stack(StackOp::Dup),
        "dup1" => IrOp::Stack(StackOp::Dup1),
        "dup2" => IrOp::Stack(StackOp::Dup2),
        "dup3" => IrOp::Stack(StackOp::Dup3),
        "insert2" => IrOp::Stack(StackOp::Insert2),
        "insert3" => IrOp::Stack(StackOp::Insert3),
        "insert4" => IrOp::Stack(StackOp::Insert4),
        "perm3" => IrOp::Stack(StackOp::Perm3),
        "perm4" => IrOp::Stack(StackOp::Perm4),
        "perm5" => IrOp::Stack(StackOp::Perm5),
        "swap" => IrOp::Stack(StackOp::Swap),
        "swap2" => IrOp::Stack(StackOp::Swap2),
        "rot3l" => IrOp::Stack(StackOp::Rot3Left),
        "rot3r" => IrOp::Stack(StackOp::Rot3Right),
        "rot4l" => IrOp::Stack(StackOp::Rot4Left),
        "rot5l" => IrOp::Stack(StackOp::Rot5Left),
        "plus" => IrOp::Unary(UnaryOp::Plus),
        "neg" => IrOp::Unary(UnaryOp::Neg),
        "inc" => IrOp::Unary(UnaryOp::Increment),
        "dec" => IrOp::Unary(UnaryOp::Decrement),
        "post_inc" => IrOp::PostUnary(UnaryOp::Increment),
        "post_dec" => IrOp::PostUnary(UnaryOp::Decrement),
        "inc_loc" => IrOp::LocalUnary {
            index: indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
            op: UnaryOp::Increment,
        },
        "dec_loc" => IrOp::LocalUnary {
            index: indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?,
            op: UnaryOp::Decrement,
        },
        "add_loc" => {
            IrOp::AddLocal(indexed_operand(instruction).ok_or(CompileFailure::InvalidArtifact)?)
        }
        "lnot" => IrOp::Unary(UnaryOp::LogicalNot),
        "is_undefined_or_null" => IrOp::Unary(UnaryOp::IsUndefinedOrNull),
        "is_undefined" => IrOp::Unary(UnaryOp::IsUndefined),
        "is_null" => IrOp::Unary(UnaryOp::IsNull),
        "not" => IrOp::Unary(UnaryOp::BitNot),
        "add" => IrOp::Binary(BinaryOp::Add),
        "sub" => IrOp::Binary(BinaryOp::Sub),
        "mul" => IrOp::Binary(BinaryOp::Mul),
        "div" => IrOp::Binary(BinaryOp::Div),
        "mod" => IrOp::Binary(BinaryOp::Mod),
        "and" => IrOp::Binary(BinaryOp::BitAnd),
        "or" => IrOp::Binary(BinaryOp::BitOr),
        "xor" => IrOp::Binary(BinaryOp::BitXor),
        "shl" => IrOp::Binary(BinaryOp::ShiftLeft),
        "sar" => IrOp::Binary(BinaryOp::ShiftRight),
        "shr" => IrOp::Binary(BinaryOp::ShiftRightUnsigned),
        "lt" => IrOp::Binary(BinaryOp::LessThan),
        "lte" => IrOp::Binary(BinaryOp::LessThanOrEqual),
        "gt" => IrOp::Binary(BinaryOp::GreaterThan),
        "gte" => IrOp::Binary(BinaryOp::GreaterThanOrEqual),
        "eq" => IrOp::Binary(BinaryOp::Equal),
        "neq" => IrOp::Binary(BinaryOp::NotEqual),
        "strict_eq" => IrOp::Binary(BinaryOp::StrictEqual),
        "strict_neq" => IrOp::Binary(BinaryOp::StrictNotEqual),
        "goto" | "goto8" | "goto16" => IrOp::Jump(
            instruction
                .branch_target()
                .ok_or(CompileFailure::InvalidArtifact)? as u32,
        ),
        "if_true" | "if_true8" => IrOp::Branch {
            target: instruction
                .branch_target()
                .ok_or(CompileFailure::InvalidArtifact)? as u32,
            when_true: true,
        },
        "if_false" | "if_false8" => IrOp::Branch {
            target: instruction
                .branch_target()
                .ok_or(CompileFailure::InvalidArtifact)? as u32,
            when_true: false,
        },
        "return" => IrOp::Return,
        "return_undef" => IrOp::ReturnUndefined,
        "catch" => IrOp::Catch(
            u32::try_from(
                instruction
                    .branch_target()
                    .ok_or(CompileFailure::InvalidArtifact)?,
            )
            .map_err(|_| CompileFailure::InvalidArtifact)?,
        ),
        "throw" => IrOp::Throw,
        "throw_error" => IrOp::ThrowError {
            atom: instruction.operand_u32(1),
            kind: instruction.operand_u8(5),
        },
        "gosub" => IrOp::Gosub {
            target: u32::try_from(
                instruction
                    .branch_target()
                    .ok_or(CompileFailure::InvalidArtifact)?,
            )
            .map_err(|_| CompileFailure::InvalidArtifact)?,
            return_pc: instruction
                .pc()
                .checked_add(
                    u32::try_from(instruction.size()).map_err(|_| CompileFailure::ResourceLimit)?,
                )
                .ok_or(CompileFailure::ResourceLimit)?,
        },
        _ => return Err(CompileFailure::UnsupportedOpcode),
    };
    Ok(operation)
}
