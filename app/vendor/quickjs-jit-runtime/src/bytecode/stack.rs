use std::collections::{BTreeMap, BTreeSet, VecDeque};

use rquickjs_core::qjs;

use super::{
    cfg::ControlFlowGraph, CompileSnapshot, Instruction, OperandFormat, Resource, VerifyError,
    VerifyErrorKind,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotKind {
    Tagged,
    Int32,
    Float64,
    CatchOffset,
    Uninitialized,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AbstractState {
    pub(crate) locals: Vec<SlotKind>,
    pub(crate) stack: Vec<SlotKind>,
    /// Live catch offsets, innermost last: the operand-stack index of each
    /// `CatchOffset` slot and its handler (`None` for iterator close offsets,
    /// which the interpreter unwinds through without resuming).
    pub(crate) catches: Vec<(usize, Option<u32>)>,
}

/// Where an exception raised while executing an instruction resumes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExceptionHandler {
    catch_index: u16,
    handler_pc: Option<u32>,
    resumes_in_frame: bool,
}

impl ExceptionHandler {
    /// Operand-stack index of the innermost live catch offset.
    pub const fn catch_index(self) -> u16 {
        self.catch_index
    }

    /// Handler PC of that catch offset; `None` for an iterator close offset.
    pub const fn handler_pc(self) -> Option<u32> {
        self.handler_pc
    }

    /// Whether some live catch offset (this one or an enclosing one) resumes
    /// a handler in this frame. `false` when every live catch offset is a
    /// for-of iterator close offset: the interpreter then closes the
    /// iterators and propagates the exception out of the frame, exactly what
    /// an ordinary native exception exit leads to.
    pub const fn resumes_in_frame(self) -> bool {
        self.resumes_in_frame
    }
}

impl AbstractState {
    /// The innermost live catch offset, which the interpreter's exception
    /// unwinding reaches first.
    fn innermost_catch(&self) -> Option<ExceptionHandler> {
        self.catches.last().and_then(|(index, handler_pc)| {
            Some(ExceptionHandler {
                catch_index: u16::try_from(*index).ok()?,
                handler_pc: *handler_pc,
                resumes_in_frame: self.innermost_handler().is_some(),
            })
        })
    }

    /// The innermost catch offset that resumes a handler in this frame.
    fn innermost_handler(&self) -> Option<(usize, u32)> {
        self.catches
            .iter()
            .rev()
            .find_map(|(index, handler_pc)| handler_pc.map(|pc| (*index, pc)))
    }
}

impl AbstractState {
    fn cell_count(&self) -> usize {
        self.locals.len().saturating_add(self.stack.len())
    }

    pub(crate) fn live_slots(&self, snapshot: &CompileSnapshot) -> Vec<SlotKind> {
        let mut result = Vec::with_capacity(
            snapshot.arg_count() as usize
                + self.locals.len()
                + snapshot.closure_count() as usize
                + self.stack.len(),
        );
        result.resize(snapshot.arg_count() as usize, SlotKind::Tagged);
        result.extend_from_slice(&self.locals);
        result.resize(
            result.len() + snapshot.closure_count() as usize,
            SlotKind::Tagged,
        );
        result.extend_from_slice(&self.stack);
        result
    }
}

struct WorkBudget {
    used: usize,
    limit: usize,
}

impl WorkBudget {
    fn charge(&mut self, pc: u32, units: usize) -> Result<(), VerifyError> {
        self.used = self.used.saturating_add(units);
        if self.used > self.limit {
            return Err(VerifyError::new(
                pc,
                VerifyErrorKind::ResourceLimit {
                    resource: Resource::WorkUnits,
                },
            ));
        }
        Ok(())
    }
}

pub(crate) struct StateProof {
    pub(crate) before: BTreeMap<u32, AbstractState>,
    pub(crate) after: BTreeMap<u32, AbstractState>,
    pub(crate) visited: BTreeSet<u32>,
    /// Innermost live catch offset before each instruction inside a region.
    pub(crate) handlers: BTreeMap<u32, ExceptionHandler>,
}

pub(crate) fn effective_pop(instruction: &Instruction) -> usize {
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
            .map_or(base, |arguments| base.saturating_add(arguments)),
        _ => base,
    }
}

pub(crate) fn local_index(instruction: &Instruction) -> Option<usize> {
    match instruction.opcode().format() {
        OperandFormat::Local => Some(instruction.operand_u16(1) as usize),
        OperandFormat::Local8 => Some(instruction.operand_u8(1) as usize),
        OperandFormat::NoneLocal => instruction
            .opcode()
            .name()
            .as_bytes()
            .last()
            .and_then(|value| value.is_ascii_digit().then_some((value - b'0') as usize)),
        _ => None,
    }
}

fn pushed_kind(snapshot: &CompileSnapshot, instruction: &Instruction) -> SlotKind {
    let name = instruction.opcode().name();
    if matches!(
        name,
        "push_i32"
            | "push_i8"
            | "push_i16"
            | "push_minus1"
            | "push_0"
            | "push_1"
            | "push_2"
            | "push_3"
            | "push_4"
            | "push_5"
            | "push_6"
            | "push_7"
    ) {
        SlotKind::Int32
    } else if matches!(name, "push_const" | "push_const8") {
        let index = if name == "push_const" {
            instruction.operand_u32(1)
        } else {
            u32::from(instruction.operand_u8(1))
        };
        match snapshot
            .constants()
            .get(index as usize)
            .map(|value| value.tag())
        {
            Some(tag) if tag == qjs::JS_TAG_INT => SlotKind::Int32,
            Some(tag) if tag == qjs::JS_TAG_FLOAT64 => SlotKind::Float64,
            _ => SlotKind::Tagged,
        }
    } else if name == "catch" {
        SlotKind::CatchOffset
    } else {
        SlotKind::Tagged
    }
}

fn copied_stack_values(name: &str, popped: &[SlotKind]) -> Option<Vec<SlotKind>> {
    let values = match (name, popped) {
        ("nip", &[_, b]) => vec![b],
        ("nip1", &[_, b, c]) => vec![b, c],
        ("dup", &[a]) => vec![a, a],
        ("dup1", &[a, b]) => vec![a, a, b],
        ("dup2", &[a, b]) => vec![a, b, a, b],
        ("dup3", &[a, b, c]) => vec![a, b, c, a, b, c],
        ("insert2", &[object, a]) => vec![a, object, a],
        ("insert3", &[object, property, a]) => vec![a, object, property, a],
        ("insert4", &[this, object, property, a]) => {
            vec![a, this, object, property, a]
        }
        ("perm3", &[object, a, b]) => vec![a, object, b],
        ("perm4", &[object, property, a, b]) => vec![a, object, property, b],
        ("perm5", &[this, object, property, a, b]) => vec![a, this, object, property, b],
        ("swap", &[a, b]) => vec![b, a],
        ("swap2", &[a, b, c, d]) => vec![c, d, a, b],
        ("rot3l", &[a, b, c]) => vec![b, c, a],
        ("rot3r", &[a, b, c]) => vec![c, a, b],
        ("rot4l", &[a, b, c, d]) => vec![b, c, d, a],
        ("rot5l", &[a, b, c, d, e]) => vec![b, c, d, e, a],
        _ => return None,
    };
    Some(values)
}

fn check_stack_size(
    snapshot: &CompileSnapshot,
    instruction: &Instruction,
    state: &AbstractState,
) -> Result<(), VerifyError> {
    if state.stack.len() > snapshot.stack_size() as usize {
        return Err(VerifyError::new(
            instruction.pc(),
            VerifyErrorKind::StackSizeExceeded {
                declared: snapshot.stack_size(),
                actual: state.stack.len(),
            },
        ));
    }
    Ok(())
}

fn transfer(
    snapshot: &CompileSnapshot,
    instruction: &Instruction,
    state: &mut AbstractState,
) -> Result<(), VerifyError> {
    let pop = effective_pop(instruction);
    if state.stack.len() < pop {
        return Err(VerifyError::new(
            instruction.pc(),
            VerifyErrorKind::StackUnderflow {
                needed: pop,
                available: state.stack.len(),
            },
        ));
    }

    let name = instruction.opcode().name();
    if name == "nip_catch" {
        // `catch_offset ... value -> value`: the interpreter releases every
        // operand down to the innermost catch offset, whatever its depth.
        let value = state.stack.pop().ok_or_else(|| {
            VerifyError::new(
                instruction.pc(),
                VerifyErrorKind::StackUnderflow {
                    needed: 1,
                    available: 0,
                },
            )
        })?;
        let Some((index, _)) = state.catches.pop() else {
            return Err(VerifyError::new(
                instruction.pc(),
                VerifyErrorKind::UnsupportedExceptionRegion,
            ));
        };
        if state.stack.get(index) != Some(&SlotKind::CatchOffset) {
            return Err(VerifyError::new(
                instruction.pc(),
                VerifyErrorKind::UnsupportedExceptionRegion,
            ));
        }
        state.stack.truncate(index);
        state.stack.push(value);
        return check_stack_size(snapshot, instruction, state);
    }
    let popped = state.stack.split_off(state.stack.len() - pop);
    let popped_top = popped.last().copied().unwrap_or(SlotKind::Tagged);
    if matches!(name, "for_of_next" | "for_in_next") {
        // The nominal pops are only the window the interpreter reads in
        // place: the enumeration record (including its catch offset) and any
        // intervening values stay on the stack unchanged, and the opcode
        // pushes `value` and the boolean `done` flag above them. This runs
        // before the consumed-catch-offset trim below: the for-of catch
        // offset stays live across `for_of_next`.
        state.stack.extend(popped);
        state.stack.extend([SlotKind::Tagged, SlotKind::Tagged]);
        return check_stack_size(snapshot, instruction, state);
    }

    // Any catch offset consumed by this instruction is no longer live.
    while state
        .catches
        .last()
        .is_some_and(|(index, _)| *index >= state.stack.len())
    {
        state.catches.pop();
    }

    if name == "get_loc0_loc1" {
        state.stack.push(state.locals[0]);
        state.stack.push(state.locals[1]);
        return check_stack_size(snapshot, instruction, state);
    }
    if name.starts_with("get_loc") {
        let index = local_index(instruction).expect("local format was validated");
        state.stack.push(state.locals[index]);
        return check_stack_size(snapshot, instruction, state);
    }
    if name == "set_loc_uninitialized" {
        let index = local_index(instruction).expect("local format was validated");
        state.locals[index] = SlotKind::Uninitialized;
    } else if name.starts_with("put_loc") || name.starts_with("set_loc") {
        let index = local_index(instruction).expect("local format was validated");
        state.locals[index] = popped_top;
    }

    if matches!(name, "inc_loc" | "dec_loc" | "add_loc") {
        let index = local_index(instruction).expect("local format was validated");
        state.locals[index] = SlotKind::Tagged;
    }

    if matches!(name, "for_of_start" | "for_await_of_start") {
        state
            .stack
            .extend([SlotKind::Tagged, SlotKind::Tagged, SlotKind::CatchOffset]);
        state.catches.push((state.stack.len() - 1, None));
        return check_stack_size(snapshot, instruction, state);
    }

    if name == "catch" {
        let target = instruction
            .branch_target()
            .and_then(|target| u32::try_from(target).ok())
            .ok_or_else(|| {
                VerifyError::new(
                    instruction.pc(),
                    VerifyErrorKind::UnsupportedExceptionRegion,
                )
            })?;
        state.stack.push(SlotKind::CatchOffset);
        state.catches.push((state.stack.len() - 1, Some(target)));
        return check_stack_size(snapshot, instruction, state);
    }

    if name == "gosub" {
        // The return address is an Int32 bytecode offset consumed by `ret`.
        state.stack.push(SlotKind::Int32);
        return check_stack_size(snapshot, instruction, state);
    }

    if name == "using_dispose_init" {
        state.stack.push(SlotKind::Uninitialized);
        return check_stack_size(snapshot, instruction, state);
    }

    if let Some(values) = copied_stack_values(name, &popped) {
        state.stack.extend(values);
        return check_stack_size(snapshot, instruction, state);
    }

    if name == "to_propkey" {
        // QuickJS leaves Int32 property keys unchanged; other numeric values
        // may become strings. Preserve only that exact fast-path proof so the
        // null-base bypass can rejoin without inventing a boxing operation.
        state.stack.push(if popped_top == SlotKind::Int32 {
            SlotKind::Int32
        } else {
            SlotKind::Tagged
        });
        return check_stack_size(snapshot, instruction, state);
    }

    if instruction.opcode().n_push() == 1
        && (name.starts_with("set_loc")
            || name.starts_with("set_arg")
            || name.starts_with("set_var_ref"))
    {
        state.stack.push(popped_top);
        return check_stack_size(snapshot, instruction, state);
    }

    let push = instruction.opcode().n_push() as usize;
    state.stack.extend(std::iter::repeat_n(
        pushed_kind(snapshot, instruction),
        push,
    ));
    check_stack_size(snapshot, instruction, state)
}

fn merge_state(
    pc: u32,
    expected: &mut AbstractState,
    actual: &AbstractState,
) -> Result<bool, VerifyError> {
    if expected.stack.len() != actual.stack.len() {
        return Err(VerifyError::new(
            pc,
            VerifyErrorKind::InconsistentMergeHeight {
                expected: expected.stack.len(),
                actual: actual.stack.len(),
            },
        ));
    }
    if expected.catches != actual.catches {
        return Err(VerifyError::new(
            pc,
            VerifyErrorKind::UnsupportedExceptionRegion,
        ));
    }
    // Operand-stack representations must agree exactly. Unlike locals, these
    // are transient values consumed directly by the following bytecode. A
    // merge is not an operation and therefore cannot silently insert the
    // boxing/conversion that would be needed to change their representation.
    for (slot, (expected, actual)) in expected.stack.iter().zip(&actual.stack).enumerate() {
        if expected != actual {
            return Err(VerifyError::new(
                pc,
                VerifyErrorKind::IncompatibleMergeKind {
                    slot,
                    expected: *expected,
                    actual: *actual,
                },
            ));
        }
    }

    let mut changed = false;
    let stack_slots = expected.stack.len();
    for (local, (expected, actual)) in expected.locals.iter_mut().zip(&actual.locals).enumerate() {
        if expected == actual {
            continue;
        }
        let prior = *expected;
        let joined = match (prior, *actual) {
            // Locals are authoritative tagged JSValue cells in the interpreter
            // frame. SlotKind records what the current path proves about their
            // contents; losing that proof at a join does not box a value. This
            // conservative join is needed for loop phis whose entry value is
            // unknown and whose backedge has established a numeric kind.
            (SlotKind::Tagged, SlotKind::Int32 | SlotKind::Float64)
            | (SlotKind::Int32 | SlotKind::Float64, SlotKind::Tagged)
            | (SlotKind::Int32, SlotKind::Float64)
            | (SlotKind::Float64, SlotKind::Int32) => SlotKind::Tagged,
            // A lexical binding declared in a loop body (for example the
            // `for (const value of values)` binding) is uninitialized on loop
            // entry and initialized on the backedge. The interpreter cell
            // holds the JS_UNINITIALIZED tag in the first case, which is still
            // a tagged value; the join only drops the proof, and every read
            // of such a binding is a `get_loc_check`. Tier 1 keeps that
            // runtime check; Tier 2 lowers it as a plain load and therefore
            // separately proves definite initialization before admitting it
            // (`ir::optimized::prove_lexical_checks`).
            (SlotKind::Tagged | SlotKind::Int32 | SlotKind::Float64, SlotKind::Uninitialized)
            | (SlotKind::Uninitialized, SlotKind::Tagged | SlotKind::Int32 | SlotKind::Float64) => {
                SlotKind::Tagged
            }
            // Catch offsets are verifier-only states, not interchangeable JS
            // value representations.
            _ => {
                return Err(VerifyError::new(
                    pc,
                    VerifyErrorKind::IncompatibleMergeKind {
                        slot: stack_slots + local,
                        expected: prior,
                        actual: *actual,
                    },
                ));
            }
        };
        if joined != prior {
            *expected = joined;
            changed = true;
        }
    }
    Ok(changed)
}

pub(crate) fn prove(
    snapshot: &CompileSnapshot,
    instructions: &[Instruction],
    cfg: &ControlFlowGraph,
    max_work_units: usize,
) -> Result<StateProof, VerifyError> {
    if instructions.is_empty() {
        return Ok(StateProof {
            before: BTreeMap::new(),
            after: BTreeMap::new(),
            visited: BTreeSet::new(),
            handlers: BTreeMap::new(),
        });
    }
    let before_points: BTreeSet<u32> = snapshot
        .data
        .metadata
        .osr_points
        .iter()
        .map(|point| point.pc)
        .chain(
            snapshot
                .data
                .metadata
                .deopt_points
                .iter()
                .map(|point| point.pc),
        )
        .chain(
            cfg.blocks()
                .iter()
                .map(|block| block.start_pc())
                .filter(|pc| cfg.is_loop_header(*pc)),
        )
        .collect();
    let after_points: BTreeSet<u32> = snapshot
        .data
        .metadata
        .deopt_points
        .iter()
        .map(|point| point.pc)
        .collect();
    let mut budget = WorkBudget {
        used: 0,
        limit: max_work_units,
    };
    budget.charge(0, snapshot.local_count() as usize)?;
    let mut block_entries = BTreeMap::new();
    block_entries.insert(
        0,
        AbstractState {
            locals: vec![SlotKind::Tagged; snapshot.local_count() as usize],
            stack: Vec::new(),
            catches: Vec::new(),
        },
    );
    let mut queue = VecDeque::from([0_u32]);
    let mut before = BTreeMap::new();
    let mut after = BTreeMap::new();
    let mut visited = BTreeSet::new();
    let mut handlers = BTreeMap::new();

    while let Some(block_pc) = queue.pop_front() {
        let block = cfg.block(block_pc).expect("CFG successor names a block");
        let entry = &block_entries[&block_pc];
        budget.charge(block_pc, entry.cell_count())?;
        let mut state = entry.clone();
        for instruction in &instructions[block.instruction_range()] {
            budget.charge(instruction.pc(), 1usize.saturating_add(state.cell_count()))?;
            if before_points.contains(&instruction.pc()) {
                budget.charge(instruction.pc(), state.cell_count())?;
                before.insert(instruction.pc(), state.clone());
            }
            visited.insert(instruction.pc());
            if let Some(handler) = state.innermost_catch() {
                handlers.insert(instruction.pc(), handler);
            }
            // Any instruction inside a try region may transfer to its
            // handler with the locals it observed on entry.
            if let Some((_, handler_pc)) = state.innermost_handler() {
                budget.charge(instruction.pc(), state.locals.len())?;
                let Some(expected) = block_entries.get_mut(&handler_pc) else {
                    return Err(VerifyError::new(
                        instruction.pc(),
                        VerifyErrorKind::UnsupportedExceptionRegion,
                    ));
                };
                if join_handler_locals(handler_pc, expected, &state.locals)? {
                    queue.push_back(handler_pc);
                }
            }
            transfer(snapshot, instruction, &mut state)?;
            if after_points.contains(&instruction.pc()) {
                budget.charge(instruction.pc(), state.cell_count())?;
                after.insert(instruction.pc(), state.clone());
            }
        }
        let tail = &instructions[block.instruction_range()]
            .last()
            .expect("CFG blocks are non-empty");
        let catch_target = (tail.opcode().name() == "catch")
            .then(|| tail.branch_target())
            .flatten()
            .and_then(|target| u32::try_from(target).ok());
        // A handler must be distinct from the protected fallthrough, which
        // continues with the catch offset itself on the stack.
        if catch_target.is_some_and(|target| {
            usize::try_from(target).ok() == (tail.pc() as usize).checked_add(tail.size())
        }) {
            return Err(VerifyError::new(
                tail.pc(),
                VerifyErrorKind::UnsupportedExceptionRegion,
            ));
        }
        for successor in block.successors() {
            if catch_target == Some(*successor) {
                // The interpreter resumes a handler with the caught value in
                // place of its catch offset and that offset no longer live.
                let mut incoming = state.clone();
                let caught = incoming.stack.last_mut().filter(|kind| {
                    **kind == SlotKind::CatchOffset
                        && incoming
                            .catches
                            .last()
                            .is_some_and(|(_, handler)| *handler == Some(*successor))
                });
                let Some(caught) = caught else {
                    return Err(VerifyError::new(
                        tail.pc(),
                        VerifyErrorKind::UnsupportedExceptionRegion,
                    ));
                };
                *caught = SlotKind::Tagged;
                incoming.catches.pop();
                budget.charge(*successor, incoming.cell_count())?;
                if let Some(expected) = block_entries.get_mut(successor) {
                    if expected.stack != incoming.stack || expected.catches != incoming.catches {
                        return Err(VerifyError::new(
                            *successor,
                            VerifyErrorKind::UnsupportedExceptionRegion,
                        ));
                    }
                    if join_handler_locals(*successor, expected, &incoming.locals)? {
                        queue.push_back(*successor);
                    }
                } else {
                    block_entries.insert(*successor, incoming);
                    queue.push_back(*successor);
                }
                continue;
            }
            let mut incoming = state.clone();
            if cfg.is_loop_header(*successor) {
                // Establish the conservative frame-domain fixed point before
                // propagating values out of a loop header. Otherwise the
                // first forward visit can publish Int32-derived operand-stack
                // states, then a later backedge widens the source local to
                // Tagged and looks like an incompatible stack merge even
                // though no bytecode control-flow join changed representation.
                for local in &mut incoming.locals {
                    if matches!(*local, SlotKind::Int32 | SlotKind::Float64) {
                        *local = SlotKind::Tagged;
                    }
                }
            }
            if let Some(expected) = block_entries.get_mut(successor) {
                budget.charge(*successor, incoming.cell_count())?;
                if merge_state(*successor, expected, &incoming)? {
                    queue.push_back(*successor);
                }
            } else {
                budget.charge(*successor, incoming.cell_count())?;
                block_entries.insert(*successor, incoming);
                queue.push_back(*successor);
            }
        }
    }
    Ok(StateProof {
        before,
        after,
        visited,
        handlers,
    })
}

/// Joins the locals observed at one exceptional edge into a handler entry.
/// Unlike an ordinary merge, a handler is reached from every instruction of
/// its try region, including those before a block-scoped binding in the
/// region is initialized. That binding is out of scope in the handler, so an
/// initialized/uninitialized disagreement conservatively becomes `Tagged`,
/// which proves nothing about the cell's contents.
fn join_handler_locals(
    pc: u32,
    expected: &mut AbstractState,
    actual: &[SlotKind],
) -> Result<bool, VerifyError> {
    if expected.locals.len() != actual.len() {
        return Err(VerifyError::new(
            pc,
            VerifyErrorKind::UnsupportedExceptionRegion,
        ));
    }
    let mut changed = false;
    for (expected, actual) in expected.locals.iter_mut().zip(actual) {
        if expected == actual {
            continue;
        }
        if matches!(*expected, SlotKind::CatchOffset) || matches!(*actual, SlotKind::CatchOffset) {
            return Err(VerifyError::new(
                pc,
                VerifyErrorKind::UnsupportedExceptionRegion,
            ));
        }
        if *expected != SlotKind::Tagged {
            *expected = SlotKind::Tagged;
            changed = true;
        }
    }
    Ok(changed)
}
