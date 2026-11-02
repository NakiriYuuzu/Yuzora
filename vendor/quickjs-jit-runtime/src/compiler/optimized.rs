#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NumericBinaryOp {
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Clone, Copy, Debug)]
pub enum NumericConstant {
    Int32(i32),
    Float64(f64),
}

impl NumericConstant {
    pub fn as_f64(self) -> Option<f64> {
        Some(match self {
            Self::Int32(value) => f64::from(value),
            Self::Float64(value) => value,
        })
    }
    pub fn is_negative_zero(self) -> bool {
        matches!(self, Self::Float64(value) if value.to_bits() == (-0.0f64).to_bits())
    }
}

#[derive(Clone, Copy, Debug)]
pub enum OptimizedInput {
    Constant(NumericConstant),
    Binary {
        op: NumericBinaryOp,
        lhs: u32,
        rhs: u32,
    },
    Return(u32),
}

impl OptimizedInput {
    pub const fn constant_i32(value: i32) -> Self {
        Self::Constant(NumericConstant::Int32(value))
    }
    pub const fn constant_f64(value: f64) -> Self {
        Self::Constant(NumericConstant::Float64(value))
    }
    pub const fn binary(op: NumericBinaryOp, lhs: u32, rhs: u32) -> Self {
        Self::Binary { op, lhs, rhs }
    }
    pub const fn ret(value: u32) -> Self {
        Self::Return(value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OptimizedCompileError {
    InvalidValue,
    MissingReturn,
    Unsupported,
}

#[derive(Debug)]
pub struct OptimizedFunction {
    constants: Box<[Option<NumericConstant>]>,
    representatives: Box<[u32]>,
    operands: Box<[Option<(u32, u32)>]>,
    return_value: u32,
    boxes_elided: u64,
    cse_eliminated: u64,
    dead_nodes_eliminated: u64,
}

impl OptimizedFunction {
    pub fn constant(&self, value: u32) -> Option<NumericConstant> {
        self.constants.get(value as usize).copied().flatten()
    }
    pub const fn return_value(&self) -> u32 {
        self.return_value
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
    pub fn representative(&self, value: u32) -> Option<u32> {
        self.representatives.get(value as usize).copied()
    }
    pub fn operands(&self, value: u32) -> Option<(u32, u32)> {
        self.operands.get(value as usize).copied().flatten()
    }
}

#[derive(Debug, Default)]
pub struct OptimizedCompiler;

impl OptimizedCompiler {
    pub fn compile(
        &mut self,
        inputs: &[OptimizedInput],
    ) -> Result<OptimizedFunction, OptimizedCompileError> {
        let mut constants = Vec::with_capacity(inputs.len());
        let mut return_value = None;
        let mut boxes_elided = 0u64;
        let mut cse_eliminated = 0u64;
        let mut expressions = std::collections::BTreeMap::<(u8, u32, u32), u32>::new();
        let mut operands = Vec::<Option<(u32, u32)>>::with_capacity(inputs.len());
        let mut representatives = Vec::<u32>::with_capacity(inputs.len());
        for input in inputs {
            let value_id =
                u32::try_from(constants.len()).map_err(|_| OptimizedCompileError::InvalidValue)?;
            let mut representative = value_id;
            let folded = match *input {
                OptimizedInput::Constant(value) => Some(value),
                OptimizedInput::Binary { op, lhs, rhs } => {
                    let lhs = *representatives
                        .get(lhs as usize)
                        .ok_or(OptimizedCompileError::InvalidValue)?;
                    let rhs = *representatives
                        .get(rhs as usize)
                        .ok_or(OptimizedCompileError::InvalidValue)?;
                    let lhs_value = constants
                        .get(lhs as usize)
                        .copied()
                        .flatten()
                        .ok_or(OptimizedCompileError::InvalidValue)?;
                    let rhs_value = constants
                        .get(rhs as usize)
                        .copied()
                        .flatten()
                        .ok_or(OptimizedCompileError::InvalidValue)?;
                    boxes_elided = boxes_elided.saturating_add(1);
                    let opcode = match op {
                        NumericBinaryOp::Add => 0,
                        NumericBinaryOp::Sub => 1,
                        NumericBinaryOp::Mul => 2,
                        NumericBinaryOp::Div => 3,
                    };
                    if let Some(existing) = expressions.get(&(opcode, lhs, rhs)).copied() {
                        representative = existing;
                        cse_eliminated = cse_eliminated.saturating_add(1);
                    } else {
                        expressions.insert((opcode, lhs, rhs), value_id);
                    }
                    Some(fold(op, lhs_value, rhs_value))
                }
                OptimizedInput::Return(value) => {
                    return_value = Some(
                        *representatives
                            .get(value as usize)
                            .ok_or(OptimizedCompileError::InvalidValue)?,
                    );
                    None
                }
            };
            operands.push(match *input {
                OptimizedInput::Binary { lhs, rhs, .. } => Some((
                    *representatives
                        .get(lhs as usize)
                        .ok_or(OptimizedCompileError::InvalidValue)?,
                    *representatives
                        .get(rhs as usize)
                        .ok_or(OptimizedCompileError::InvalidValue)?,
                )),
                _ => None,
            });
            constants.push(folded);
            representatives.push(representative);
        }
        let return_value = return_value.ok_or(OptimizedCompileError::MissingReturn)?;
        let mut live = vec![false; inputs.len()];
        let mut work = vec![return_value];
        while let Some(value) = work.pop() {
            let Some(slot) = live.get_mut(value as usize) else {
                return Err(OptimizedCompileError::InvalidValue);
            };
            if *slot {
                continue;
            }
            *slot = true;
            if let Some((lhs, rhs)) = operands[value as usize] {
                work.extend([lhs, rhs]);
            }
        }
        let dead_nodes_eliminated = inputs
            .iter()
            .enumerate()
            .filter(|(index, input)| !live[*index] && !matches!(input, OptimizedInput::Return(_)))
            .count() as u64;
        Ok(OptimizedFunction {
            constants: constants.into_boxed_slice(),
            representatives: representatives.into_boxed_slice(),
            operands: operands.into_boxed_slice(),
            return_value,
            boxes_elided,
            cse_eliminated,
            dead_nodes_eliminated,
        })
    }
}

fn fold(op: NumericBinaryOp, lhs: NumericConstant, rhs: NumericConstant) -> NumericConstant {
    if let (NumericConstant::Int32(lhs), NumericConstant::Int32(rhs)) = (lhs, rhs) {
        if matches!(op, NumericBinaryOp::Mul) && (lhs == 0 || rhs == 0) && (lhs < 0 || rhs < 0) {
            return NumericConstant::Float64(-0.0);
        }
        let exact = match op {
            NumericBinaryOp::Add => lhs.checked_add(rhs),
            NumericBinaryOp::Sub => lhs.checked_sub(rhs),
            NumericBinaryOp::Mul => lhs.checked_mul(rhs),
            NumericBinaryOp::Div => None,
        };
        if let Some(value) = exact {
            return NumericConstant::Int32(value);
        }
    }
    let lhs = lhs.as_f64().expect("numeric constant");
    let rhs = rhs.as_f64().expect("numeric constant");
    NumericConstant::Float64(match op {
        NumericBinaryOp::Add => lhs + rhs,
        NumericBinaryOp::Sub => lhs - rhs,
        NumericBinaryOp::Mul => lhs * rhs,
        NumericBinaryOp::Div => lhs / rhs,
    })
}

mod array_cache;
/// Production Tier 2 compiler. Its deliberately narrow first implementation
/// reuses the audited Tier 1 machine lowering after proving a numeric/local
/// subset and attaches exact guard/deopt metadata. Unsupported semantics reject
/// the tier and leave Tier 1 installed.
mod call_guards;
mod const_canon;
mod element_address;
mod frame_inline;
mod property_cache;

pub struct Tier2Compiler {
    isa: cranelift_codegen::isa::OwnedTargetIsa,
    feedback_epoch: u64,
}

#[derive(Clone, Copy)]
struct OptPair {
    payload: cranelift_codegen::ir::Value,
    tag: cranelift_codegen::ir::Value,
}

#[derive(Clone, Copy)]
struct OptVars {
    payload: cranelift_frontend::Variable,
    tag: cranelift_frontend::Variable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OptProvenance {
    Argument(usize),
    Local(usize),
    ImmediatePrimitive,
    /// The interpreter stack slot at this index owns the value (a helper
    /// wrote it there). Exits and the call bridge leave it in place; it may
    /// be consumed by a call, returned, freed by `drop`, or moved into a
    /// local. Duplication must create a second owner through the DUP helper.
    OwnedSlot,
    Unknown,
}

fn merge_opt_provenance(lhs: OptProvenance, rhs: OptProvenance) -> OptProvenance {
    if lhs == rhs {
        lhs
    } else {
        OptProvenance::Unknown
    }
}

struct ProvenanceBudget<'a> {
    work: usize,
    control: Option<&'a CompileControl>,
}

impl ProvenanceBudget<'_> {
    fn charge(&mut self, work: usize) -> Result<(), CompileFailure> {
        if let Some(control) = self.control {
            control.check()?;
        }
        self.work = self
            .work
            .checked_sub(work)
            .ok_or(CompileFailure::ResourceLimit)?;
        Ok(())
    }
}

fn resolve_opt_phi_provenance<I: Iterator<Item = usize>>(
    values: &mut [Option<OptProvenance>],
    phi_inputs: impl Fn(usize) -> Option<I>,
    budget: &mut ProvenanceBudget<'_>,
) -> Result<(), CompileFailure> {
    // Tentatively seed loop equations from concrete predecessors. Do not expose
    // those candidates until every input is resolved: seedless components are
    // widened to Unknown below, then that uncertainty is propagated again.
    loop {
        let mut changed = false;
        for index in 0..values.len() {
            budget.charge(1)?;
            let Some(inputs) = phi_inputs(index) else {
                continue;
            };
            let mut merged = None;
            for input in inputs {
                budget.charge(1)?;
                let provenance = *values.get(input).ok_or(CompileFailure::InvalidArtifact)?;
                if let Some(provenance) = provenance {
                    merged = Some(match merged {
                        Some(previous) => merge_opt_provenance(previous, provenance),
                        None => provenance,
                    });
                }
            }
            let Some(merged) = merged else { continue };
            let widened = match values[index] {
                Some(previous) => merge_opt_provenance(previous, merged),
                None => merged,
            };
            if values[index] != Some(widened) {
                values[index] = Some(widened);
                changed = true;
            }
        }
        if !changed {
            for value in values.iter_mut() {
                budget.charge(1)?;
                if value.is_none() {
                    *value = Some(OptProvenance::Unknown);
                    changed = true;
                }
            }
            if !changed {
                return Ok(());
            }
        }
    }
}

/// Compute the ownership/value-origin state at every CFG block entry. This is
/// the same conservative join used by optimizing JavaScript engines for frame
/// states: a borrowed value survives only when every incoming edge names the
/// same root, while primitive and slot-owned values merge by value class.
fn opt_cfg_entry_provenance(
    ir: &OptimizedIr,
    specialization: &NumericSpecialization,
    stack_slots: usize,
    live_analysis_bytes: usize,
    control: Option<&CompileControl>,
) -> Result<std::collections::BTreeMap<u32, Box<[OptProvenance]>>, CompileFailure> {
    use crate::ir::{FrameSlot, ScalarValue};

    let graph = ir.scalar_graph();
    let mut budget = ProvenanceBudget {
        work: graph
            .values()
            .len()
            .saturating_add(ir.blocks().len())
            .saturating_mul(128)
            .min(1_048_576),
        control,
    };
    budget.charge(graph.values().len())?;
    // Account for the value lattice and retained entry states before allocating.
    // The map allowance includes its nodes and allocator overhead per entry.
    let mut scratch_bytes = graph
        .values()
        .len()
        .checked_mul(
            core::mem::size_of::<Option<OptProvenance>>()
                + core::mem::size_of::<Option<crate::ir::ScalarValueId>>(),
        )
        .ok_or(CompileFailure::ResourceLimit)?;
    for block in ir.blocks() {
        budget.charge(1)?;
        let depth = usize::from(block.stack_depth());
        if depth > stack_slots {
            return Err(CompileFailure::ResourceLimit);
        }
        scratch_bytes = depth
            .checked_mul(core::mem::size_of::<OptProvenance>())
            .and_then(|bytes| bytes.checked_add(512))
            .and_then(|bytes| scratch_bytes.checked_add(bytes))
            .ok_or(CompileFailure::ResourceLimit)?;
    }
    if let Some(control) = control {
        control.check_ir_bytes(
            live_analysis_bytes
                .checked_add(scratch_bytes)
                .ok_or(CompileFailure::ResourceLimit)?,
        )?;
    }
    let mut values = vec![None; graph.values().len()];
    let mut guarded_aliases = vec![None; graph.values().len()];
    // The native ToPropertyKey path only accepts Int32/string/symbol and
    // returns the very same borrowed value. Preserve that ownership across
    // CFG joins; the generic semantic graph correctly leaves its result
    // opaque because a general conversion can allocate or run script.
    for node in ir.nodes() {
        budget.charge(1)?;
        if matches!(node.kind(), crate::ir::OptimizedNodeKind::Bytecode { opcode }
            if opcode.as_ref() == "to_propkey")
        {
            let source = graph
                .frame_state_for_node(node.id())
                .and_then(|state| state.stack.last())
                .copied()
                .ok_or(CompileFailure::InvalidArtifact)?;
            let [result] = graph.outputs_for_node(node.id()) else {
                return Err(CompileFailure::InvalidArtifact);
            };
            guarded_aliases[result.index()] = Some(source);
        }
    }
    for (index, value) in graph.values().iter().enumerate() {
        budget.charge(1)?;
        if guarded_aliases[index].is_some() {
            continue;
        }
        values[index] = match value {
            ScalarValue::Int32(_)
            | ScalarValue::Bool(_)
            | ScalarValue::Binary { .. }
            | ScalarValue::Update { .. }
            | ScalarValue::Bitwise { .. }
            | ScalarValue::Compare { .. } => Some(OptProvenance::ImmediatePrimitive),
            ScalarValue::Input {
                block_pc: 0,
                slot: FrameSlot::Argument(index),
            } => Some(OptProvenance::Argument(usize::from(*index))),
            ScalarValue::Input {
                block_pc: 0,
                slot: FrameSlot::Local(index),
            }
            | ScalarValue::FrameRead {
                slot: FrameSlot::Local(index),
                ..
            } => Some(OptProvenance::Local(usize::from(*index))),
            ScalarValue::FrameRead {
                slot: FrameSlot::Argument(index),
                ..
            } => Some(OptProvenance::Argument(usize::from(*index))),
            ScalarValue::Call(call) => {
                let node = ir
                    .nodes()
                    .get(call.frame_state_node as usize)
                    .ok_or(CompileFailure::InvalidArtifact)?;
                Some(if call.frame_inline.is_some() {
                    OptProvenance::OwnedSlot
                } else if call.inline.is_some()
                    || specialization.calls.get(&node.pc()).is_some_and(|call| {
                        call.result() != crate::runtime::FeedbackRepresentation::HeapRef
                    })
                {
                    OptProvenance::ImmediatePrimitive
                } else {
                    // Both the generic bridge and frame-backed inline ABI leave
                    // their result rooted in an interpreter stack slot.
                    OptProvenance::OwnedSlot
                })
            }
            // Bottom is reserved for unresolved Phi equations. Every unknown
            // producer is top and must participate in the join.
            ScalarValue::Phi { .. } => None,
            ScalarValue::Input { .. }
            | ScalarValue::FrameRead {
                slot: FrameSlot::Stack(_),
                ..
            }
            | ScalarValue::GetProperty { .. }
            | ScalarValue::GetElement { .. }
            | ScalarValue::Opaque => Some(OptProvenance::Unknown),
        };
    }

    resolve_opt_phi_provenance(
        &mut values,
        |index| {
            let inputs = match &graph.values()[index] {
                ScalarValue::Phi { inputs, .. } => inputs.as_ref(),
                _ if guarded_aliases[index].is_some() => &[],
                _ => return None,
            };
            Some(
                inputs
                    .iter()
                    .map(|input| input.value.index())
                    .chain(guarded_aliases[index].map(|value| value.index())),
            )
        },
        &mut budget,
    )?;

    let mut entries = std::collections::BTreeMap::new();
    let block_lookup_work = (usize::BITS - ir.blocks().len().leading_zeros()) as usize;
    for block in ir.blocks() {
        // The block-input lookup and map insertion both have logarithmic cost.
        budget.charge(1 + 2 * block_lookup_work)?;
        let depth = usize::from(block.stack_depth());
        if depth > stack_slots {
            return Err(CompileFailure::ResourceLimit);
        }
        budget.charge(depth)?;
        let mut entry = vec![OptProvenance::Unknown; depth];
        for &(slot, value) in graph.inputs_for_block(block.start_pc()) {
            budget.charge(1)?;
            let FrameSlot::Stack(index) = slot else {
                continue;
            };
            let index = usize::from(index);
            if index >= depth {
                return Err(CompileFailure::InvalidArtifact);
            }
            entry[index] = values
                .get(value.index())
                .copied()
                .flatten()
                .unwrap_or(OptProvenance::Unknown);
        }
        entries.insert(block.start_pc(), entry.into_boxed_slice());
    }
    Ok(entries)
}

#[cfg(test)]
mod provenance_merge_tests {
    use super::{
        merge_opt_provenance, resolve_opt_phi_provenance, CompileControl, CompileFailure,
        OptProvenance, ProvenanceBudget,
    };

    fn solve(
        values: &mut [Option<OptProvenance>],
        inputs: &[Option<&[usize]>],
        work: usize,
        control: Option<&CompileControl>,
    ) -> Result<(), CompileFailure> {
        resolve_opt_phi_provenance(
            values,
            |index| inputs[index].map(|inputs| inputs.iter().copied()),
            &mut ProvenanceBudget { work, control },
        )
    }

    #[test]
    fn phi_unknown_input_poisoning_reaches_the_whole_loop() {
        use OptProvenance::{Argument, ImmediatePrimitive, Unknown};

        for known in [Argument(0), ImmediatePrimitive] {
            let mut values = [Some(known), Some(Unknown), None, None];
            let inputs: &[Option<&[usize]>] = &[None, None, Some(&[0, 3]), Some(&[2, 1])];
            solve(&mut values, inputs, 128, None).unwrap();
            assert_eq!(
                values,
                [Some(known), Some(Unknown), Some(Unknown), Some(Unknown)]
            );
        }
    }

    #[test]
    fn seedless_phi_component_invalidates_a_seeded_consumer() {
        use OptProvenance::{Argument, Unknown};

        let mut values = [Some(Argument(0)), None, None, None];
        let inputs: &[Option<&[usize]>] = &[None, Some(&[2]), Some(&[1]), Some(&[0, 1])];
        solve(&mut values, inputs, 128, None).unwrap();
        assert_eq!(
            values,
            [
                Some(Argument(0)),
                Some(Unknown),
                Some(Unknown),
                Some(Unknown)
            ]
        );
    }

    #[test]
    fn seeded_phi_cycle_preserves_only_agreeing_origins() {
        use OptProvenance::{Argument, Unknown};

        for (second, expected) in [(Argument(0), Argument(0)), (Argument(1), Unknown)] {
            let mut values = [Some(Argument(0)), Some(second), None, None];
            let inputs: &[Option<&[usize]>] = &[None, None, Some(&[0, 3]), Some(&[2, 1])];
            solve(&mut values, inputs, 128, None).unwrap();
            assert_eq!(values[2..], [Some(expected), Some(expected)]);
        }
    }

    #[test]
    fn phi_analysis_rejects_partial_results_on_work_exhaustion_or_cancellation() {
        use std::sync::{atomic::AtomicBool, Arc};
        use std::time::Duration;

        let inputs: &[Option<&[usize]>] = &[None, Some(&[0, 1])];
        let initial = [Some(OptProvenance::Argument(0)), None];
        for work in 0..8 {
            let mut values = initial;
            assert_eq!(
                solve(&mut values, inputs, work, None),
                Err(CompileFailure::ResourceLimit)
            );
        }
        let cancelled =
            CompileControl::new(Arc::new(AtomicBool::new(true)), Duration::from_secs(60));
        let mut values = initial;
        assert_eq!(
            solve(&mut values, inputs, 128, Some(&cancelled)),
            Err(CompileFailure::Cancelled)
        );
        let expired = CompileControl::new(Arc::new(AtomicBool::new(false)), Duration::ZERO);
        assert_eq!(
            solve(&mut values, inputs, 128, Some(&expired)),
            Err(CompileFailure::TimedOut)
        );
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn cfg_provenance_charges_scratch_in_addition_to_live_analyses() {
        use std::sync::{atomic::AtomicBool, Arc};
        use std::time::Duration;

        let fixture = crate::test_support::SnapshotFixture::compile("(function(a){return a})");
        let verified = fixture
            .snapshot()
            .verify(crate::bytecode::VerifyLimits::default())
            .unwrap();
        let ir = crate::ir::OptimizedIr::translate(&verified, 1).unwrap();
        let control = CompileControl::with_ir_limit(
            Arc::new(AtomicBool::new(false)),
            Duration::from_secs(60),
            4096,
        );
        assert_eq!(
            super::opt_cfg_entry_provenance(&ir, &Default::default(), 16, 4096, Some(&control)),
            Err(CompileFailure::ResourceLimit)
        );
        assert!(
            super::opt_cfg_entry_provenance(&ir, &Default::default(), 16, 0, Some(&control))
                .is_ok()
        );
    }

    #[test]
    fn cfg_merge_preserves_only_identical_borrows_or_uniform_value_classes() {
        use OptProvenance::{Argument, ImmediatePrimitive, Local, OwnedSlot, Unknown};

        assert_eq!(merge_opt_provenance(Argument(2), Argument(2)), Argument(2));
        assert_eq!(merge_opt_provenance(Local(1), Local(1)), Local(1));
        assert_eq!(
            merge_opt_provenance(ImmediatePrimitive, ImmediatePrimitive),
            ImmediatePrimitive
        );
        assert_eq!(merge_opt_provenance(OwnedSlot, OwnedSlot), OwnedSlot);
        assert_eq!(merge_opt_provenance(Argument(2), Argument(3)), Unknown);
        assert_eq!(merge_opt_provenance(Argument(2), Local(2)), Unknown);
        assert_eq!(merge_opt_provenance(OwnedSlot, ImmediatePrimitive), Unknown);
        assert_eq!(merge_opt_provenance(Unknown, OwnedSlot), Unknown);
    }

    #[cfg(feature = "test-support")]
    #[test]
    fn owned_local_join_does_not_inherit_the_last_layout_blocks_primitive() {
        let fixture = crate::test_support::SnapshotFixture::compile(
            "(function(f,choose){let value=choose?f():null;return value})",
        );
        let verified = fixture
            .snapshot()
            .verify(crate::bytecode::VerifyLimits::default())
            .unwrap();
        let ir = crate::ir::OptimizedIr::translate(&verified, 1).unwrap();
        let entries = super::opt_cfg_entry_provenance(
            &ir,
            &Default::default(),
            usize::from(ir.max_stack()) + crate::ir::MAX_HELPER_SCRATCH_SLOTS,
            0,
            None,
        )
        .unwrap();
        let ownership = super::owned_local_targets(&ir, &Default::default(), &entries).unwrap();
        assert_eq!(ownership, [true], "an owning incoming value must not be erased by the primitive predecessor's layout order");
    }

    #[test]
    fn owned_local_uses_its_cfg_predecessor_not_the_previous_layout_block() {
        use crate::bytecode::{linked_opcode_table, CompileSnapshot, VerifyLimits};
        let opcode = |name| {
            linked_opcode_table()
                .find(|op| op.name() == name)
                .unwrap()
                .id()
        };
        // 0: choose ? block3 : block7
        // 3: call f(), jump to block9 with an owned stack result
        // 7: return null (overwrites the same abstract slot; no edge to block9)
        // 9: move the call result into local0, then return local0.
        let verified = CompileSnapshot::from_untrusted_bytecode(
            vec![
                opcode("get_arg1"),
                opcode("if_false8"),
                5,
                opcode("get_arg0"),
                opcode("call0"),
                opcode("goto8"),
                3,
                opcode("null"),
                opcode("return"),
                opcode("put_loc0"),
                opcode("get_loc0"),
                opcode("return"),
            ],
            2,
            1,
            0,
            0,
        )
        .verify(VerifyLimits::default())
        .unwrap();
        let ir = crate::ir::OptimizedIr::translate(&verified, 1).unwrap();
        let entries = super::opt_cfg_entry_provenance(
            &ir,
            &Default::default(),
            usize::from(ir.max_stack()) + crate::ir::MAX_HELPER_SCRATCH_SLOTS,
            0,
            None,
        )
        .unwrap();
        assert_eq!(entries[&9].as_ref(), [OptProvenance::OwnedSlot]);
        assert_eq!(
            super::owned_local_targets(&ir, &Default::default(), &entries).unwrap(),
            [true]
        );
    }
}

#[derive(Clone, Copy, PartialEq)]
struct GuardedElementSource {
    provenance: OptProvenance,
    block_pc: u32,
    data: cranelift_codegen::ir::Value,
    count: cranelift_codegen::ir::Value,
    kind: cranelift_codegen::ir::Value,
    /// The continuing-path guard proved the observable `length` value equals
    /// `count`: exact dense Array length or the intrinsic typed-array getter.
    /// An element-only query does not establish this property-lookup fact.
    exact_length: bool,
    /// A typed-array leaf query guarded fixed backing for this exact mode.
    /// `exact_length` separately records whether it guarded the intrinsic
    /// lookup. Both facts are revalidated after polls.
    typed_mode: Option<crate::runtime::ArrayMode>,
    /// Compile-time value of `kind` when every path that established this
    /// source produced one storage kind (0 packed, 1 Int32, 2 Float64).
    /// Cached accesses then emit only that kind's load instead of a runtime
    /// dispatch on `kind`.
    static_kind: Option<i64>,
    /// Established by a storage-only loop hoist that does not side-exit.
    /// Such a hoist runs in the preheader even when the loop body never
    /// reaches the access (zero trips, a conditional access), so a failed
    /// storage query must not deopt there. Instead `count` is 0 and `data`
    /// null, and every consumer's retained bounds check exits at the access
    /// itself, exactly where the unhoisted guard would have. Consumers must
    /// therefore never skip the bounds check for such a source (it never has
    /// `exact_length`), and poll revalidation is skipped while `count` is 0.
    unguarded: bool,
}

/// Guarded metadata tuples that are live at the current lowering point, keyed
/// by the frame slot (argument or local) that holds each receiver. Every entry
/// is individually valid: operations that can reenter, rebind a frame slot or
/// alter storage clear the whole set, exactly like the former single slot.
/// Leaf-or-exit array operations add or replace only their own receiver.
#[derive(Clone, Copy, Default)]
struct ElementSources {
    entries: [Option<GuardedElementSource>; array_cache::ArrayPlan::MAX_LIVE_SOURCES],
}

impl ElementSources {
    fn clear(&mut self) {
        self.entries = Default::default();
    }

    fn find(&self, provenance: OptProvenance) -> Option<GuardedElementSource> {
        if !matches!(
            provenance,
            OptProvenance::Argument(_) | OptProvenance::Local(_)
        ) {
            return None;
        }
        self.entries
            .iter()
            .flatten()
            .find(|source| source.provenance == provenance)
            .copied()
    }

    fn insert(&mut self, source: GuardedElementSource) {
        if !matches!(
            source.provenance,
            OptProvenance::Argument(_) | OptProvenance::Local(_)
        ) {
            return;
        }
        if let Some(slot) = self
            .entries
            .iter_mut()
            .find(|entry| entry.is_some_and(|entry| entry.provenance == source.provenance))
        {
            *slot = Some(source);
        } else if let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) {
            *slot = Some(source);
        } else {
            // Evict the oldest tuple; dropping metadata is always sound.
            self.entries.rotate_left(1);
            self.entries[self.entries.len() - 1] = Some(source);
        }
    }

    /// The tuples live on entry to `block_pc`: those live at the end of
    /// every CFG predecessor. All predecessors must already be lowered (a
    /// loop header's backedge is not), so each tuple is valid on every
    /// incoming edge. Its Cranelift values are shared by all predecessors and
    /// therefore defined in a block dominating this join.
    fn at_block_entry(
        block_pc: u32,
        predecessors: Option<&Vec<u32>>,
        exits: &std::collections::BTreeMap<u32, ElementSources>,
    ) -> Self {
        let mut entry = Self::default();
        let Some(predecessors) = predecessors.filter(|incoming| !incoming.is_empty()) else {
            return entry;
        };
        let Some(states) = predecessors
            .iter()
            .map(|predecessor| exits.get(predecessor))
            .collect::<Option<Vec<_>>>()
        else {
            return entry;
        };
        let same = |left: &GuardedElementSource, right: &GuardedElementSource| {
            GuardedElementSource { block_pc, ..*left }
                == GuardedElementSource { block_pc, ..*right }
        };
        for source in states[0].entries.iter().flatten() {
            if states[1..].iter().all(|state| {
                state
                    .entries
                    .iter()
                    .flatten()
                    .any(|other| same(source, other))
            }) {
                entry.insert(GuardedElementSource {
                    block_pc,
                    ..*source
                });
            }
        }
        entry
    }
}

#[derive(Clone, Copy, Default)]
enum EntryRepresentation {
    #[default]
    Any,
    Bool,
    Numeric,
    Int32,
    Float64,
    HeapRef,
}

#[derive(Default)]
struct NumericSpecialization {
    key: Option<crate::runtime::FunctionKey>,
    entry: EntryRepresentation,
    arguments: Box<[EntryRepresentation]>,
    int_pcs: std::collections::BTreeSet<u32>,
    float_pcs: std::collections::BTreeSet<u32>,
    calls: std::collections::BTreeMap<u32, crate::runtime::CallSpecializationKey>,
    properties: std::collections::BTreeMap<u32, Box<[crate::runtime::ShapeObservation]>>,
    /// get_field/put_field sites whose feedback is megamorphic. They lower to
    /// the owning generic property helper instead of a shape guard, so a
    /// shape miss never becomes a deopt/recompile loop.
    generic_properties: std::collections::BTreeSet<u32>,
    arrays: Box<[crate::runtime::ArrayFeedbackSnapshot]>,
    direct_calls: std::collections::BTreeMap<u32, DirectCallSite>,
    native_calls: std::collections::BTreeMap<u32, NativeCallSite>,
    inline_callees: std::collections::BTreeMap<u32, crate::ir::InlineCallee>,
    frame_inline_callees: std::collections::BTreeMap<u32, crate::ir::FrameInlineCallee>,
    numeric_constants: std::collections::BTreeMap<u32, crate::ir::TaggedValue>,
}

#[derive(Clone)]
struct DirectCallSite {
    call: crate::runtime::CallSpecializationKey,
    entry: usize,
}

/// A monomorphic call site lowered through the P3a native-call convention.
#[derive(Clone)]
struct NativeCallSite {
    /// Global name whose binding `JS_JitNativeCallBegin` proves.
    atom: u32,
    arguments: Box<[crate::runtime::FeedbackRepresentation]>,
    result: crate::runtime::FeedbackRepresentation,
    callee_identity: u64,
    callee_bytecode_identity: u64,
    /// `None` names the native entry published in this same artifact.
    entry: Option<usize>,
}

impl NumericSpecialization {
    fn retain_frame_inline_target(
        target: &crate::runtime::FrameInlineTarget,
    ) -> Option<crate::ir::FrameInlineCallee> {
        let body = target
            .snapshot()
            .verify(crate::bytecode::VerifyLimits::default())
            .ok()?;
        let children = target
            .children()
            .iter()
            .filter_map(|child| {
                Self::retain_frame_inline_target(child).map(|callee| (child.pc(), callee))
            })
            .collect();
        Some(crate::ir::FrameInlineCallee {
            artifact: target.artifact_key(),
            target: *target.target(),
            body,
            children,
        })
    }

    fn retain_inline_callees(&mut self, request: &CompileRequest) {
        if request.side_path_profile().is_some() {
            return;
        }
        for target in request.direct_call_targets() {
            // A published native entry runs the callee's whole recursion
            // natively; inlining one level would call it generically.
            if request.native_call_target(target.pc()).is_some() {
                continue;
            }
            if let Some(body) = target.inline_snapshot().and_then(|snapshot| {
                snapshot
                    .verify(crate::bytecode::VerifyLimits::default())
                    .ok()
            }) {
                self.inline_callees.insert(
                    target.pc(),
                    crate::ir::InlineCallee {
                        artifact: target.artifact_key(),
                        call: target.call().clone(),
                        body,
                    },
                );
            }
        }
        for target in request.frame_inline_targets() {
            if request.native_call_target(target.pc()).is_some() {
                continue;
            }
            if let Some(callee) = Self::retain_frame_inline_target(target) {
                self.frame_inline_callees.insert(target.pc(), callee);
            }
        }
    }

    fn translate(
        &self,
        function: &VerifiedFunction,
        epoch: u64,
    ) -> Result<OptimizedIr, CompileFailure> {
        use crate::ir::ScalarNumericMode;
        let mut modes = self
            .int_pcs
            .iter()
            .map(|&pc| (pc, ScalarNumericMode::Int32))
            .collect::<std::collections::BTreeMap<_, _>>();
        modes.extend(
            self.float_pcs
                .iter()
                .map(|&pc| (pc, ScalarNumericMode::Float64)),
        );
        // A monomorphic packed/typed-array length candidate becomes Int32 only
        // on the guarded path. Packed guards require exact dense length;
        // typed guards require the intrinsic getter and fixed backing. Misses
        // deopt before consuming the assumption.
        modes.extend(self.arrays.iter().filter_map(|site| {
            let mut observed = site.modes();
            let mode = observed.next()?;
            (site.access() == crate::runtime::ArrayAccess::Length
                && site.can_specialize()
                && matches!(
                    mode,
                    crate::runtime::ArrayMode::Packed
                        | crate::runtime::ArrayMode::Int32
                        | crate::runtime::ArrayMode::Float64
                )
                && observed.next().is_none())
            .then_some((site.pc(), ScalarNumericMode::Int32))
        }));
        let has_guarded_array_length = self.arrays.iter().any(|site| {
            let mut observed = site.modes();
            site.access() == crate::runtime::ArrayAccess::Length
                && site.can_specialize()
                && matches!(
                    observed.next(),
                    Some(
                        crate::runtime::ArrayMode::Packed
                            | crate::runtime::ArrayMode::Int32
                            | crate::runtime::ArrayMode::Float64
                    )
                )
                && observed.next().is_none()
        });
        let cfg = function.control_flow_graph();
        if (matches!(self.entry, EntryRepresentation::Int32) || has_guarded_array_length)
            && self.calls.is_empty()
            && cfg
                .blocks()
                .iter()
                .any(|block| cfg.is_loop_header(block.start_pc()))
        {
            for instruction in function.instructions() {
                if matches!(
                    instruction.opcode().name(),
                    "inc" | "dec" | "post_inc" | "post_dec" | "inc_loc" | "dec_loc"
                ) {
                    modes.insert(instruction.pc(), ScalarNumericMode::Int32);
                }
            }
        }
        OptimizedIr::translate_with_frame_inline_callees(
            function,
            epoch,
            &modes,
            &self.inline_callees,
            &self.frame_inline_callees,
        )
    }

    fn frame_inline_dependencies(&self) -> Vec<crate::runtime::FunctionKey> {
        fn collect(
            callee: &crate::ir::FrameInlineCallee,
            dependencies: &mut std::collections::BTreeSet<crate::runtime::FunctionKey>,
        ) {
            dependencies.insert(callee.target.callee());
            for child in callee.children.values() {
                collect(child, dependencies);
            }
        }
        let mut dependencies = std::collections::BTreeSet::new();
        for callee in self.frame_inline_callees.values() {
            collect(callee, &mut dependencies);
        }
        dependencies.into_iter().collect()
    }

    fn from_feedback(
        function: &VerifiedFunction,
        key: crate::runtime::FunctionKey,
        feedback: &crate::runtime::FeedbackSnapshot,
    ) -> Self {
        use crate::runtime::{FeedbackRepresentation, FeedbackState, ObservedType};
        let arrays = feedback
            .arrays()
            .iter()
            .filter(|site| site.function() == key)
            .copied()
            .collect::<Vec<_>>()
            .into_boxed_slice();
        let calls = function
            .instructions()
            .iter()
            .filter(|instruction| is_call_site(instruction.opcode().name()))
            .filter_map(|instruction| {
                feedback
                    .call_specialization_at(key, instruction.pc())
                    .map(|call| (instruction.pc(), call))
            })
            .collect();
        let properties = function
            .instructions()
            .iter()
            .filter_map(|instruction| {
                let name = instruction.opcode().name();
                if !matches!(name, "get_field" | "put_field") {
                    return None;
                }
                let site = feedback.property_at(instruction.pc())?;
                let observations = site.observations();
                let safe = site.state() != crate::runtime::ShapeFeedbackState::Megamorphic
                    && !observations.is_empty()
                    && observations.len() <= crate::runtime::POLYMORPHIC_PROPERTY_LIMIT
                    && observations.iter().all(|observation| {
                        let primitive = matches!(
                            observation.value(),
                            ObservedType::Int32
                                | ObservedType::Float64
                                | ObservedType::Bool
                                | ObservedType::Null
                                | ObservedType::Undefined
                        );
                        let owned_object_load =
                            name == "get_field" && observation.value() == ObservedType::Object;
                        observation.prototype().identity() == 0
                            && observation.prototype().generation() == 0
                            && !observation
                                .attributes()
                                .contains(crate::runtime::PropertyAttributes::ACCESSOR)
                            && (name != "put_field"
                                || observation
                                    .attributes()
                                    .contains(crate::runtime::PropertyAttributes::WRITABLE))
                            && (primitive || owned_object_load)
                    });
                safe.then_some((instruction.pc(), observations.to_vec().into_boxed_slice()))
            })
            .collect();
        let generic_properties = function
            .instructions()
            .iter()
            .filter(|instruction| {
                matches!(instruction.opcode().name(), "get_field" | "put_field")
                    && feedback.property_at(instruction.pc()).is_some_and(|site| {
                        site.state() == crate::runtime::ShapeFeedbackState::Megamorphic
                    })
            })
            .map(|instruction| instruction.pc())
            .collect();
        let numeric_constants = function
            .snapshot()
            .constants()
            .iter()
            .filter_map(|constant| {
                matches!(
                    constant.tag(),
                    rquickjs_core::qjs::JS_TAG_INT | rquickjs_core::qjs::JS_TAG_FLOAT64
                )
                .then_some((
                    constant.index(),
                    crate::ir::TaggedValue::new(constant.payload(), i64::from(constant.tag())),
                ))
            })
            .collect();
        let argument_count = usize::from(function.snapshot().arg_count());
        let entry_arguments = feedback
            .call_argument_types(key)
            .filter(|arguments| arguments.len() == argument_count)
            .map(|arguments| {
                arguments
                    .iter()
                    .map(|argument| match argument {
                        ObservedType::Int32 => EntryRepresentation::Int32,
                        ObservedType::Float64 => EntryRepresentation::Float64,
                        ObservedType::Object => EntryRepresentation::HeapRef,
                        ObservedType::Bool => EntryRepresentation::Bool,
                        _ => EntryRepresentation::Any,
                    })
                    .collect::<Vec<_>>()
                    .into_boxed_slice()
            })
            .unwrap_or_default();
        // The checked Int32 arithmetic path guards both operand tags at
        // run time, so stable per-site Int32 feedback selects it whatever the
        // function's own signature is (a kernel may well return a string).
        let int_pcs = function
            .instructions()
            .iter()
            .filter(|instruction| {
                matches!(
                    instruction.opcode().name(),
                    "add" | "sub" | "mul" | "div" | "or" | "lt" | "lte" | "gt" | "gte"
                )
            })
            .filter_map(|instruction| {
                let site = feedback.binary_at(key, instruction.pc())?;
                let expected_result =
                    if matches!(instruction.opcode().name(), "lt" | "lte" | "gt" | "gte") {
                        ObservedType::Bool
                    } else {
                        ObservedType::Int32
                    };
                (site.state() == FeedbackState::Monomorphic
                    && site.lhs() == [ObservedType::Int32]
                    && site.rhs() == [ObservedType::Int32]
                    && site.result() == [expected_result])
                .then_some(instruction.pc())
            })
            .collect();
        let Some(signature) = feedback
            .bounded_specialization(key)
            .filter(|signature| signature.arity() == argument_count)
        else {
            return Self {
                key: Some(key),
                arguments: entry_arguments,
                int_pcs,
                calls,
                properties,
                generic_properties,
                arrays,
                numeric_constants,
                ..Self::default()
            };
        };
        let representation = signature.result();
        let observed = match representation {
            FeedbackRepresentation::Int32 => ObservedType::Int32,
            FeedbackRepresentation::Float64 => ObservedType::Float64,
            FeedbackRepresentation::Bool | FeedbackRepresentation::HeapRef => {
                return Self {
                    key: Some(key),
                    int_pcs,
                    calls,
                    properties,
                    generic_properties,
                    arrays,
                    numeric_constants,
                    ..Self::default()
                }
            }
        };
        let float_pcs = function
            .instructions()
            .iter()
            .filter(|instruction| {
                matches!(instruction.opcode().name(), "add" | "sub" | "mul" | "div")
            })
            .filter_map(|instruction| {
                let site = feedback.binary_at(key, instruction.pc())?;
                (representation == FeedbackRepresentation::Float64
                    && site.state() == FeedbackState::Monomorphic
                    && site.lhs() == [observed]
                    && site.rhs() == [observed]
                    && site.result() == [observed])
                .then_some(instruction.pc())
            })
            .collect();
        Self {
            key: Some(key),
            entry: if signature
                .arguments()
                .iter()
                .all(|argument| *argument == representation)
            {
                match representation {
                    FeedbackRepresentation::Int32 => EntryRepresentation::Int32,
                    FeedbackRepresentation::Float64 => EntryRepresentation::Float64,
                    FeedbackRepresentation::Bool | FeedbackRepresentation::HeapRef => {
                        EntryRepresentation::Any
                    }
                }
            } else {
                EntryRepresentation::Any
            },
            arguments: signature
                .arguments()
                .iter()
                .map(|argument| match argument {
                    FeedbackRepresentation::Int32 => EntryRepresentation::Int32,
                    FeedbackRepresentation::Float64 => EntryRepresentation::Float64,
                    FeedbackRepresentation::HeapRef => EntryRepresentation::HeapRef,
                    FeedbackRepresentation::Bool => EntryRepresentation::Bool,
                })
                .collect::<Vec<_>>()
                .into_boxed_slice(),
            int_pcs,
            float_pcs,
            calls,
            properties,
            generic_properties,
            arrays,
            direct_calls: Default::default(),
            native_calls: Default::default(),
            inline_callees: Default::default(),
            frame_inline_callees: Default::default(),
            numeric_constants,
        }
    }
}

/// Optimized element loads publish only primitives: a heap element
/// deoptimizes before it reaches the operand stack. A shape-guarded property
/// access whose receiver is exactly such an element load therefore deopts on
/// every execution (an object element exits at the load, a primitive one at
/// the receiver guard). Reject the function up front instead of installing an
/// artifact that can only bounce between tiers. Generic (megamorphic) sites
/// accept any receiver and remain admissible.
fn shape_guarded_element_receiver(
    ir: &OptimizedIr,
    specialization: &NumericSpecialization,
) -> bool {
    let graph = ir.scalar_graph();
    ir.nodes().iter().any(|node| {
        let base = match graph.heap_operation(node.id()) {
            Some(
                crate::ir::ScalarHeapOperation::GetProperty { base, .. }
                | crate::ir::ScalarHeapOperation::PutProperty { base, .. },
            ) => *base,
            _ => return false,
        };
        specialization.properties.contains_key(&node.pc())
            && !specialization.generic_properties.contains(&node.pc())
            && matches!(
                graph.values().get(base.index()),
                Some(crate::ir::ScalarValue::GetElement { .. })
            )
    })
}

fn lower_optimized_machine(
    isa: &cranelift_codegen::isa::OwnedTargetIsa,
    ir: &OptimizedIr,
    control: Option<&CompileControl>,
    side_path: Option<crate::runtime::SidePathProfile>,
    specialization: &NumericSpecialization,
) -> Result<super::baseline::RelocatableCode, CompileFailure> {
    use cranelift_codegen::ir::{
        types, AbiParam, ArgumentPurpose, Function, InstBuilder, MemFlags, Signature,
    };
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
    use rquickjs_core::qjs;
    let pointer_type = isa.pointer_type();
    if pointer_type.bytes() != 8 {
        return Err(CompileFailure::InvalidArtifact);
    }
    let layout = super::helpers::FrameLayout::validated(8)?;
    let abi = crate::abi::AbiInfo::linked().map_err(|_| CompileFailure::InvalidArtifact)?;
    let element_layout = abi.element_layout();
    let inline_api = abi.inline_api();
    let array_query = abi
        .array_api()
        .query
        .ok_or(CompileFailure::InvalidArtifact)? as usize;
    let Some(entry_site) = ir.guard_maps().first() else {
        return Err(CompileFailure::InvalidArtifact);
    };
    if shape_guarded_element_receiver(ir, specialization) {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    let shape = entry_site.shape();
    let int32_loop = matches!(specialization.entry, EntryRepresentation::Int32)
        && specialization.calls.is_empty()
        && ir.scalar_graph().frame_inlined_calls() == 0
        && ir.blocks().iter().any(|block| block.is_loop_header());
    // Tier 2 lowers `push_const*` only for numeric constant-pool entries, as
    // literal Int32/Float64 immediates, and otherwise rejects the function.
    let numeric_constant_mode = |id: u32| {
        let index = opt_push_const_index(ir.nodes().get(id as usize)?)?;
        let constant = specialization.numeric_constants.get(&index)?;
        match i32::try_from(constant.tag).ok()? {
            qjs::JS_TAG_INT => Some(crate::ir::ScalarNumericMode::Int32),
            qjs::JS_TAG_FLOAT64 => Some(crate::ir::ScalarNumericMode::Float64),
            _ => None,
        }
    };
    let scalar_numeric = if side_path.is_none() {
        ir.scalar_graph().proven_numeric_values_with_constants(
            ir.nodes(),
            |index| {
                usize::from(index) < usize::from(shape.arguments())
                    && matches!(
                        specialization
                            .arguments
                            .get(usize::from(index))
                            .copied()
                            .unwrap_or(specialization.entry),
                        EntryRepresentation::Int32
                    )
            },
            int32_loop,
            numeric_constant_mode,
            ir.scalar_graph().values().len().saturating_mul(128),
        )
    } else {
        Vec::new()
    };
    let initial_amortized_poll = int32_loop
        || (side_path.is_none()
            && ir
                .scalar_graph()
                .permits_amortized_poll(ir.nodes(), &scalar_numeric));
    if let Some(control) = control {
        control.check_ir_bytes(
            ir.scalar_graph()
                .allocated_bytes()
                .saturating_add(
                    scalar_numeric.len().saturating_mul(core::mem::size_of::<
                        Option<crate::ir::ScalarNumericMode>,
                    >()),
                )
                .saturating_add(call_guards::HoistedCallGuards::scratch_bytes(ir)),
        )?;
    }
    let hoisted_calls = call_guards::HoistedCallGuards::analyze(ir, initial_amortized_poll);
    if let Some(control) = control {
        control.check()?;
    }
    let property_work = ir
        .nodes()
        .len()
        .saturating_add(ir.scalar_graph().values().len())
        .saturating_mul(128);
    if let Some(control) = control {
        control.check_ir_bytes(
            ir.scalar_graph()
                .allocated_bytes()
                .saturating_add(call_guards::HoistedCallGuards::scratch_bytes(ir))
                .saturating_add(crate::ir::PropertyPlan::bytes_upper_bound(ir.nodes().len()))
                .saturating_add(
                    ir.scalar_graph()
                        .values()
                        .len()
                        .saturating_mul(crate::ir::KnownFacts::bytes_per_value()),
                ),
        )?;
    }
    let stack_slots = usize::from(ir.max_stack())
        .checked_add(crate::ir::MAX_HELPER_SCRATCH_SLOTS)
        .ok_or(CompileFailure::ResourceLimit)?;
    let array_work = ir
        .nodes()
        .len()
        .saturating_add(ir.scalar_graph().values().len())
        .saturating_mul(192);
    // Reserve the full set of simultaneously live analyses before building
    // provenance. Both local-release planning and lowering consume this map.
    let live_analysis_bytes = ir
        .scalar_graph()
        .allocated_bytes()
        .saturating_add(core::mem::size_of_val(scalar_numeric.as_slice()))
        .saturating_add(usize::from(shape.locals()) * core::mem::size_of::<bool>())
        .saturating_add(ir.nodes().len().saturating_mul(128))
        .saturating_add(call_guards::HoistedCallGuards::scratch_bytes(ir))
        .saturating_add(crate::ir::PropertyPlan::bytes_upper_bound(ir.nodes().len()))
        .saturating_add(array_cache::ArrayPlan::bytes_upper_bound(ir))
        .saturating_add(crate::ir::IntegerRangeAnalysis::bytes_upper_bound(
            array_work,
        ))
        .saturating_add(crate::ir::LoopAnalysis::bytes_upper_bound(ir, array_work))
        .saturating_add(
            ir.scalar_graph()
                .values()
                .len()
                .saturating_mul(crate::ir::KnownFacts::bytes_per_value())
                .saturating_mul(2),
        );
    let cfg_entry_provenance = opt_cfg_entry_provenance(
        ir,
        specialization,
        stack_slots,
        live_analysis_bytes,
        control,
    )?;
    let owned_locals = owned_local_targets(ir, specialization, &cfg_entry_provenance)?;
    // Seed loop-carried receiver Phi cycles as an assume/validate SCC. This is
    // the same speculative contract used by field promotion in production
    // JITs: feedback can seed identity propagation, but every assumed site
    // must subsequently receive the exact guarded, non-reentrant lowering.
    let assumed_property_sites = ir
        .nodes()
        .iter()
        .filter(|node| {
            matches!(
                ir.scalar_graph().heap_operation(node.id()),
                Some(
                    crate::ir::ScalarHeapOperation::GetProperty { .. }
                        | crate::ir::ScalarHeapOperation::PutProperty { .. }
                )
            ) && specialization
                .properties
                .get(&node.pc())
                .is_some_and(|observations| {
                    matches!(observations.as_ref(), [observation]
                        if crate::ir::eligible_property_observation(*observation))
                })
        })
        .map(|node| node.id())
        .collect::<std::collections::BTreeSet<_>>();
    let initial_property_facts = crate::ir::KnownFacts::analyze_with_preserved_frame_reads(
        ir.scalar_graph(),
        ir.nodes(),
        |id| {
            assumed_property_sites.contains(&id)
                || ir.nodes().get(id as usize).is_some_and(|node| {
                    matches!(
                        node.kind(),
                        crate::ir::OptimizedNodeKind::GuardNumeric { .. }
                    )
                })
        },
        property_work,
    );
    let property_plan = if int32_loop || side_path.is_some() {
        crate::ir::PropertyPlan::default()
    } else {
        let mut plan = crate::ir::PropertyPlan::analyze(
            ir,
            &initial_property_facts,
            &specialization.properties,
            |id| {
                // These local stores only move SSA values. An owned local would
                // require FREE (which is observable under stress GC), so it keeps
                // the normal materialization barrier. Alias guards use the common
                // full-frame deopt path before changing the local.
                match ir.scalar_graph().effect_for_node(id) {
                    crate::ir::ScalarHeapEffect::FrameWrite(crate::ir::FrameSlot::Local(index)) => {
                        owned_locals
                            .get(usize::from(index))
                            .is_some_and(|&owned| !owned)
                    }
                    _ => false,
                }
            },
            property_work,
        );
        if !assumed_property_sites
            .iter()
            .all(|&id| plan.access(id).is_some())
        {
            plan = crate::ir::PropertyPlan::default();
        }
        // Facts may cross a property operation only after a first, conservative
        // pass has proved that exact site has a guarded leaf-or-deopt lowering.
        // Feedback or the opcode alone is not a capability: an accessor/Proxy
        // fallback can reenter and mutate captured caller frame slots.
        // Grow the capability set to a bounded fixed point. A later repeated
        // access may inherit its receiver identity only after every preceding
        // access in the chain has itself become a guarded leaf.
        for _ in 0..32 {
            let guarded_property_sites = ir
                .nodes()
                .iter()
                .filter(|node| plan.access(node.id()).is_some())
                .map(|node| node.id())
                .collect::<std::collections::BTreeSet<_>>();
            let property_facts = crate::ir::KnownFacts::analyze_with_preserved_frame_reads(
                ir.scalar_graph(),
                ir.nodes(),
                |id| {
                    guarded_property_sites.contains(&id)
                        || ir.nodes().get(id as usize).is_some_and(|node| {
                            matches!(
                                node.kind(),
                                crate::ir::OptimizedNodeKind::GuardNumeric { .. }
                            )
                        })
                },
                property_work,
            );
            let next = crate::ir::PropertyPlan::analyze(
                ir,
                &property_facts,
                &specialization.properties,
                |id| match ir.scalar_graph().effect_for_node(id) {
                    crate::ir::ScalarHeapEffect::FrameWrite(crate::ir::FrameSlot::Local(index)) => {
                        owned_locals
                            .get(usize::from(index))
                            .is_some_and(|&owned| !owned)
                    }
                    _ => false,
                },
                property_work,
            );
            let next_sites = ir
                .nodes()
                .iter()
                .filter(|node| next.access(node.id()).is_some())
                .map(|node| node.id())
                .collect::<std::collections::BTreeSet<_>>();
            if !guarded_property_sites.is_subset(&next_sites) {
                // The next plan must retain every capability its facts relied
                // on. Conflicting feedback may otherwise make the refinement
                // non-monotone; discard the optimization rather than publish
                // a proof whose non-reentrant premise is no longer emitted.
                plan = crate::ir::PropertyPlan::default();
                break;
            }
            plan = next;
            if next_sites == guarded_property_sites {
                break;
            }
        }
        plan
    };
    // Guarded property leaves are non-reentrant: each either completes
    // without running JavaScript or deoptimizes from its exact pre-access
    // frame state. Frame slots read across them therefore keep their
    // pre-access identities, so loop-carried Int32 induction variables (for
    // example `i++` in a loop that updates object fields) no longer degrade to
    // mixed Int32/Float64 updates. Selected updates keep their operand check
    // and overflow deopt; the proof only removes redundant checks and the
    // Float64 fallback. The premise is the final plan: every preserved site
    // must be lowered through the planned guarded leaf or compilation fails.
    let property_specialized_ir;
    let property_numeric;
    let (ir, scalar_numeric) = if !int32_loop
        && side_path.is_none()
        && !property_plan.candidates().is_empty()
        && ir.blocks().iter().any(|block| block.is_loop_header())
    {
        let preserved = |id: u32| {
            property_plan.access(id).is_some()
                || ir.nodes().get(id as usize).is_some_and(|node| {
                    matches!(
                        node.kind(),
                        crate::ir::OptimizedNodeKind::GuardNumeric { .. }
                    )
                })
        };
        if let Some(control) = control {
            control.check_ir_bytes(
                ir.scalar_graph()
                    .allocated_bytes()
                    .saturating_mul(2)
                    .saturating_add(ir.nodes().len().saturating_mul(128)),
            )?;
        }
        let mut candidate = ir.clone();
        let work = ir.scalar_graph().values().len().saturating_mul(128);
        if candidate.specialize_integer_updates_with_preserved_frame_reads(preserved, work) != 0 {
            property_numeric = candidate
                .scalar_graph()
                .proven_numeric_values_with_preserved_frame_reads(
                    candidate.nodes(),
                    |index| {
                        usize::from(index) < usize::from(shape.arguments())
                            && matches!(
                                specialization
                                    .arguments
                                    .get(usize::from(index))
                                    .copied()
                                    .unwrap_or(specialization.entry),
                                EntryRepresentation::Int32
                            )
                    },
                    preserved,
                    work,
                );
            property_specialized_ir = candidate;
            (&property_specialized_ir, property_numeric)
        } else {
            (ir, scalar_numeric)
        }
    } else {
        (ir, scalar_numeric)
    };
    let array_site = |pc| {
        specialization
            .arrays
            .iter()
            .find(|site| site.pc() == pc)
            .copied()
    };
    let specialized_load = |pc| {
        let Some(site) = array_site(pc) else {
            return false;
        };
        let mut modes = site.modes();
        site.access() == crate::runtime::ArrayAccess::Load
            && site.can_specialize()
            && modes.next().is_some()
            && modes.next().is_none()
    };
    let specialized_length = |pc| {
        let Some(site) = array_site(pc) else {
            return false;
        };
        let mut modes = site.modes();
        site.access() == crate::runtime::ArrayAccess::Length
            && site.can_specialize()
            && matches!(
                modes.next(),
                Some(
                    crate::runtime::ArrayMode::Packed
                        | crate::runtime::ArrayMode::Int32
                        | crate::runtime::ArrayMode::Float64
                )
            )
            && modes.next().is_none()
    };
    let specialized_store = |pc| {
        let Some(site) = array_site(pc) else {
            return false;
        };
        let mut modes = site.modes();
        site.access() == crate::runtime::ArrayAccess::Store
            && site.can_specialize()
            && matches!(
                modes.next(),
                Some(crate::runtime::ArrayMode::Int32 | crate::runtime::ArrayMode::Float64)
            )
            && modes.next().is_none()
    };
    // This native guard only accepts already-canonical Int32/string/symbol
    // keys and otherwise exits before conversion. It never writes the frame.
    let guarded_propkey = |id: u32| {
        matches!(ir.nodes()[id as usize].kind(),
        crate::ir::OptimizedNodeKind::Bytecode { opcode } if opcode.as_ref() == "to_propkey")
    };
    // Lowering materializes only numeric constant-pool entries, as immediates
    // (any other constant fails compilation). Such a push cannot reenter.
    let numeric_constant_push = |id: u32| {
        ir.nodes()
            .get(id as usize)
            .and_then(opt_push_const_index)
            .is_some_and(|index| specialization.numeric_constants.contains_key(&index))
    };
    let array_loops = crate::ir::LoopAnalysis::analyze(ir, array_work);
    let assumed_array_sites = ir
        .nodes()
        .iter()
        .filter(|node| match node.kind() {
            crate::ir::OptimizedNodeKind::Bytecode { opcode } => {
                (opcode.as_ref() == "get_array_el" && specialized_load(node.pc()))
                    || (opcode.as_ref() == "get_length" && specialized_length(node.pc()))
                    || (opcode.as_ref() == "put_array_el" && specialized_store(node.pc()))
            }
            _ => false,
        })
        .map(|node| node.id())
        .collect::<std::collections::BTreeSet<_>>();
    let initial_array_facts = crate::ir::KnownFacts::analyze_with_preserved_frame_reads(
        ir.scalar_graph(),
        ir.nodes(),
        |id| {
            assumed_array_sites.contains(&id)
                || guarded_propkey(id)
                || numeric_constant_push(id)
                || ir.nodes().get(id as usize).is_some_and(|node| {
                    matches!(
                        node.kind(),
                        crate::ir::OptimizedNodeKind::GuardNumeric { .. }
                    )
                })
        },
        array_work,
    );
    if let Some(control) = control {
        control.check_ir_bytes(
            ir.scalar_graph()
                .allocated_bytes()
                .saturating_add(array_cache::ArrayPlan::bytes_upper_bound(ir))
                .saturating_add(crate::ir::IntegerRangeAnalysis::bytes_upper_bound(
                    array_work,
                ))
                .saturating_add(crate::ir::LoopAnalysis::bytes_upper_bound(ir, array_work))
                .saturating_add(
                    ir.scalar_graph()
                        .values()
                        .len()
                        .saturating_mul(crate::ir::KnownFacts::bytes_per_value()),
                ),
        )?;
    }
    let array_non_reentrant = |id, guarded_sites: &std::collections::BTreeSet<u32>| {
        let Some(node) = ir.nodes().get(id as usize) else {
            return false;
        };
        match ir.scalar_graph().effect_for_node(id) {
            crate::ir::ScalarHeapEffect::Pure => true,
            crate::ir::ScalarHeapEffect::FrameWrite(crate::ir::FrameSlot::Local(index)) => {
                owned_locals
                    .get(usize::from(index))
                    .is_some_and(|&owned| !owned)
            }
            crate::ir::ScalarHeapEffect::Reentrant => match node.kind() {
                crate::ir::OptimizedNodeKind::Bytecode { opcode } => {
                    (matches!(opcode.as_ref(), "get_array_el" | "put_array_el")
                        && guarded_sites.contains(&id))
                        || guarded_propkey(id)
                        || numeric_constant_push(id)
                        || (matches!(
                            opcode.as_ref(),
                            "add"
                                | "sub"
                                | "mul"
                                | "div"
                                | "or"
                                | "and"
                                | "xor"
                                | "shl"
                                | "sar"
                                | "shr"
                        ) && ir.scalar_graph().binary_operation(id).is_some())
                        || (matches!(
                            opcode.as_ref(),
                            "or" | "and" | "xor" | "shl" | "sar" | "shr"
                        ) && ir.scalar_graph().bitwise_operation(id).is_some())
                        || (matches!(opcode.as_ref(), "lt" | "lte" | "gt" | "gte")
                            && ir.scalar_graph().comparison(id).is_some())
                }
                _ => false,
            },
            _ => false,
        }
    };
    let array_plan = if int32_loop || side_path.is_some() || specialization.arrays.is_empty() {
        array_cache::ArrayPlan::default()
    } else {
        let mut plan = array_cache::ArrayPlan::analyze(
            ir,
            &initial_array_facts,
            &array_loops,
            specialization.arrays.len(),
            array_site,
            // The versioned leaf query guards the exact intrinsic getter and
            // fixed backing; feedback only selects the candidate.
            true,
            |id| array_non_reentrant(id, &assumed_array_sites),
            array_work,
        );
        if !assumed_array_sites.iter().all(|&id| plan.guarded_leaf(id)) {
            plan = array_cache::ArrayPlan::default();
        }
        for _ in 0..32 {
            let guarded_sites = ir
                .nodes()
                .iter()
                .filter(|node| plan.guarded_leaf(node.id()))
                .map(|node| node.id())
                .collect::<std::collections::BTreeSet<_>>();
            if guarded_sites.is_empty() {
                break;
            }
            let facts = crate::ir::KnownFacts::analyze_with_preserved_frame_reads(
                ir.scalar_graph(),
                ir.nodes(),
                |id| {
                    guarded_sites.contains(&id)
                        || guarded_propkey(id)
                        || numeric_constant_push(id)
                        || ir.nodes().get(id as usize).is_some_and(|node| {
                            matches!(
                                node.kind(),
                                crate::ir::OptimizedNodeKind::GuardNumeric { .. }
                            )
                        })
                },
                array_work,
            );
            let next = array_cache::ArrayPlan::analyze(
                ir,
                &facts,
                &array_loops,
                specialization.arrays.len(),
                array_site,
                true,
                |id| array_non_reentrant(id, &guarded_sites),
                array_work,
            );
            let next_sites = ir
                .nodes()
                .iter()
                .filter(|node| next.guarded_leaf(node.id()))
                .map(|node| node.id())
                .collect::<std::collections::BTreeSet<_>>();
            if !guarded_sites.is_subset(&next_sites) {
                plan = array_cache::ArrayPlan::default();
                break;
            }
            plan = next;
            if next_sites == guarded_sites {
                break;
            }
        }
        plan
    };
    // Amortization skips the per-iteration loop-header representation guard,
    // so frame writes must normally be numeric-proven. That guard never checks
    // an owned (helper-derived heap) local: it accepts any representation and
    // every consumer keeps its own checks. A store into an owned local outside
    // every natural loop (for example `const ints = buffers.ints` before a
    // traversal) therefore neither feeds a guarded loop Phi nor runs between
    // two header polls; an in-loop write to it is still rejected below.
    let local_store_outside_loops = |id: u32| {
        array_loops.is_complete()
            && matches!(
                ir.scalar_graph().effect_for_node(id),
                crate::ir::ScalarHeapEffect::FrameWrite(crate::ir::FrameSlot::Local(local))
                    if owned_locals.get(usize::from(local)).copied().unwrap_or(false)
            )
            && array_loops.block_for_node(id).is_some_and(|block| {
                !array_loops
                    .loops()
                    .iter()
                    .any(|natural_loop| natural_loop.contains_block(block))
            })
    };
    // Hoisted argument receivers whose slot no node of the loop rebinds. Only
    // these may have their object tag restated at the loop header. Only
    // length hoists qualify: they side-exit, so the preheader proved the
    // receiver is an object; a storage-only hoist never side-exits.
    let stable_hoisted_receivers = array_plan
        .hoists()
        .iter()
        .filter_map(|hoist| {
            let array_cache::ArrayReceiver::Argument(argument) =
                array_plan.candidates().get(hoist.candidate)?.receiver
            else {
                return None;
            };
            let natural_loop = array_loops
                .loops()
                .iter()
                .find(|natural_loop| natural_loop.header() == hoist.header)?;
            let rebinds = ir.nodes().iter().any(|node| {
                if !array_loops
                    .block_for_node(node.id())
                    .is_some_and(|block| natural_loop.contains_block(block))
                {
                    return false;
                }
                let slot = crate::ir::FrameSlot::Argument(argument);
                ir.scalar_graph().effect_for_node(node.id())
                    == crate::ir::ScalarHeapEffect::FrameWrite(slot)
                    || ir
                        .scalar_graph()
                        .frame_definitions_for_node(node.id())
                        .iter()
                        .any(|(defined, _)| *defined == slot)
                    || matches!(node.kind(), crate::ir::OptimizedNodeKind::Bytecode { opcode }
                        if opcode.starts_with("put_arg") || opcode.starts_with("set_arg"))
            });
            (!rebinds).then_some((hoist.header, usize::from(argument)))
        })
        .collect::<std::collections::BTreeSet<_>>();
    // Array feedback only proposes candidates. Poll amortization is enabled
    // after the concrete plan has proved that every heap site on the native
    // path is a guarded leaf-or-exit operation.
    // The certified array, property key and constant sites, and primitive
    // get_field sites (shape-guarded leaf-or-exit accesses; a missing guard
    // fails compilation) run no script on their continuing path.
    // Object-observed get_field sites are different: they lower through the
    // generic GET_PROPERTY bridge, which can run a getter. Script can only
    // write this frame's arguments or locals through a closure, a mapped
    // `arguments` object or direct eval, so those sites preserve the frame
    // only while the function contains none of the opcodes that create them
    // (Tier 1 policy already rejects them; this is checked again here).
    // An argument or local read after a frame-preserving site keeps its
    // entry-guarded representation. This proof only decides poll
    // amortization; lowering keeps using `scalar_numeric`.
    let script_cannot_write_frame = !ir.nodes().iter().any(|node| {
        matches!(node.kind(), crate::ir::OptimizedNodeKind::Bytecode { opcode }
        if matches!(
            opcode.as_ref(),
            "fclosure" | "fclosure8" | "special_object" | "eval" | "apply_eval"
        ))
    });
    let leaf_frame_preserving = |id: u32| {
        array_plan.guarded_leaf(id)
            || property_plan.access(id).is_some()
            || guarded_propkey(id)
            || numeric_constant_push(id)
            || ir.nodes().get(id as usize).is_some_and(|node| {
                (int32_loop
                    && matches!(
                        node.kind(),
                        crate::ir::OptimizedNodeKind::GuardNumeric { .. }
                    ))
                    || (matches!(node.kind(), crate::ir::OptimizedNodeKind::Bytecode { opcode }
                        if opcode.as_ref() == "get_field")
                        && specialization
                            .properties
                            .get(&node.pc())
                            .is_some_and(|observations| {
                                script_cannot_write_frame
                                    || observations.iter().all(|observation| {
                                        observation.value() != crate::runtime::ObservedType::Object
                                    })
                            }))
            })
    };
    let amortized_numeric = if side_path.is_none() && !int32_loop {
        // Seeded with the same certified numeric literals as `scalar_numeric`
        // so this proof stays a superset of the lowering proof.
        ir.scalar_graph()
            .proven_numeric_values_with_preserved_frame_reads_and_constants(
                ir.nodes(),
                |index| {
                    usize::from(index) < usize::from(shape.arguments())
                        && matches!(
                            specialization
                                .arguments
                                .get(usize::from(index))
                                .copied()
                                .unwrap_or(specialization.entry),
                            EntryRepresentation::Int32
                        )
                },
                leaf_frame_preserving,
                numeric_constant_mode,
                ir.scalar_graph().values().len().saturating_mul(128),
            )
    } else {
        Vec::new()
    };
    let guarded_heap = |id: u32| {
        if array_plan.guarded_leaf(id)
            || property_plan.access(id).is_some()
            || numeric_constant_push(id)
            || guarded_propkey(id)
            || local_store_outside_loops(id)
        {
            return true;
        }
        let Some(node) = ir.nodes().get(id as usize) else {
            return false;
        };
        let crate::ir::OptimizedNodeKind::Bytecode { opcode } = node.kind() else {
            return false;
        };
        if matches!(opcode.as_ref(), "get_field" | "put_field")
            && specialization.properties.contains_key(&node.pc())
        {
            return true;
        }
        (matches!(
            opcode.as_ref(),
            "add" | "sub" | "mul" | "div" | "or" | "and" | "xor" | "shl" | "sar" | "shr"
        ) && ir.scalar_graph().binary_operation(id).is_some())
            || (matches!(
                opcode.as_ref(),
                "or" | "and" | "xor" | "shl" | "sar" | "shr"
            ) && ir.scalar_graph().bitwise_operation(id).is_some())
            || (matches!(opcode.as_ref(), "lt" | "lte" | "gt" | "gte")
                && ir.scalar_graph().comparison(id).is_some())
    };
    let amortized_poll = int32_loop
        || (side_path.is_none()
            && ir.scalar_graph().permits_amortized_poll_with_guarded_heap(
                ir.nodes(),
                &amortized_numeric,
                guarded_heap,
            ));
    // A function whose other code (for example a call before or after the
    // loops) rules out whole-function amortization can still count down the
    // poll of an individual loop whose own nodes pass the same proof. Unlike
    // the whole-function mode, frame locals keep their immediate stores and
    // the header's numeric guard, hoisted-call and property revalidation all
    // still run on every iteration; only the runtime poll call is amortized,
    // exactly as it already is for raw Int32 loops.
    let amortized_loop_headers =
        if amortized_poll || side_path.is_some() || !array_loops.is_complete() {
            std::collections::BTreeSet::new()
        } else {
            array_loops
                .loops()
                .iter()
                .filter(|loop_| {
                    ir.scalar_graph().permits_amortized_poll_for(
                        ir.blocks()
                            .iter()
                            .filter(|block| loop_.contains_block(block.start_pc()))
                            .flat_map(|block| block.nodes().iter())
                            .filter_map(|&id| ir.nodes().get(id as usize)),
                        &amortized_numeric,
                        guarded_heap,
                    )
                })
                .map(|loop_| loop_.header())
                .collect::<std::collections::BTreeSet<_>>()
        };
    let loop_forwarded_property_sites = ir
        .nodes()
        .iter()
        .filter_map(|node| {
            let access = property_plan.access(node.id())?;
            let pc = array_loops.block_for_node(node.id())?;
            let loop_ = array_loops
                .loops()
                .iter()
                .find(|loop_| loop_.contains_block(pc))?;
            let loop_preserves_candidate = ir
                .blocks()
                .iter()
                .filter(|block| loop_.contains_block(block.start_pc()))
                .flat_map(|block| block.nodes().iter().copied())
                .all(|id| {
                    (!matches!(
                        property_plan.before_node(id),
                        crate::ir::PropertyBoundary::FlushInvalidate
                    ) || (amortized_poll
                        && matches!(
                            ir.nodes()[id as usize].kind(),
                            crate::ir::OptimizedNodeKind::GuardNumeric { mid_loop: true, .. }
                        )))
                        && property_plan
                            .access(id)
                            .is_none_or(|other| !other.aliases.contains(&access.candidate))
                });
            if !loop_preserves_candidate {
                return None;
            }
            ir.nodes()
                .iter()
                .filter_map(|seed| property_plan.access(seed.id()).map(|access| (seed, access)))
                .any(|(seed, seed_access)| {
                    seed_access.candidate == access.candidate
                        && seed.id() < node.id()
                        && array_loops.node_dominates(seed.id(), node.id())
                        && ir.nodes()[seed.id() as usize + 1..node.id() as usize]
                            .iter()
                            .all(|between| {
                                (!matches!(
                                    property_plan.before_node(between.id()),
                                    crate::ir::PropertyBoundary::FlushInvalidate
                                ) || (amortized_poll
                                    && matches!(
                                        between.kind(),
                                        crate::ir::OptimizedNodeKind::GuardNumeric {
                                            mid_loop: true,
                                            ..
                                        }
                                    )))
                                    && property_plan.access(between.id()).is_none_or(|other| {
                                        !other.aliases.contains(&access.candidate)
                                    })
                            })
                })
                .then_some(node.id())
        })
        .collect::<std::collections::BTreeSet<_>>();

    let mut signature = Signature::new(isa.default_call_conv());
    signature.params.push(AbiParam::special(
        pointer_type,
        ArgumentPurpose::StructReturn,
    ));
    signature.params.push(AbiParam::new(pointer_type));
    let mut clif = Function::with_name_signature(Default::default(), signature);
    let mut context = FunctionBuilderContext::new();
    let frame_buffers;
    {
        let mut builder = FunctionBuilder::new(&mut clif, &mut context);
        let generated_signatures = super::helpers::generated_signatures(&**isa)?;
        let poll_signature = generated_signatures
            .get(rquickjs_core::qjs::JSJitHelperId_JS_JIT_HELPER_POLL as usize)
            .cloned()
            .ok_or(CompileFailure::InvalidArtifact)?;
        let poll_signature = builder.import_signature(poll_signature);
        let helper_signatures = generated_signatures
            .into_iter()
            .map(|signature| builder.import_signature(signature))
            .collect::<Vec<_>>();
        let prologue = builder.create_block();
        builder.append_block_params_for_function_params(prologue);
        builder.switch_to_block(prologue);
        let params = builder.block_params(prologue);
        let sret = params[0];
        let frame = params[1];
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
        frame_buffers = FrameBufferLoads {
            sret,
            frame,
            loads: [
                (arg_buf, layout.arg_buf),
                (var_buf, layout.var_buf),
                (stack_base, layout.stack_base),
            ],
        };
        let mut next_var = 0u32;
        let mut alloc = || {
            let pair = OptVars {
                payload: Variable::from_u32(next_var),
                tag: Variable::from_u32(next_var + 1),
            };
            next_var += 2;
            pair
        };
        let arguments = (0..shape.arguments()).map(|_| alloc()).collect::<Vec<_>>();
        let locals = (0..shape.locals()).map(|_| alloc()).collect::<Vec<_>>();
        let stack = (0..stack_slots).map(|_| alloc()).collect::<Vec<_>>();
        let phi_vars = ir
            .blocks()
            .iter()
            .flat_map(|block| {
                ir.scalar_graph()
                    .inputs_for_block(block.start_pc())
                    .iter()
                    .map(|&(_, value)| value)
            })
            .map(|value| (value, alloc()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut stack_provenance = vec![OptProvenance::Unknown; stack_slots];
        let mut guarded_element_source: ElementSources;
        // Tuples live at the end of each lowered block, for forward joins.
        let mut block_exit_sources = std::collections::BTreeMap::<u32, ElementSources>::new();
        let mut hoisted_element_sources =
            std::collections::BTreeMap::<u32, Vec<GuardedElementSource>>::new();
        // The scalar-region proof excludes heap effects and requires every
        // local definition to be primitive (or a lexical sentinel). No local
        // owns a heap value, so poll/deopt boundaries can publish SSA locals
        // instead of storing them on every iteration. Arguments stay rooted
        // in their existing buffers and retain immediate writes.
        let defer_scalar_locals =
            amortized_poll && !int32_loop && !owned_locals.iter().any(|&owned| owned);

        let bounded_increments = provably_bounded_increments(ir);
        let payload_type = if int32_loop { types::I32 } else { types::I64 };
        let property_cache = property_cache::PropertyCache::new(
            &mut builder,
            &property_plan,
            &mut next_var,
            pointer_type,
        )?;
        let call_guard_state = hoisted_calls.create_state(&mut builder, &mut next_var);
        let env = OptEnv {
            frame,
            sret,
            arg_buf,
            var_buf,
            stack_base,
            pointer_type,
            payload_type,
            layout,
            int32_loop,
            arguments: &arguments,
            locals: &locals,
            stack: &stack,
            helper_signatures: &helper_signatures,
            property_cache: &property_cache,
        };
        for vars in arguments
            .iter()
            .chain(&locals)
            .chain(&stack)
            .chain(phi_vars.values())
        {
            builder.declare_var(vars.payload, payload_type);
            builder.declare_var(vars.tag, types::I64);
        }
        // The amortized poll countdown is an SSA variable so the register
        // allocator can keep it in a register across the loop instead of a
        // load/decrement/store through a stack slot on every iteration.
        let poll_budget = Variable::from_u32(next_var);
        builder.declare_var(poll_budget, types::I32);
        let initial_poll_budget = builder.ins().iconst(types::I32, 64);
        builder.def_var(poll_budget, initial_poll_budget);
        for (index, vars) in arguments.iter().enumerate() {
            let mut pair = opt_load(&mut builder, arg_buf, index);
            if int32_loop {
                pair.payload = builder.ins().ireduce(types::I32, pair.payload);
            }
            opt_define(&mut builder, *vars, pair);
        }
        for (index, vars) in locals.iter().enumerate() {
            let pair = if int32_loop {
                OptPair {
                    payload: builder.ins().iconst(types::I32, 0),
                    tag: builder
                        .ins()
                        .iconst(types::I64, i64::from(qjs::JS_TAG_UNDEFINED)),
                }
            } else {
                opt_load(&mut builder, var_buf, index)
            };
            opt_define(&mut builder, *vars, pair);
        }
        let undefined = OptPair {
            payload: builder.ins().iconst(payload_type, 0),
            tag: builder
                .ins()
                .iconst(types::I64, i64::from(qjs::JS_TAG_UNDEFINED)),
        };
        for vars in &stack {
            opt_define(&mut builder, *vars, undefined);
        }
        hoisted_calls.probe_entry(&call_guard_state, &mut builder, &env)?;
        let blocks = ir
            .blocks()
            .iter()
            .map(|block| (block.start_pc(), builder.create_block()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let mut predecessors = std::collections::BTreeMap::<u32, Vec<u32>>::new();
        for block in ir.blocks() {
            for successor in block.successors() {
                predecessors
                    .entry(*successor)
                    .or_default()
                    .push(block.start_pc());
            }
        }
        for &(_, value) in ir.scalar_graph().inputs_for_block(0) {
            let initial = match &ir.scalar_graph().values()[value.index()] {
                crate::ir::ScalarValue::Phi { inputs, .. } => {
                    inputs
                        .iter()
                        .find(|input| input.predecessor.is_none())
                        .ok_or(CompileFailure::InvalidArtifact)?
                        .value
                }
                _ => value,
            };
            let crate::ir::ScalarValue::Input { block_pc: 0, slot } =
                ir.scalar_graph().values()[initial.index()]
            else {
                return Err(CompileFailure::InvalidArtifact);
            };
            let variables = match slot {
                crate::ir::FrameSlot::Argument(index) => arguments[usize::from(index)],
                crate::ir::FrameSlot::Local(index) => locals[usize::from(index)],
                crate::ir::FrameSlot::Stack(_) => return Err(CompileFailure::InvalidArtifact),
            };
            let pair = opt_use(&mut builder, variables);
            opt_define(&mut builder, phi_vars[&value], pair);
        }
        let entry = *blocks.get(&0).ok_or(CompileFailure::InvalidArtifact)?;
        emit_opt_numeric_guard(
            &mut builder,
            frame,
            sret,
            &arguments,
            &[],
            pointer_type,
            layout,
            entry_site.guard(),
            0,
            entry,
            side_path.filter(|profile| profile.guard().get() == entry_site.guard()),
            specialization.entry,
            &specialization.arguments,
            &[],
        );
        for block in ir.blocks() {
            guarded_element_source = ElementSources::at_block_entry(
                block.start_pc(),
                predecessors.get(&block.start_pc()),
                &block_exit_sources,
            );
            for source in hoisted_element_sources
                .get(&block.start_pc())
                .into_iter()
                .flatten()
            {
                guarded_element_source.insert(*source);
            }
            let hoisted_receivers = hoisted_element_sources
                .get(&block.start_pc())
                .into_iter()
                .flatten()
                // An unguarded hoist never proved its receiver is an object.
                .filter(|source| !source.unguarded)
                .filter_map(|source| match source.provenance {
                    OptProvenance::Argument(argument)
                        if stable_hoisted_receivers.contains(&(block.start_pc(), argument)) =>
                    {
                        Some(argument)
                    }
                    _ => None,
                })
                .collect::<Vec<_>>();
            let clif_block = blocks[&block.start_pc()];
            builder.switch_to_block(clif_block);
            // IR blocks are lowered in bytecode order and are entered only by
            // IR edges (the function entry edge targets pc 0 and is emitted
            // before this loop). When every predecessor precedes this block,
            // no later edge can target it, so it can be sealed now. Loop
            // headers keep their backedges open until `seal_all_blocks`.
            if predecessors.get(&block.start_pc()).is_none_or(|incoming| {
                incoming
                    .iter()
                    .all(|&predecessor| predecessor < block.start_pc())
            }) {
                builder.seal_block(clif_block);
            }
            // Each such hoisted receiver is an argument slot no loop node
            // rebinds, and the dominating preheader guard proved it holds an
            // object. Its variable therefore holds that object on every edge
            // into the header.
            for &argument in &hoisted_receivers {
                let vars = *arguments
                    .get(argument)
                    .ok_or(CompileFailure::InvalidArtifact)?;
                let tag = builder
                    .ins()
                    .iconst(types::I64, i64::from(qjs::JS_TAG_OBJECT));
                builder.def_var(vars.tag, tag);
            }
            let mut depth = usize::from(block.stack_depth());
            stack_provenance.fill(OptProvenance::Unknown);
            let entry_provenance = cfg_entry_provenance
                .get(&block.start_pc())
                .ok_or(CompileFailure::InvalidArtifact)?;
            stack_provenance[..depth].copy_from_slice(&entry_provenance[..depth]);
            let mut terminated = false;
            let mut reusable_values = std::collections::BTreeMap::<u32, OptPair>::new();
            let mut scalar_values = std::collections::BTreeMap::new();
            for &(slot, value) in ir.scalar_graph().inputs_for_block(block.start_pc()) {
                let variables = match slot {
                    crate::ir::FrameSlot::Argument(index) => arguments[usize::from(index)],
                    crate::ir::FrameSlot::Local(index) => locals[usize::from(index)],
                    crate::ir::FrameSlot::Stack(index) => stack[usize::from(index)],
                };
                let mut pair = opt_use(&mut builder, phi_vars[&value]);
                if scalar_numeric.get(value.index()).copied().flatten()
                    == Some(crate::ir::ScalarNumericMode::Int32)
                {
                    // This proof includes every loop entry and backedge. The
                    // entry guard still checks the original argument tags.
                    pair.tag = builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT));
                }
                opt_define(&mut builder, variables, pair);
                scalar_values.insert(value, pair);
            }
            let mut previous_node = None;
            for node_id in block.nodes() {
                // Register the preceding node at the next instruction boundary;
                // specialized lowering may finish a node with `continue`.
                if let Some(previous) = previous_node {
                    opt_bind_scalar_node(
                        &mut builder,
                        ir,
                        previous,
                        depth,
                        &env,
                        &mut scalar_values,
                    )?;
                }
                previous_node = Some(*node_id);
                let node = ir
                    .nodes()
                    .get(*node_id as usize)
                    .ok_or(CompileFailure::InvalidArtifact)?;
                if node.eliminated() {
                    if node.pops() == 1
                        && node.pushes() == 0
                        && depth
                            .checked_sub(1)
                            .is_some_and(|top| stack_provenance[top] == OptProvenance::OwnedSlot)
                    {
                        property_cache.flush(&mut builder);
                        property_cache.invalidate(&mut builder);
                        emit_opt_free_stack_slot(&mut builder, &env, depth - 1)?;
                    }
                    depth = depth
                        .checked_sub(usize::from(node.pops()))
                        .and_then(|value| value.checked_add(usize::from(node.pushes())))
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    continue;
                }
                // An amortized poll is observable only when its countdown
                // expires.  Publishing here would flush/invalidate virtual
                // fields on every loop iteration and defeat write sinking.
                // Its cold poll block performs this boundary immediately
                // before calling into the runtime instead.
                let deferred_property_poll = matches!(
                    node.kind(),
                    crate::ir::OptimizedNodeKind::GuardNumeric { mid_loop: true, .. }
                ) && amortized_poll;
                match property_plan.before_node(node.id()) {
                    _ if deferred_property_poll => {}
                    crate::ir::PropertyBoundary::None => {}
                    crate::ir::PropertyBoundary::Flush => property_cache.flush(&mut builder),
                    crate::ir::PropertyBoundary::FlushInvalidate => {
                        property_cache.flush(&mut builder);
                        property_cache.invalidate(&mut builder);
                    }
                }
                let retains_hoisted_metadata = matches!(
                    node.kind(),
                    crate::ir::OptimizedNodeKind::GuardNumeric { mid_loop: true, .. }
                ) && hoisted_element_sources
                    .contains_key(&block.start_pc());
                if !array_plan.is_empty() && array_plan.invalidates_before(node.id()) {
                    guarded_element_source.clear();
                    if retains_hoisted_metadata {
                        // The poll revalidates exactly the hoisted tuples.
                        for source in hoisted_element_sources
                            .get(&block.start_pc())
                            .into_iter()
                            .flatten()
                        {
                            guarded_element_source.insert(*source);
                        }
                    }
                }
                if matches!(node.kind(), crate::ir::OptimizedNodeKind::Bytecode { opcode }
                    if matches!(opcode.as_ref(), "if_false8" | "if_true8" | "if_false" | "if_true" | "goto" | "goto8" | "goto16"))
                {
                    opt_define_scalar_edges(&mut builder, ir, block, &scalar_values, &phi_vars)?;
                }
                match node.kind() {
                    crate::ir::OptimizedNodeKind::GuardNumeric { guard, mid_loop } => {
                        if *mid_loop && depth != 0 {
                            // The poll's numeric-guard exit republishes an
                            // empty operand stack only.
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        // A poll that reaches the runtime can run the interrupt
                        // handler, which may detach or replace storage of any
                        // enclosing loop's hoisted receiver. Revalidate every
                        // enclosing header's tuples, not only this block's:
                        // an outer header re-seeds its tuples on each backedge.
                        let poll_revalidations = if *mid_loop {
                            opt_poll_revalidations(
                                &array_plan,
                                node.id(),
                                &hoisted_element_sources,
                            )?
                        } else {
                            Vec::new()
                        };
                        if *mid_loop && amortized_poll {
                            // The amortized poll's cold path never redefines
                            // a frame variable (it only publishes, polls and
                            // revalidates heap metadata), so each variable
                            // leaves the join holding its pre-poll value.
                            // Re-state literal tags after the join; otherwise
                            // they hide behind a merge parameter and every
                            // later representation check stays in the loop.
                            let literal_tags = arguments
                                .iter()
                                .chain(&locals)
                                .chain(stack.iter().take(depth))
                                .filter_map(|vars| {
                                    let tag = builder.use_var(vars.tag);
                                    opt_known_const(&builder, tag).map(|known| (*vars, known))
                                })
                                .collect::<Vec<_>>();
                            emit_opt_amortized_poll(
                                &mut builder,
                                frame,
                                sret,
                                poll_signature,
                                pointer_type,
                                layout,
                                node.pc(),
                                poll_budget,
                                !int32_loop,
                                |builder| {
                                    property_cache.flush(builder);
                                    property_cache.invalidate(builder);
                                    if defer_scalar_locals {
                                        for (index, vars) in locals.iter().enumerate() {
                                            let value = opt_use(builder, *vars);
                                            opt_store(builder, var_buf, index, value);
                                        }
                                    }
                                },
                                |builder| {
                                    if !int32_loop {
                                        let pass = builder.create_block();
                                        emit_opt_numeric_guard(
                                            builder,
                                            frame,
                                            sret,
                                            &arguments,
                                            &locals,
                                            pointer_type,
                                            layout,
                                            *guard,
                                            node.pc(),
                                            pass,
                                            None,
                                            EntryRepresentation::Numeric,
                                            &specialization.arguments,
                                            &owned_locals,
                                        );
                                        builder.switch_to_block(pass);
                                    }
                                    hoisted_calls.emit(
                                        &call_guard_state,
                                        builder,
                                        &env,
                                        node.pc(),
                                        *guard,
                                    )?;
                                    for source in &poll_revalidations {
                                        emit_opt_packed_loop_revalidate(
                                            builder,
                                            &env,
                                            &stack_provenance,
                                            depth,
                                            node.pc(),
                                            *guard,
                                            *source,
                                            element_layout,
                                            array_query,
                                        )?;
                                    }
                                    property_cache.revalidate_after_poll(
                                        builder,
                                        &env,
                                        &stack_provenance,
                                        depth,
                                        node.pc(),
                                        *guard,
                                    )?;
                                    Ok(())
                                },
                            )?;
                            for (vars, known) in literal_tags {
                                let tag = builder.ins().iconst(types::I64, known);
                                builder.def_var(vars.tag, tag);
                            }
                        } else if *mid_loop {
                            if amortized_loop_headers.contains(&block.start_pc()) {
                                emit_opt_countdown_poll(
                                    &mut builder,
                                    frame,
                                    sret,
                                    poll_signature,
                                    pointer_type,
                                    layout,
                                    node.pc(),
                                    poll_budget,
                                );
                            } else {
                                emit_opt_poll(
                                    &mut builder,
                                    frame,
                                    sret,
                                    poll_signature,
                                    pointer_type,
                                    layout,
                                    node.pc(),
                                );
                            }
                            let pass = builder.create_block();
                            emit_opt_numeric_guard(
                                &mut builder,
                                frame,
                                sret,
                                &arguments,
                                &locals,
                                pointer_type,
                                layout,
                                *guard,
                                node.pc(),
                                pass,
                                side_path.filter(|profile| profile.guard().get() == *guard),
                                EntryRepresentation::Numeric,
                                &specialization.arguments,
                                &owned_locals,
                            );
                            builder.switch_to_block(pass);
                            // Regular polls are just as observable as the
                            // amortized cold edge. A typed store can keep its
                            // numeric locals unproven, selecting this path;
                            // cached pointers/length must still be revalidated.
                            for source in &poll_revalidations {
                                emit_opt_packed_loop_revalidate(
                                    &mut builder,
                                    &env,
                                    &stack_provenance,
                                    depth,
                                    node.pc(),
                                    *guard,
                                    *source,
                                    element_layout,
                                    array_query,
                                )?;
                            }
                        }
                    }
                    crate::ir::OptimizedNodeKind::Reuse { source } => {
                        depth = depth
                            .checked_sub(usize::from(node.pops()))
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let pair = *reusable_values
                            .get(source)
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        opt_define(&mut builder, stack[depth], pair);
                        stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                        reusable_values.insert(*node_id, pair);
                        depth += 1;
                    }
                    crate::ir::OptimizedNodeKind::Bytecode { opcode } => {
                        let name = opcode.as_ref();
                        if array_plan.is_empty()
                            && node.effect() == crate::ir::OptimizedEffect::Reentrant
                        {
                            guarded_element_source.clear();
                        }
                        match name {
                            "set_loc_uninitialized" => {
                                guarded_element_source.clear();
                                let index = opt_u16(node.bytes())?;
                                if owned_locals[index] {
                                    emit_opt_free_local_slot(&mut builder, &env, index)?;
                                }
                                // Store the exact interpreter cell so a later
                                // deopt/exit resumes with the binding still in
                                // its TDZ. Checked reads of it are never
                                // admitted (see `prove_lexical_checks`).
                                let uninitialized = OptPair {
                                    payload: builder.ins().iconst(payload_type, 0),
                                    tag: builder
                                        .ins()
                                        .iconst(types::I64, i64::from(qjs::JS_TAG_UNINITIALIZED)),
                                };
                                opt_define(&mut builder, locals[index], uninitialized);
                                if !int32_loop && !defer_scalar_locals {
                                    opt_store(&mut builder, var_buf, index, uninitialized);
                                }
                            }
                            "push_i8" => {
                                let value = i64::from(
                                    node.bytes()
                                        .get(1)
                                        .copied()
                                        .ok_or(CompileFailure::InvalidArtifact)?
                                        as i8,
                                );
                                let pair = OptPair {
                                    payload: builder.ins().iconst(payload_type, value),
                                    tag: builder
                                        .ins()
                                        .iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
                                };
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                reusable_values.insert(*node_id, pair);
                                depth += 1;
                            }
                            "undefined" => {
                                opt_define(&mut builder, stack[depth], undefined);
                                stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                depth += 1;
                            }
                            "push_i16" => {
                                let value = i64::from(i16::from_le_bytes([
                                    node.bytes()[1],
                                    node.bytes()[2],
                                ]));
                                let pair = OptPair {
                                    payload: builder.ins().iconst(payload_type, value),
                                    tag: builder
                                        .ins()
                                        .iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
                                };
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                reusable_values.insert(*node_id, pair);
                                depth += 1;
                            }
                            "push_i32" => {
                                let value = i64::from(i32::from_le_bytes(
                                    node.bytes()[1..5]
                                        .try_into()
                                        .map_err(|_| CompileFailure::InvalidArtifact)?,
                                ));
                                let pair = OptPair {
                                    payload: builder.ins().iconst(payload_type, value),
                                    tag: builder
                                        .ins()
                                        .iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
                                };
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                depth += 1;
                            }
                            "push_const" | "push_const8" => {
                                let constant = if name == "push_const8" {
                                    u32::from(
                                        *node
                                            .bytes()
                                            .get(1)
                                            .ok_or(CompileFailure::InvalidArtifact)?,
                                    )
                                } else {
                                    u32::from_le_bytes(
                                        node.bytes()
                                            .get(1..5)
                                            .ok_or(CompileFailure::InvalidArtifact)?
                                            .try_into()
                                            .map_err(|_| CompileFailure::InvalidArtifact)?,
                                    )
                                };
                                let constant = specialization
                                    .numeric_constants
                                    .get(&constant)
                                    .copied()
                                    .ok_or(CompileFailure::UnsupportedOpcode)?;
                                let pair = OptPair {
                                    payload: builder
                                        .ins()
                                        .iconst(types::I64, constant.payload as i64),
                                    tag: builder.ins().iconst(types::I64, constant.tag),
                                };
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                depth += 1;
                            }
                            "push_0" | "push_1" | "push_2" | "push_3" | "push_4" | "push_5"
                            | "push_6" | "push_7" => {
                                let value = i64::from(name.as_bytes()[5] - b'0');
                                let pair = OptPair {
                                    payload: builder.ins().iconst(payload_type, value),
                                    tag: builder
                                        .ins()
                                        .iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
                                };
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                depth += 1;
                            }
                            n if opt_index(n, node.bytes(), "get_arg")?.is_some() => {
                                let index = opt_index(n, node.bytes(), "get_arg")?.unwrap();
                                let pair = opt_use(&mut builder, arguments[index]);
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::Argument(index);
                                depth += 1;
                            }
                            n if n != "get_loc0_loc1"
                                && (opt_index(n, node.bytes(), "get_loc")?.is_some()
                                    || n == "get_loc_check") =>
                            {
                                let index = opt_index(n, node.bytes(), "get_loc")?
                                    .map_or_else(|| opt_u16(node.bytes()), Ok)?;
                                let pair = opt_use(&mut builder, locals[index]);
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::Local(index);
                                depth += 1;
                            }
                            n if opt_index(n, node.bytes(), "put_loc")?.is_some()
                                || matches!(n, "put_loc_check" | "put_loc_check_init") =>
                            {
                                guarded_element_source.clear();
                                let index = opt_index(n, node.bytes(), "put_loc")?
                                    .map_or_else(|| opt_u16(node.bytes()), Ok)?;
                                let source_owned = depth
                                    .checked_sub(1)
                                    .and_then(|source| stack_provenance.get(source))
                                    == Some(&OptProvenance::OwnedSlot);
                                depth = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                emit_opt_alias_store_guard(
                                    &mut builder,
                                    &env,
                                    specialization,
                                    &stack_provenance,
                                    depth + 1,
                                    depth,
                                    OptProvenance::Local(index),
                                    node.pc(),
                                    node.deopt_guard(),
                                )?;
                                let pair = opt_use(&mut builder, stack[depth]);
                                // A value owned by its stack slot moves into
                                // the local exactly like the interpreter's
                                // set_value: release what the local owned,
                                // then let var_buf own the new value.
                                if owned_locals[index] {
                                    emit_opt_free_local_slot(&mut builder, &env, index)?;
                                }
                                opt_define(&mut builder, locals[index], pair);
                                if !int32_loop && !defer_scalar_locals {
                                    opt_store(&mut builder, var_buf, index, pair);
                                }
                                if source_owned {
                                    // `put_loc` moves, rather than copies, an
                                    // owning JSValue. The var buffer becomes
                                    // the sole owner; clear the now-inactive
                                    // stack slot before any GC/finalizer can
                                    // observe both byte-identical references.
                                    let undefined = OptPair {
                                        payload: builder.ins().iconst(types::I64, 0),
                                        tag: builder
                                            .ins()
                                            .iconst(types::I64, i64::from(qjs::JS_TAG_UNDEFINED)),
                                    };
                                    opt_store(&mut builder, stack_base, depth, undefined);
                                    opt_define(&mut builder, stack[depth], undefined);
                                    opt_set_stack_top(
                                        &mut builder,
                                        frame,
                                        stack_base,
                                        depth,
                                        pointer_type,
                                        layout,
                                    );
                                }
                                opt_invalidate_provenance(
                                    &mut stack_provenance,
                                    depth,
                                    OptProvenance::Local(index),
                                );
                            }
                            "get_var" => {
                                let atom = opt_u32(node.bytes())?;
                                depth = emit_opt_owned_helper_push(
                                    &mut builder,
                                    &env,
                                    &mut stack_provenance,
                                    depth,
                                    node.pc(),
                                    qjs::JSJitHelperId_JS_JIT_HELPER_GET_GLOBAL as usize,
                                    &[atom],
                                )?;
                            }
                            "get_field2" => {
                                let atom = opt_u32(node.bytes())?;
                                let receiver = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                let receiver_slot = opt_flat_stack_slot(&env, receiver)?;
                                depth = emit_opt_owned_helper_push(
                                    &mut builder,
                                    &env,
                                    &mut stack_provenance,
                                    depth,
                                    node.pc(),
                                    qjs::JSJitHelperId_JS_JIT_HELPER_GET_PROPERTY as usize,
                                    &[receiver_slot, atom],
                                )?;
                            }
                            "is_undefined_or_null" => {
                                opt_reject_owned(
                                    &stack_provenance,
                                    depth.saturating_sub(1)..depth,
                                )?;
                                let index = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                let value = opt_use(&mut builder, stack[index]);
                                let undefined = builder.ins().icmp_imm(
                                    cranelift_codegen::ir::condcodes::IntCC::Equal,
                                    value.tag,
                                    i64::from(qjs::JS_TAG_UNDEFINED),
                                );
                                let null = builder.ins().icmp_imm(
                                    cranelift_codegen::ir::condcodes::IntCC::Equal,
                                    value.tag,
                                    i64::from(qjs::JS_TAG_NULL),
                                );
                                let truth = builder.ins().bor(undefined, null);
                                let result = opt_bool_pair(&mut builder, &env, truth);
                                opt_define(&mut builder, stack[index], result);
                                stack_provenance[index] = OptProvenance::ImmediatePrimitive;
                            }
                            "to_propkey" => {
                                opt_reject_owned(
                                    &stack_provenance,
                                    depth.saturating_sub(1)..depth,
                                )?;
                                depth = emit_opt_guarded_propkey(
                                    &mut builder,
                                    frame,
                                    sret,
                                    arg_buf,
                                    var_buf,
                                    stack_base,
                                    &arguments,
                                    &locals,
                                    &stack,
                                    &mut stack_provenance,
                                    depth,
                                    node.pc(),
                                    node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
                                    &helper_signatures,
                                    pointer_type,
                                    layout,
                                )?;
                            }
                            "drop" => {
                                depth = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                            }
                            "get_field" | "put_field" => {
                                if name == "put_field"
                                    && specialization.generic_properties.contains(&node.pc())
                                {
                                    property_cache.flush(&mut builder);
                                    property_cache.invalidate(&mut builder);
                                    let atom = opt_u32(node.bytes())?;
                                    depth = emit_opt_owned_property_store(
                                        &mut builder,
                                        &env,
                                        &mut stack_provenance,
                                        depth,
                                        node.pc(),
                                        atom,
                                    )?;
                                    continue;
                                }
                                if name == "get_field"
                                    && (specialization.generic_properties.contains(&node.pc())
                                        || specialization.properties.get(&node.pc()).is_some_and(
                                            |observations| {
                                                observations.iter().any(|observation| {
                                                    observation.value()
                                                        == crate::runtime::ObservedType::Object
                                                })
                                            },
                                        ))
                                {
                                    property_cache.flush(&mut builder);
                                    property_cache.invalidate(&mut builder);
                                    let atom = opt_u32(node.bytes())?;
                                    depth = emit_opt_owned_property_replace(
                                        &mut builder,
                                        &env,
                                        &mut stack_provenance,
                                        depth,
                                        node.pc(),
                                        atom,
                                    )?;
                                    continue;
                                }
                                if let Some(access) = property_plan.access(node.id()) {
                                    if !property_cache.can_emit_access(
                                        access,
                                        depth,
                                        &stack_provenance,
                                    ) {
                                        // Planning used this exact site as a
                                        // non-reentrant capability. Never turn
                                        // it back into a generic helper path.
                                        return Err(CompileFailure::UnsupportedOpcode);
                                    }
                                    depth = property_cache.emit_access(
                                        &mut builder,
                                        &env,
                                        access,
                                        node,
                                        loop_forwarded_property_sites.contains(&node.id()),
                                        match ir.scalar_graph().heap_operation(node.id()) {
                                            Some(crate::ir::ScalarHeapOperation::PutProperty {
                                                value,
                                                ..
                                            }) if access.store => {
                                                scalar_numeric.get(value.index()).copied().flatten()
                                            }
                                            _ => None,
                                        },
                                        depth,
                                        &mut stack_provenance,
                                    )?;
                                    continue;
                                }
                                // Legacy paths can call helpers or leave through their own
                                // recovery blocks, so publish before entering them.
                                property_cache.flush(&mut builder);
                                property_cache.invalidate(&mut builder);
                                opt_reject_owned(
                                    &stack_provenance,
                                    depth.saturating_sub(2)..depth,
                                )?;
                                let Some(property) = specialization.properties.get(&node.pc())
                                else {
                                    #[cfg(feature = "test-support")]
                                    if let Some(key) = specialization.key {
                                        record_tier2_stage(
                                            key,
                                            Tier2CompileStage::PropertyFeedbackMissing,
                                            Some(CompileFailure::InvalidArtifact),
                                        );
                                    }
                                    return Err(CompileFailure::InvalidArtifact);
                                };
                                depth = emit_opt_guarded_property(
                                    &mut builder,
                                    frame,
                                    sret,
                                    arg_buf,
                                    var_buf,
                                    stack_base,
                                    &arguments,
                                    &locals,
                                    &stack,
                                    &mut stack_provenance,
                                    depth,
                                    name == "put_field",
                                    property,
                                    node.pc(),
                                    node.deopt_guard().unwrap_or(entry_site.guard()),
                                    &helper_signatures,
                                    pointer_type,
                                    layout,
                                )?;
                            }
                            "get_array_el" => {
                                let planned_access = array_plan
                                    .access(node.id())
                                    .filter(|access| {
                                        access.access == crate::runtime::ArrayAccess::Load
                                    })
                                    .filter(|access| {
                                        access.requires_index_guard
                                            || access.bounds_covered_by_hoist
                                    });
                                let expected_mode = planned_access
                                    .and_then(|access| {
                                        array_plan.candidates().get(access.candidate)
                                    })
                                    .filter(|candidate| {
                                        depth >= 2
                                            && stack_provenance[depth - 2]
                                                == opt_array_receiver_provenance(candidate.receiver)
                                    })
                                    .map(|candidate| candidate.mode);
                                opt_reject_owned(
                                    &stack_provenance,
                                    depth.saturating_sub(2)..depth,
                                )?;
                                depth = emit_opt_element_get(
                                    &mut builder,
                                    frame,
                                    sret,
                                    arg_buf,
                                    var_buf,
                                    stack_base,
                                    &arguments,
                                    &locals,
                                    &stack,
                                    &mut stack_provenance,
                                    depth,
                                    node.pc(),
                                    node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
                                    &helper_signatures,
                                    pointer_type,
                                    layout,
                                    element_layout,
                                    block.start_pc(),
                                    &mut guarded_element_source,
                                    expected_mode,
                                    // The hoist proved bounds for the planned
                                    // receiver only. When the operand's
                                    // provenance does not name that receiver,
                                    // any cached tuple belongs to another slot:
                                    // keep the per-access bounds check.
                                    expected_mode.is_some()
                                        && planned_access
                                            .is_some_and(|access| access.bounds_covered_by_hoist),
                                )?;
                            }
                            "get_length" => {
                                opt_reject_owned(
                                    &stack_provenance,
                                    depth.saturating_sub(1)..depth,
                                )?;
                                // A planned typed length selects the versioned
                                // leaf query (exit on miss) rather than the
                                // accessor-running lookup bridge.
                                let planned_typed = array_plan
                                    .access(node.id())
                                    .filter(|access| {
                                        access.access == crate::runtime::ArrayAccess::Length
                                    })
                                    .and_then(|access| {
                                        array_plan.candidates().get(access.candidate)
                                    })
                                    .filter(|candidate| {
                                        matches!(
                                            candidate.mode,
                                            crate::runtime::ArrayMode::Int32
                                                | crate::runtime::ArrayMode::Float64
                                        )
                                    });
                                // The query guards the actual operand, so it
                                // is valid whichever frame slot supplied it;
                                // provenance only keys the cached tuple. A
                                // certified leaf length therefore never falls
                                // back to the reentrant bridge.
                                let typed_length = match planned_typed {
                                    Some(candidate) => Some((
                                        candidate.mode,
                                        node.deopt_guard()
                                            .ok_or(CompileFailure::InvalidArtifact)?,
                                        block.start_pc(),
                                        array_query,
                                    )),
                                    None => None,
                                };
                                depth = emit_opt_array_length(
                                    &mut builder,
                                    &env,
                                    &mut stack_provenance,
                                    depth,
                                    node.pc(),
                                    element_layout,
                                    &mut guarded_element_source,
                                    typed_length,
                                )?;
                            }
                            "put_array_el" => {
                                opt_reject_owned(
                                    &stack_provenance,
                                    depth.saturating_sub(3)..depth,
                                )?;
                                let source_provenance = stack_provenance[depth
                                    .checked_sub(3)
                                    .ok_or(CompileFailure::InvalidArtifact)?];
                                if let Some(candidate) = array_plan
                                    .access(node.id())
                                    .filter(|access| {
                                        access.access == crate::runtime::ArrayAccess::Store
                                    })
                                    .and_then(|access| {
                                        array_plan.candidates().get(access.candidate)
                                    })
                                {
                                    // Every certified store must select this guarded leaf.
                                    // A provenance disagreement cannot silently fall back to
                                    // an operation stronger than the published effect.
                                    if source_provenance
                                        != opt_array_receiver_provenance(candidate.receiver)
                                    {
                                        return Err(CompileFailure::InvalidArtifact);
                                    }
                                    depth = emit_opt_typed_store(
                                        &mut builder,
                                        &env,
                                        &mut stack_provenance,
                                        depth,
                                        node.pc(),
                                        node.deopt_guard()
                                            .ok_or(CompileFailure::InvalidArtifact)?,
                                        block.start_pc(),
                                        candidate.mode,
                                        array_query,
                                        &mut guarded_element_source,
                                    )?;
                                    continue;
                                }
                                depth = emit_opt_element_put(
                                    &mut builder,
                                    frame,
                                    sret,
                                    arg_buf,
                                    var_buf,
                                    stack_base,
                                    &arguments,
                                    &locals,
                                    &stack,
                                    &mut stack_provenance,
                                    depth,
                                    node.pc(),
                                    node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
                                    &helper_signatures,
                                    pointer_type,
                                    layout,
                                    element_layout,
                                    block.start_pc(),
                                    source_provenance,
                                    &mut guarded_element_source,
                                )?;
                            }
                            "call" | "call0" | "call1" | "call2" | "call3" | "call_method" => {
                                // Native callees (Math.max, String, ...) never
                                // record call-site feedback; they take the
                                // generic CALL bridge with an owned result.
                                let call = specialization.calls.get(&node.pc());
                                let Some(semantic_call) = ir.scalar_graph().call(node.id()) else {
                                    #[cfg(feature = "test-support")]
                                    if let Some(key) = specialization.key {
                                        record_tier2_stage(
                                            key,
                                            Tier2CompileStage::GenericCallSemanticMissing,
                                            Some(CompileFailure::InvalidArtifact),
                                        );
                                    }
                                    return Err(CompileFailure::InvalidArtifact);
                                };
                                let argc = semantic_call.arguments.len();
                                let has_this = semantic_call.receiver.is_some();
                                opt_restore_scalar_frame(
                                    &mut builder,
                                    ir,
                                    node,
                                    depth,
                                    &env,
                                    &scalar_values,
                                )?;
                                let Some(state) = ir.scalar_graph().frame_state_for_node(node.id())
                                else {
                                    #[cfg(feature = "test-support")]
                                    if let Some(key) = specialization.key {
                                        record_tier2_stage(
                                            key,
                                            Tier2CompileStage::GenericCallFrameStateMissing,
                                            Some(CompileFailure::InvalidArtifact),
                                        );
                                    }
                                    return Err(CompileFailure::InvalidArtifact);
                                };
                                let pop = argc + 1 + usize::from(has_this);
                                let base = depth
                                    .checked_sub(pop)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                let target = base + usize::from(has_this);
                                if semantic_call.frame_state_node != node.id()
                                    || usize::from(node.pops()) != pop
                                    || state.stack[target] != semantic_call.target
                                    || state.stack[target + 1..] != *semantic_call.arguments
                                    || semantic_call
                                        .receiver
                                        .is_some_and(|receiver| state.stack[base] != receiver)
                                {
                                    #[cfg(feature = "test-support")]
                                    if let Some(key) = specialization.key {
                                        record_tier2_stage(
                                            key,
                                            Tier2CompileStage::GenericCallShapeMismatch,
                                            Some(CompileFailure::InvalidArtifact),
                                        );
                                    }
                                    return Err(CompileFailure::InvalidArtifact);
                                }
                                if call.is_some_and(|call| argc != call.arguments().len()) {
                                    #[cfg(feature = "test-support")]
                                    if let Some(key) = specialization.key {
                                        record_tier2_stage(
                                            key,
                                            Tier2CompileStage::GenericCallArityMismatch,
                                            Some(CompileFailure::InvalidArtifact),
                                        );
                                    }
                                    return Err(CompileFailure::InvalidArtifact);
                                }
                                if let Some(region) = semantic_call.frame_inline.as_deref() {
                                    depth = frame_inline::emit(
                                        &mut builder,
                                        &env,
                                        region,
                                        node,
                                        argc,
                                        &mut stack_provenance,
                                        depth,
                                        inline_api,
                                        node.deopt_guard()
                                            .ok_or(CompileFailure::InvalidArtifact)?,
                                    )?;
                                    continue;
                                }
                                if call.is_none() && int32_loop {
                                    return Err(CompileFailure::InvalidArtifact);
                                }
                                if semantic_call.inline.is_some() {
                                    hoisted_calls.admit_call(
                                        &call_guard_state,
                                        &mut builder,
                                        &env,
                                        node,
                                        &stack_provenance,
                                        depth,
                                    )?;
                                    let result = emit_opt_inlined_call(
                                        &mut builder,
                                        ir,
                                        node,
                                        depth,
                                        &env,
                                        &stack_provenance,
                                        &scalar_values,
                                        &scalar_numeric,
                                        hoisted_calls.contains(node.id()),
                                    )?;
                                    opt_define(&mut builder, stack[base], result);
                                    stack_provenance[base] = OptProvenance::ImmediatePrimitive;
                                    depth = base + 1;
                                    continue;
                                }
                                let Some(call_guard) = node.deopt_guard() else {
                                    #[cfg(feature = "test-support")]
                                    if let Some(key) = specialization.key {
                                        record_tier2_stage(
                                            key,
                                            Tier2CompileStage::GenericCallGuardMissing,
                                            Some(CompileFailure::InvalidArtifact),
                                        );
                                    }
                                    return Err(CompileFailure::InvalidArtifact);
                                };
                                let emitted = emit_opt_specialized_call(
                                    &mut builder,
                                    frame,
                                    sret,
                                    arg_buf,
                                    var_buf,
                                    stack_base,
                                    &arguments,
                                    &locals,
                                    &stack,
                                    &mut stack_provenance,
                                    depth,
                                    argc,
                                    has_this,
                                    node.pc(),
                                    &helper_signatures,
                                    pointer_type,
                                    layout,
                                    specialization.direct_calls.get(&node.pc()),
                                    specialization.native_calls.get(&node.pc()),
                                    call_guard,
                                    call.is_some_and(|call| {
                                        call.result()
                                            != crate::runtime::FeedbackRepresentation::HeapRef
                                    }),
                                    specialization.key,
                                );
                                depth = if let Some(key) = specialization.key {
                                    tier2_stage(key, Tier2CompileStage::GenericCallEmit, emitted)?
                                } else {
                                    emitted?
                                };
                            }
                            "add" | "sub" | "mul" | "div" => {
                                depth = depth
                                    .checked_sub(2)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                let (lhs, rhs) = opt_prepare_scalar_operation(
                                    &mut builder,
                                    ir,
                                    node,
                                    depth + 2,
                                    &env,
                                    &stack_provenance,
                                    &scalar_values,
                                    &scalar_numeric,
                                )?;
                                let rhs = rhs.ok_or(CompileFailure::InvalidArtifact)?;
                                let (operation, mode) = ir
                                    .scalar_graph()
                                    .binary_operation(*node_id)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                use crate::ir::{ScalarBinaryOp, ScalarNumericMode};
                                if mode == ScalarNumericMode::Float64 {
                                    let lhs = builder.ins().bitcast(
                                        types::F64,
                                        MemFlags::new(),
                                        lhs.payload,
                                    );
                                    let rhs = builder.ins().bitcast(
                                        types::F64,
                                        MemFlags::new(),
                                        rhs.payload,
                                    );
                                    let result = match operation {
                                        ScalarBinaryOp::Add => builder.ins().fadd(lhs, rhs),
                                        ScalarBinaryOp::Sub => builder.ins().fsub(lhs, rhs),
                                        ScalarBinaryOp::Mul => builder.ins().fmul(lhs, rhs),
                                        _ => builder.ins().fdiv(lhs, rhs),
                                    };
                                    let pair = OptPair {
                                        payload: builder.ins().bitcast(
                                            types::I64,
                                            MemFlags::new(),
                                            result,
                                        ),
                                        tag: builder
                                            .ins()
                                            .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64)),
                                    };
                                    opt_define(&mut builder, stack[depth], pair);
                                    stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                    reusable_values.insert(*node_id, pair);
                                    depth += 1;
                                    continue;
                                }
                                if mode == ScalarNumericMode::Int32 {
                                    let deopt = builder.create_block();
                                    let li = if int32_loop {
                                        lhs.payload
                                    } else {
                                        builder.ins().ireduce(types::I32, lhs.payload)
                                    };
                                    let ri = if int32_loop {
                                        rhs.payload
                                    } else {
                                        builder.ins().ireduce(types::I32, rhs.payload)
                                    };
                                    let pass = builder.create_block();
                                    builder.append_block_param(pass, types::I32);
                                    if operation == ScalarBinaryOp::Div {
                                        use cranelift_codegen::ir::condcodes::IntCC;
                                        let zero = builder.ins().icmp_imm(IntCC::Equal, ri, 0);
                                        let min = builder.ins().icmp_imm(
                                            IntCC::Equal,
                                            li,
                                            i64::from(i32::MIN),
                                        );
                                        let negative_one =
                                            builder.ins().icmp_imm(IntCC::Equal, ri, -1);
                                        let min_overflow = builder.ins().band(min, negative_one);
                                        let zero_lhs = builder.ins().icmp_imm(IntCC::Equal, li, 0);
                                        let negative_rhs =
                                            builder.ins().icmp_imm(IntCC::SignedLessThan, ri, 0);
                                        let negative_zero =
                                            builder.ins().band(zero_lhs, negative_rhs);
                                        let exceptional = builder.ins().bor(zero, min_overflow);
                                        let exceptional =
                                            builder.ins().bor(exceptional, negative_zero);
                                        let safe = builder.create_block();
                                        builder.ins().brif(exceptional, deopt, &[], safe, &[]);
                                        builder.switch_to_block(safe);
                                        let remainder = builder.ins().srem(li, ri);
                                        let exact =
                                            builder.ins().icmp_imm(IntCC::Equal, remainder, 0);
                                        let quotient = builder.create_block();
                                        builder.ins().brif(exact, quotient, &[], deopt, &[]);
                                        builder.switch_to_block(quotient);
                                        let result = builder.ins().sdiv(li, ri);
                                        builder.ins().jump(pass, &[result]);
                                    } else {
                                        let (result, mut failure) = match operation {
                                            ScalarBinaryOp::Add => {
                                                builder.ins().sadd_overflow(li, ri)
                                            }
                                            ScalarBinaryOp::Sub => {
                                                builder.ins().ssub_overflow(li, ri)
                                            }
                                            _ => builder.ins().smul_overflow(li, ri),
                                        };
                                        if operation == ScalarBinaryOp::Mul {
                                            use cranelift_codegen::ir::condcodes::IntCC;
                                            let lhs_zero =
                                                builder.ins().icmp_imm(IntCC::Equal, li, 0);
                                            let rhs_zero =
                                                builder.ins().icmp_imm(IntCC::Equal, ri, 0);
                                            let lhs_negative = builder.ins().icmp_imm(
                                                IntCC::SignedLessThan,
                                                li,
                                                0,
                                            );
                                            let rhs_negative = builder.ins().icmp_imm(
                                                IntCC::SignedLessThan,
                                                ri,
                                                0,
                                            );
                                            let lhs_zero_negative_rhs =
                                                builder.ins().band(lhs_zero, rhs_negative);
                                            let rhs_zero_negative_lhs =
                                                builder.ins().band(rhs_zero, lhs_negative);
                                            let negative_zero = builder
                                                .ins()
                                                .bor(lhs_zero_negative_rhs, rhs_zero_negative_lhs);
                                            failure = builder.ins().bor(failure, negative_zero);
                                        }
                                        builder.ins().brif(failure, deopt, &[], pass, &[result]);
                                    }
                                    // Every edge into this local diamond is
                                    // declared; sealing keeps later reads of
                                    // unchanged variables (and their literal
                                    // tags) direct.
                                    builder.seal_block(deopt);
                                    builder.seal_block(pass);
                                    builder.switch_to_block(deopt);
                                    emit_opt_deopt(
                                        &mut builder,
                                        &env,
                                        &stack_provenance,
                                        depth + 2,
                                        node.pc(),
                                        node.deopt_guard()
                                            .ok_or(CompileFailure::InvalidArtifact)?,
                                    )?;
                                    builder.switch_to_block(pass);
                                    let result = builder.block_params(pass)[0];
                                    let pair = OptPair {
                                        payload: if int32_loop {
                                            result
                                        } else {
                                            builder.ins().sextend(types::I64, result)
                                        },
                                        tag: builder
                                            .ins()
                                            .iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
                                    };
                                    opt_define(&mut builder, stack[depth], pair);
                                    stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                    reusable_values.insert(*node_id, pair);
                                    depth += 1;
                                    continue;
                                }
                                let lf = opt_f64(&mut builder, lhs);
                                let rf = opt_f64(&mut builder, rhs);
                                let result = match operation {
                                    ScalarBinaryOp::Add => builder.ins().fadd(lf, rf),
                                    ScalarBinaryOp::Sub => builder.ins().fsub(lf, rf),
                                    ScalarBinaryOp::Mul => builder.ins().fmul(lf, rf),
                                    _ => builder.ins().fdiv(lf, rf),
                                };
                                let float_payload =
                                    builder.ins().bitcast(types::I64, MemFlags::new(), result);
                                let float_tag = builder
                                    .ins()
                                    .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64));
                                let pair = if operation == ScalarBinaryOp::Add {
                                    use cranelift_codegen::ir::condcodes::IntCC;
                                    let lhs_int = builder.ins().icmp_imm(
                                        IntCC::Equal,
                                        lhs.tag,
                                        i64::from(qjs::JS_TAG_INT),
                                    );
                                    let rhs_int = builder.ins().icmp_imm(
                                        IntCC::Equal,
                                        rhs.tag,
                                        i64::from(qjs::JS_TAG_INT),
                                    );
                                    let both_int = builder.ins().band(lhs_int, rhs_int);
                                    let li = builder.ins().ireduce(types::I32, lhs.payload);
                                    let ri = builder.ins().ireduce(types::I32, rhs.payload);
                                    let (sum, overflow) = builder.ins().sadd_overflow(li, ri);
                                    let no_overflow = builder.ins().bnot(overflow);
                                    let keep_int = builder.ins().band(both_int, no_overflow);
                                    let int_payload = builder.ins().sextend(types::I64, sum);
                                    let int_tag = builder
                                        .ins()
                                        .iconst(types::I64, i64::from(qjs::JS_TAG_INT));
                                    OptPair {
                                        payload: builder.ins().select(
                                            keep_int,
                                            int_payload,
                                            float_payload,
                                        ),
                                        tag: builder.ins().select(keep_int, int_tag, float_tag),
                                    }
                                } else {
                                    OptPair {
                                        payload: float_payload,
                                        tag: float_tag,
                                    }
                                };
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                reusable_values.insert(*node_id, pair);
                                depth += 1;
                            }
                            "or" | "and" | "xor" | "shl" | "sar" | "shr" => {
                                let (lhs, rhs) = opt_prepare_scalar_operation(
                                    &mut builder,
                                    ir,
                                    node,
                                    depth,
                                    &env,
                                    &stack_provenance,
                                    &scalar_values,
                                    &scalar_numeric,
                                )?;
                                let rhs = rhs.ok_or(CompileFailure::InvalidArtifact)?;
                                let (operation, _, _) = ir
                                    .scalar_graph()
                                    .bitwise_operation(node.id())
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                depth = emit_opt_guarded_int_binary(
                                    &mut builder,
                                    &env,
                                    &mut stack_provenance,
                                    depth,
                                    operation,
                                    lhs,
                                    rhs,
                                    node.pc(),
                                    node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
                                )?;
                            }
                            "mod" => {
                                depth = emit_opt_mod(
                                    &mut builder,
                                    &env,
                                    &mut stack_provenance,
                                    depth,
                                    node.pc(),
                                    node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
                                )?;
                            }
                            "eq" | "neq" | "strict_eq" | "strict_neq" => {
                                depth = emit_opt_equality(
                                    &mut builder,
                                    &env,
                                    &mut stack_provenance,
                                    depth,
                                    name,
                                    node.pc(),
                                    node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
                                )?;
                            }
                            "neg" | "plus" | "not" | "lnot" => {
                                emit_opt_unary(
                                    &mut builder,
                                    &env,
                                    &mut stack_provenance,
                                    depth,
                                    name,
                                    node.pc(),
                                    node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
                                )?;
                            }
                            "is_undefined" | "is_null" => {
                                opt_reject_owned(
                                    &stack_provenance,
                                    depth.saturating_sub(1)..depth,
                                )?;
                                let index = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                let value = opt_use(&mut builder, stack[index]);
                                let expected = if name == "is_undefined" {
                                    qjs::JS_TAG_UNDEFINED
                                } else {
                                    qjs::JS_TAG_NULL
                                };
                                let truth = opt_tag_is(&mut builder, value.tag, expected);
                                let pair = opt_bool_pair(&mut builder, &env, truth);
                                opt_define(&mut builder, stack[index], pair);
                                stack_provenance[index] = OptProvenance::ImmediatePrimitive;
                            }
                            "inc_loc" | "dec_loc" => {
                                guarded_element_source.clear();
                                let index = opt_u8(node.bytes())?;
                                let (old, _) = opt_prepare_scalar_operation(
                                    &mut builder,
                                    ir,
                                    node,
                                    depth,
                                    &env,
                                    &stack_provenance,
                                    &scalar_values,
                                    &scalar_numeric,
                                )?;
                                let (mode, _, delta) = ir
                                    .scalar_graph()
                                    .update_operation(*node_id)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                if int32_loop && mode != crate::ir::ScalarNumericMode::Int32 {
                                    return Err(CompileFailure::InvalidArtifact);
                                }
                                let delta = OptPair {
                                    payload: builder.ins().iconst(payload_type, i64::from(delta)),
                                    tag: builder
                                        .ins()
                                        .iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
                                };
                                let pair = emit_opt_checked_update(
                                    &mut builder,
                                    &env,
                                    mode,
                                    &stack_provenance,
                                    depth,
                                    old,
                                    delta,
                                    node.pc(),
                                    node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
                                )?;
                                opt_define(&mut builder, locals[index], pair);
                                if !int32_loop && !defer_scalar_locals {
                                    opt_store(&mut builder, var_buf, index, pair);
                                }
                                opt_invalidate_provenance(
                                    &mut stack_provenance,
                                    depth,
                                    OptProvenance::Local(index),
                                );
                            }
                            "add_loc" => {
                                guarded_element_source.clear();
                                let index = opt_u8(node.bytes())?;
                                let rhs_index = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                let lhs = opt_use(&mut builder, locals[index]);
                                let rhs = opt_use(&mut builder, stack[rhs_index]);
                                let pair = emit_opt_checked_add(
                                    &mut builder,
                                    &env,
                                    &stack_provenance,
                                    depth,
                                    lhs,
                                    rhs,
                                    node.pc(),
                                    node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
                                )?;
                                opt_define(&mut builder, locals[index], pair);
                                if !int32_loop && !defer_scalar_locals {
                                    opt_store(&mut builder, var_buf, index, pair);
                                }
                                depth = rhs_index;
                                opt_invalidate_provenance(
                                    &mut stack_provenance,
                                    depth,
                                    OptProvenance::Local(index),
                                );
                            }
                            "push_minus1" => {
                                let pair = OptPair {
                                    payload: builder.ins().iconst(payload_type, -1),
                                    tag: builder
                                        .ins()
                                        .iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
                                };
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                reusable_values.insert(*node_id, pair);
                                depth += 1;
                            }
                            "null" | "push_true" | "push_false" => {
                                let (payload, tag) = match name {
                                    "null" => (0, qjs::JS_TAG_NULL),
                                    "push_true" => (1, qjs::JS_TAG_BOOL),
                                    _ => (0, qjs::JS_TAG_BOOL),
                                };
                                let pair = OptPair {
                                    payload: builder.ins().iconst(payload_type, payload),
                                    tag: builder.ins().iconst(types::I64, i64::from(tag)),
                                };
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                depth += 1;
                            }
                            "get_loc0_loc1" => {
                                if locals.len() < 2 {
                                    return Err(CompileFailure::InvalidArtifact);
                                }
                                for (index, vars) in locals.iter().take(2).enumerate() {
                                    let pair = opt_use(&mut builder, *vars);
                                    opt_define(&mut builder, stack[depth], pair);
                                    stack_provenance[depth] = OptProvenance::Local(index);
                                    depth += 1;
                                }
                            }
                            n if opt_index(n, node.bytes(), "put_arg")?.is_some() => {
                                guarded_element_source.clear();
                                opt_reject_owned(
                                    &stack_provenance,
                                    depth.saturating_sub(1)..depth,
                                )?;
                                let index = opt_index(n, node.bytes(), "put_arg")?.unwrap();
                                depth = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                emit_opt_alias_store_guard(
                                    &mut builder,
                                    &env,
                                    specialization,
                                    &stack_provenance,
                                    depth + 1,
                                    depth,
                                    OptProvenance::Argument(index),
                                    node.pc(),
                                    node.deopt_guard(),
                                )?;
                                let pair = opt_use(&mut builder, stack[depth]);
                                opt_define(&mut builder, arguments[index], pair);
                                opt_store(&mut builder, arg_buf, index, pair);
                                opt_invalidate_provenance(
                                    &mut stack_provenance,
                                    depth,
                                    OptProvenance::Argument(index),
                                );
                            }
                            n if opt_index(n, node.bytes(), "set_arg")?.is_some() => {
                                guarded_element_source.clear();
                                opt_reject_owned(
                                    &stack_provenance,
                                    depth.saturating_sub(1)..depth,
                                )?;
                                let index = opt_index(n, node.bytes(), "set_arg")?.unwrap();
                                let top = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                emit_opt_alias_store_guard(
                                    &mut builder,
                                    &env,
                                    specialization,
                                    &stack_provenance,
                                    depth,
                                    top,
                                    OptProvenance::Argument(index),
                                    node.pc(),
                                    node.deopt_guard(),
                                )?;
                                let pair = opt_use(&mut builder, stack[top]);
                                opt_define(&mut builder, arguments[index], pair);
                                opt_store(&mut builder, arg_buf, index, pair);
                                // The value stays on the stack; only older
                                // aliases of the slot go stale.
                                opt_invalidate_provenance(
                                    &mut stack_provenance,
                                    top,
                                    OptProvenance::Argument(index),
                                );
                            }
                            n if opt_index(n, node.bytes(), "set_loc")?.is_some() => {
                                guarded_element_source.clear();
                                opt_reject_owned(
                                    &stack_provenance,
                                    depth.saturating_sub(1)..depth,
                                )?;
                                let index = opt_index(n, node.bytes(), "set_loc")?.unwrap();
                                let top = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                emit_opt_alias_store_guard(
                                    &mut builder,
                                    &env,
                                    specialization,
                                    &stack_provenance,
                                    depth,
                                    top,
                                    OptProvenance::Local(index),
                                    node.pc(),
                                    node.deopt_guard(),
                                )?;
                                let pair = opt_use(&mut builder, stack[top]);
                                opt_define(&mut builder, locals[index], pair);
                                if !int32_loop && !defer_scalar_locals {
                                    opt_store(&mut builder, var_buf, index, pair);
                                }
                                opt_invalidate_provenance(
                                    &mut stack_provenance,
                                    top,
                                    OptProvenance::Local(index),
                                );
                            }
                            n if opt_stack_permutation(n).is_some() => {
                                let (take, order) = opt_stack_permutation(n).unwrap();
                                let start = depth
                                    .checked_sub(take)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                if n == "dup" && stack_provenance[start] == OptProvenance::OwnedSlot
                                {
                                    // An owning value needs a second reference, not
                                    // merely a second SSA name. Publish live roots
                                    // before DUP, whose stress-GC boundary may collect.
                                    property_cache.flush(&mut builder);
                                    property_cache.invalidate(&mut builder);
                                    let source = opt_flat_stack_slot(&env, start)?;
                                    depth = emit_opt_owned_dup(
                                        &mut builder,
                                        &env,
                                        &mut stack_provenance,
                                        start,
                                        depth,
                                        node.pc(),
                                        source,
                                    )?;
                                    continue;
                                }
                                opt_reject_owned(&stack_provenance, start..depth)?;
                                if start + order.len() > stack.len() {
                                    return Err(CompileFailure::ResourceLimit);
                                }
                                // Shuffles move SSA values together with the
                                // frame-slot aliases they carry, so a later
                                // exit still materializes every copy of a
                                // borrowed value as its own owner.
                                let values = (0..take)
                                    .map(|offset| {
                                        (
                                            opt_use(&mut builder, stack[start + offset]),
                                            stack_provenance[start + offset],
                                        )
                                    })
                                    .collect::<Vec<_>>();
                                for (destination, source) in order.iter().enumerate() {
                                    let (pair, provenance) = values[*source];
                                    opt_define(&mut builder, stack[start + destination], pair);
                                    stack_provenance[start + destination] = provenance;
                                }
                                depth = start + order.len();
                            }
                            "lt" | "lte" | "gt" | "gte" => {
                                use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
                                depth = depth
                                    .checked_sub(2)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                let (lhs_pair, rhs_pair) = opt_prepare_scalar_operation(
                                    &mut builder,
                                    ir,
                                    node,
                                    depth + 2,
                                    &env,
                                    &stack_provenance,
                                    &scalar_values,
                                    &scalar_numeric,
                                )?;
                                let rhs_pair = rhs_pair.ok_or(CompileFailure::InvalidArtifact)?;
                                let (operation, lhs, rhs) = ir
                                    .scalar_graph()
                                    .comparison(*node_id)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                use crate::ir::ScalarCompareOp;
                                // A literal Int32 tag (for example a
                                // hoisted exact array length or a checked
                                // Int32 result) is as strong as the graph
                                // proof for this one lowering.
                                let integer_compare = int32_loop
                                    || [(lhs, lhs_pair), (rhs, rhs_pair)].iter().all(
                                        |(value, pair)| {
                                            scalar_numeric.get(value.index()).copied().flatten()
                                                == Some(crate::ir::ScalarNumericMode::Int32)
                                                || opt_tag_known(
                                                    &builder,
                                                    pair.tag,
                                                    qjs::JS_TAG_INT,
                                                )
                                        },
                                    );
                                let value = if integer_compare {
                                    let cc = match operation {
                                        ScalarCompareOp::LessThan => IntCC::SignedLessThan,
                                        ScalarCompareOp::LessEqual => IntCC::SignedLessThanOrEqual,
                                        ScalarCompareOp::GreaterThan => IntCC::SignedGreaterThan,
                                        ScalarCompareOp::GreaterEqual => {
                                            IntCC::SignedGreaterThanOrEqual
                                        }
                                    };
                                    let lhs = opt_i32(&mut builder, &env, lhs_pair);
                                    let rhs = opt_i32(&mut builder, &env, rhs_pair);
                                    builder.ins().icmp(cc, lhs, rhs)
                                } else {
                                    let lhs = opt_f64(&mut builder, lhs_pair);
                                    let rhs = opt_f64(&mut builder, rhs_pair);
                                    let cc = match operation {
                                        ScalarCompareOp::LessThan => FloatCC::LessThan,
                                        ScalarCompareOp::LessEqual => FloatCC::LessThanOrEqual,
                                        ScalarCompareOp::GreaterThan => FloatCC::GreaterThan,
                                        ScalarCompareOp::GreaterEqual => {
                                            FloatCC::GreaterThanOrEqual
                                        }
                                    };
                                    builder.ins().fcmp(cc, lhs, rhs)
                                };
                                let pair = opt_bool_pair(&mut builder, &env, value);
                                opt_define(&mut builder, stack[depth], pair);
                                stack_provenance[depth] = OptProvenance::ImmediatePrimitive;
                                stack_provenance[depth + 1] = OptProvenance::Unknown;
                                depth += 1;
                            }
                            "post_inc" | "inc" | "post_dec" | "dec" => {
                                let index = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                let (old, _) = opt_prepare_scalar_operation(
                                    &mut builder,
                                    ir,
                                    node,
                                    depth,
                                    &env,
                                    &stack_provenance,
                                    &scalar_values,
                                    &scalar_numeric,
                                )?;
                                let (mode, _, delta) = ir
                                    .scalar_graph()
                                    .update_operation(*node_id)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                if int32_loop && mode != crate::ir::ScalarNumericMode::Int32 {
                                    return Err(CompileFailure::InvalidArtifact);
                                }
                                let increment = delta == 1;
                                let delta = OptPair {
                                    payload: builder.ins().iconst(payload_type, i64::from(delta)),
                                    tag: builder
                                        .ins()
                                        .iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
                                };
                                let pair = if int32_loop
                                    && increment
                                    && bounded_increments.contains(&node.pc())
                                {
                                    // Proven `k < X` at the loop header with no
                                    // other write to `k`: the increment fits.
                                    let sum = builder.ins().iadd(old.payload, delta.payload);
                                    opt_int_pair(&mut builder, &env, sum)
                                } else {
                                    emit_opt_checked_update(
                                        &mut builder,
                                        &env,
                                        mode,
                                        &stack_provenance,
                                        depth,
                                        old,
                                        delta,
                                        node.pc(),
                                        node.deopt_guard()
                                            .ok_or(CompileFailure::InvalidArtifact)?,
                                    )?
                                };
                                if name.starts_with("post_") {
                                    // ToNumeric of a proven number is the
                                    // number itself, so the old value stays.
                                    opt_define(&mut builder, stack[index + 1], pair);
                                    stack_provenance[index + 1] = OptProvenance::ImmediatePrimitive;
                                    depth += 1;
                                } else {
                                    opt_define(&mut builder, stack[index], pair);
                                    stack_provenance[index] = OptProvenance::ImmediatePrimitive;
                                }
                            }
                            "if_false8" | "if_true8" | "if_false" | "if_true" => {
                                use cranelift_codegen::ir::condcodes::IntCC;
                                depth = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                let condition = opt_use(&mut builder, stack[depth]);
                                if int32_loop {
                                    let truth = builder.ins().icmp_imm(
                                        IntCC::NotEqual,
                                        condition.payload,
                                        0,
                                    );
                                    let target = opt_ir_edge(&blocks, block, node.branch_target())?;
                                    let fallthrough = opt_ir_edge(
                                        &blocks,
                                        block,
                                        Some(next_block_pc(ir, block.start_pc())?),
                                    )?;
                                    if name.starts_with("if_false") {
                                        builder.ins().brif(truth, fallthrough, &[], target, &[]);
                                    } else {
                                        builder.ins().brif(truth, target, &[], fallthrough, &[]);
                                    }
                                    terminated = true;
                                    continue;
                                }
                                if opt_tag_known(&builder, condition.tag, qjs::JS_TAG_BOOL)
                                    || opt_tag_known(&builder, condition.tag, qjs::JS_TAG_INT)
                                {
                                    // A literal Bool/Int32 tag: ToBoolean is
                                    // exactly `int32 payload != 0`, and the
                                    // representation deopt edge is unreachable.
                                    let integer =
                                        builder.ins().ireduce(types::I32, condition.payload);
                                    let truth = builder.ins().icmp_imm(IntCC::NotEqual, integer, 0);
                                    let target = opt_ir_edge(&blocks, block, node.branch_target())?;
                                    let fallthrough = opt_ir_edge(
                                        &blocks,
                                        block,
                                        Some(next_block_pc(ir, block.start_pc())?),
                                    )?;
                                    if let Some(known) = opt_known_const(&builder, truth) {
                                        // Folded from literals: only one edge
                                        // is ever taken. The other IR block
                                        // simply loses this predecessor.
                                        let taken = if (known != 0) == name.starts_with("if_false")
                                        {
                                            fallthrough
                                        } else {
                                            target
                                        };
                                        builder.ins().jump(taken, &[]);
                                    } else if name.starts_with("if_false") {
                                        builder.ins().brif(truth, fallthrough, &[], target, &[]);
                                    } else {
                                        builder.ins().brif(truth, target, &[], fallthrough, &[]);
                                    }
                                    terminated = true;
                                    continue;
                                }
                                let is_int = builder.ins().icmp_imm(
                                    IntCC::Equal,
                                    condition.tag,
                                    i64::from(qjs::JS_TAG_INT),
                                );
                                let is_bool = builder.ins().icmp_imm(
                                    IntCC::Equal,
                                    condition.tag,
                                    i64::from(qjs::JS_TAG_BOOL),
                                );
                                let is_float = builder.ins().icmp_imm(
                                    IntCC::Equal,
                                    condition.tag,
                                    i64::from(qjs::JS_TAG_FLOAT64),
                                );
                                let is_null = builder.ins().icmp_imm(
                                    IntCC::Equal,
                                    condition.tag,
                                    i64::from(qjs::JS_TAG_NULL),
                                );
                                let is_undefined = builder.ins().icmp_imm(
                                    IntCC::Equal,
                                    condition.tag,
                                    i64::from(qjs::JS_TAG_UNDEFINED),
                                );
                                let scalar = builder.ins().bor(is_int, is_bool);
                                let empty = builder.ins().bor(is_null, is_undefined);
                                let numeric = builder.ins().bor(scalar, is_float);
                                let allowed = builder.ins().bor(numeric, empty);
                                let truth_block = builder.create_block();
                                let deopt_block = builder.create_block();
                                if opt_known_condition(builder.func, allowed, 8) == Some(true) {
                                    // A comparison result: the unreachable
                                    // recovery block is still emitted below.
                                    builder.ins().jump(truth_block, &[]);
                                } else {
                                    builder
                                        .ins()
                                        .brif(allowed, truth_block, &[], deopt_block, &[]);
                                }
                                builder.switch_to_block(deopt_block);
                                property_cache.flush(&mut builder);
                                for (index, vars) in arguments.iter().enumerate() {
                                    let value = opt_use(&mut builder, *vars);
                                    opt_store(&mut builder, arg_buf, index, value);
                                }
                                for (index, vars) in locals.iter().enumerate() {
                                    let value = opt_use(&mut builder, *vars);
                                    opt_store(&mut builder, var_buf, index, value);
                                }
                                for (index, vars) in stack.iter().take(depth).enumerate() {
                                    let value = opt_use(&mut builder, *vars);
                                    opt_store(&mut builder, stack_base, index, value);
                                }
                                // Resume before the branch opcode, so restore
                                // its popped condition as an owned interpreter
                                // stack value. This is the important generic
                                // refcounted deopt case (objects/strings).
                                opt_store(&mut builder, stack_base, depth, condition);
                                let start = builder.ins().load(
                                    pointer_type,
                                    MemFlags::new(),
                                    frame,
                                    layout.bytecode_start,
                                );
                                let resume = builder.ins().iadd_imm(start, i64::from(node.pc()));
                                builder
                                    .ins()
                                    .store(MemFlags::new(), resume, frame, layout.pc);
                                opt_own_stack_for_exit(
                                    &mut builder,
                                    frame,
                                    sret,
                                    stack_base,
                                    depth + 1,
                                    arguments.len() + locals.len(),
                                    &stack_provenance,
                                    &helper_signatures,
                                    pointer_type,
                                    layout,
                                )?;
                                emit_opt_exit(
                                    &mut builder,
                                    sret,
                                    qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
                                    Some(resume),
                                    pointer_type,
                                    node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
                                );
                                builder.switch_to_block(truth_block);
                                let integer = builder.ins().ireduce(types::I32, condition.payload);
                                let integer_truth =
                                    builder.ins().icmp_imm(IntCC::NotEqual, integer, 0);
                                let float_truth = super::helpers::emit_f64_bits_truthy(
                                    &mut builder,
                                    condition.payload,
                                );
                                let numeric_truth =
                                    builder.ins().select(is_float, float_truth, integer_truth);
                                let false_value = builder.ins().iconst(types::I8, 0);
                                let truth = builder.ins().select(empty, false_value, numeric_truth);
                                let target = opt_ir_edge(&blocks, block, node.branch_target())?;
                                let fallthrough = opt_ir_edge(
                                    &blocks,
                                    block,
                                    Some(next_block_pc(ir, block.start_pc())?),
                                )?;
                                if name.starts_with("if_false") {
                                    builder.ins().brif(truth, fallthrough, &[], target, &[]);
                                } else {
                                    builder.ins().brif(truth, target, &[], fallthrough, &[]);
                                }
                                terminated = true;
                            }
                            "goto" | "goto8" | "goto16" => {
                                let target = opt_ir_edge(&blocks, block, node.branch_target())?;
                                builder.ins().jump(target, &[]);
                                terminated = true;
                            }
                            "return" => {
                                depth = depth
                                    .checked_sub(1)
                                    .ok_or(CompileFailure::InvalidArtifact)?;
                                if !int32_loop
                                    && matches!(
                                        stack_provenance[depth],
                                        OptProvenance::Argument(_) | OptProvenance::Local(_)
                                    )
                                {
                                    // DONE transfers one owner to frame.result. A borrowed
                                    // local/argument cannot be moved: interpreter teardown
                                    // will separately release its root after native return.
                                    let borrowed = opt_use(&mut builder, stack[depth]);
                                    let primitive = builder.create_block();
                                    let materialize = builder.create_block();
                                    builder.set_cold_block(materialize);
                                    let ready = builder.create_block();
                                    super::call_cleanup::emit_cleanup_dispatch(
                                        &mut builder,
                                        frame,
                                        borrowed.tag,
                                        layout.flags,
                                        primitive,
                                        materialize,
                                    );
                                    builder.switch_to_block(primitive);
                                    builder.ins().jump(ready, &[]);
                                    builder.switch_to_block(materialize);
                                    for (index, vars) in arguments.iter().enumerate() {
                                        let value = opt_use(&mut builder, *vars);
                                        opt_store(&mut builder, arg_buf, index, value);
                                    }
                                    for (index, vars) in locals.iter().enumerate() {
                                        let value = opt_use(&mut builder, *vars);
                                        opt_store(&mut builder, var_buf, index, value);
                                    }
                                    for (index, vars) in stack.iter().take(depth + 1).enumerate() {
                                        let value = opt_use(&mut builder, *vars);
                                        opt_store(&mut builder, stack_base, index, value);
                                    }
                                    let start = builder.ins().load(
                                        pointer_type,
                                        MemFlags::new(),
                                        frame,
                                        layout.bytecode_start,
                                    );
                                    let pc = builder.ins().iadd_imm(start, i64::from(node.pc()));
                                    builder.ins().store(MemFlags::new(), pc, frame, layout.pc);
                                    opt_own_stack_for_helper(
                                        &mut builder,
                                        frame,
                                        sret,
                                        stack_base,
                                        depth + 1,
                                        arguments.len() + locals.len(),
                                        &stack_provenance,
                                        &helper_signatures,
                                        pointer_type,
                                        layout,
                                    )?;
                                    let result = opt_load(&mut builder, stack_base, depth);
                                    opt_define(&mut builder, stack[depth], result);
                                    builder.ins().jump(ready, &[]);
                                    builder.switch_to_block(ready);
                                }
                                let result = opt_use(&mut builder, stack[depth]);
                                opt_store_at(&mut builder, frame, layout.result, result);
                                opt_set_stack_top(
                                    &mut builder,
                                    frame,
                                    stack_base,
                                    depth,
                                    pointer_type,
                                    layout,
                                );
                                emit_opt_exit(
                                    &mut builder,
                                    sret,
                                    qjs::JSJitExitKind_JS_JIT_EXIT_DONE,
                                    None,
                                    pointer_type,
                                    0,
                                );
                                terminated = true;
                            }
                            "return_undef" => {
                                opt_store_at(&mut builder, frame, layout.result, undefined);
                                emit_opt_exit(
                                    &mut builder,
                                    sret,
                                    qjs::JSJitExitKind_JS_JIT_EXIT_DONE,
                                    None,
                                    pointer_type,
                                    0,
                                );
                                terminated = true;
                            }
                            "nop" => {}
                            _ => return Err(CompileFailure::UnsupportedOpcode),
                        }
                    }
                }
                if terminated {
                    break;
                }
            }
            if !terminated {
                if let Some(previous) = previous_node {
                    opt_bind_scalar_node(
                        &mut builder,
                        ir,
                        previous,
                        depth,
                        &env,
                        &mut scalar_values,
                    )?;
                }
                opt_define_scalar_edges(&mut builder, ir, block, &scalar_values, &phi_vars)?;
                let next = next_block_pc(ir, block.start_pc())?;
                emit_opt_array_loop_hoists(
                    &mut builder,
                    &env,
                    &stack_provenance,
                    depth,
                    &array_plan,
                    block.start_pc(),
                    next,
                    &mut hoisted_element_sources,
                    element_layout,
                    array_query,
                    ir,
                )?;
                builder
                    .ins()
                    .jump(opt_ir_edge(&blocks, block, Some(next))?, &[]);
            }
            block_exit_sources.insert(block.start_pc(), guarded_element_source);
        }
        builder.seal_all_blocks();
        builder.finalize();
    }
    let lowered_text = clif.display().to_string();
    if ir.blocks().iter().any(|block| block.is_loop_header()) {
        let explicit_sret_return = isa.triple().architecture
            == target_lexicon::Architecture::X86_64
            && isa.default_call_conv() == cranelift_codegen::isa::CallConv::SystemV;
        rematerialize_invariants_in_exits(
            &mut clif,
            &frame_buffers,
            pointer_type,
            explicit_sret_return,
        );
    }
    // Let Cranelift's identity-based constant-phi pass see loop-invariant
    // tags/flags that every edge sets to the same constant. The finalizer
    // canonicalizes again after its own cleanups; this bounded pre-pass keeps
    // the Tier 2 lowering's guarantee independent of that pipeline.
    const_canon::canonicalize_integer_constants(&mut clif, 4096);
    // Helpers that validate a stack map (CALL, GET_GLOBAL, GET_PROPERTY, ...)
    // are always invoked with map 0, so an artifact that calls anything must
    // publish the single helper stack map the runtime checks that id against.
    let calls_helpers = clif.layout.blocks().any(|block| {
        clif.layout
            .block_insts(block)
            .any(|inst| clif.dfg.insts[inst].opcode().is_call())
    });
    super::baseline::finalize_optimized_machine_with_lowered_text(
        isa,
        clif,
        control,
        calls_helpers,
        Some(lowered_text),
    )
}

/// The activation's invariant pointers: the `sret` exit record, the root
/// `JSJitExecFrame`, and its buffer pointers loaded once in the prologue.
struct FrameBufferLoads {
    sret: cranelift_codegen::ir::Value,
    frame: cranelift_codegen::ir::Value,
    loads: [(cranelift_codegen::ir::Value, i32); 3],
}

/// Keeps activation-invariant pointers out of loop register pressure.
///
/// Exit blocks (`return`: deopt, interrupt and normal exits) reload the root
/// frame's `arg_buf`/`var_buf`/`stack_base` from the frame and `sret` from a
/// prologue stash slot, instead of keeping the prologue values live across
/// every loop that can exit.
///
/// All four values are fixed for the whole activation: `sret` is an entry
/// parameter, `JSJitExecFrame` buffers are initialized before entry and no
/// helper rewrites them (the lowering itself already reuses the prologue
/// values after arbitrary helper calls). A reload therefore yields exactly the
/// prologue value. Exits run at most once per activation; only functions with
/// a loop are rewritten, since the stash costs one store per entry.
///
/// Exit blocks are outside every loop, so Cranelift cannot hoist the reload
/// back into a loop preheader. The stash is read through a distinct
/// `stack_addr` (slot offset 8, load offset -8) so GVN cannot merge it with
/// the prologue store's address and keep that address live instead.
///
/// With `explicit_sret_return` (x86-64 System V only), the exit record is
/// additionally passed as an ordinary pointer argument and every exit returns
/// the reloaded pointer explicitly. The machine-level contract is unchanged:
/// the caller's hidden pointer still arrives in the first integer argument
/// register and is returned in `rax`, as the ABI requires for
/// `JSJitExit (*)(JSJitExecFrame *)`. Cranelift's `StructReturn` purpose
/// would instead keep the incoming parameter live until every return.
fn rematerialize_invariants_in_exits(
    function: &mut cranelift_codegen::ir::Function,
    invariants: &FrameBufferLoads,
    pointer_type: cranelift_codegen::ir::Type,
    explicit_sret_return: bool,
) {
    use cranelift_codegen::cursor::{Cursor, FuncCursor};
    use cranelift_codegen::ir::{
        AbiParam, ArgumentPurpose, InstBuilder, MemFlags, Opcode, StackSlotData, StackSlotKind,
    };
    let Some(entry) = function.layout.entry_block() else {
        return;
    };
    let explicit_sret_return = explicit_sret_return
        && function.signature.returns.is_empty()
        && function
            .signature
            .params
            .first()
            .is_some_and(|param| param.purpose == ArgumentPurpose::StructReturn)
        && function.dfg.block_params(entry).first() == Some(&invariants.sret)
        && function.layout.blocks().all(|block| {
            function.layout.block_insts(block).all(|inst| {
                function.dfg.insts[inst].opcode() != Opcode::Return
                    || function.dfg.inst_args(inst).is_empty()
            })
        });
    function.dfg.resolve_all_aliases();
    let exits = function
        .layout
        .blocks()
        .filter(|&block| {
            block != entry
                && function
                    .layout
                    .last_inst(block)
                    .is_some_and(|inst| function.dfg.insts[inst].opcode() == Opcode::Return)
        })
        .collect::<Vec<_>>();
    let pointer_bytes = pointer_type.bytes();
    let Ok(stash_offset) = i32::try_from(pointer_bytes) else {
        return;
    };
    let mut sret_slot = None;
    for block in exits {
        let insts = function.layout.block_insts(block).collect::<Vec<_>>();
        let uses = |value| {
            insts
                .iter()
                .any(|&inst| function.dfg.inst_values(inst).any(|arg| arg == value))
        };
        let buffers = invariants.loads.map(|(value, _)| uses(value));
        let sret_used = explicit_sret_return || uses(invariants.sret);
        if !sret_used && !buffers.iter().any(|&used| used) {
            continue;
        }
        let slot = if sret_used {
            Some(*sret_slot.get_or_insert_with(|| {
                let align = u8::try_from(pointer_bytes.trailing_zeros()).unwrap_or(3);
                let slot = function.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    pointer_bytes * 2,
                    align,
                ));
                let mut cursor = FuncCursor::new(function);
                cursor.goto_first_insertion_point(entry);
                cursor.ins().stack_store(invariants.sret, slot, 0);
                slot
            }))
        } else {
            None
        };
        let mut cursor = FuncCursor::new(function);
        cursor.goto_first_insertion_point(block);
        let sret = slot.map(|slot| {
            let address = cursor.ins().stack_addr(pointer_type, slot, stash_offset);
            cursor
                .ins()
                .load(pointer_type, MemFlags::trusted(), address, -stash_offset)
        });
        let mut replacements = vec![(invariants.sret, sret)];
        for (index, &(original, offset)) in invariants.loads.iter().enumerate() {
            let reload = buffers[index].then(|| {
                cursor
                    .ins()
                    .load(pointer_type, MemFlags::new(), invariants.frame, offset)
            });
            replacements.push((original, reload));
        }
        for &inst in &insts {
            function.dfg.map_inst_values(inst, |value| {
                replacements
                    .iter()
                    .find_map(|&(original, replacement)| {
                        (original == value).then_some(replacement).flatten()
                    })
                    .unwrap_or(value)
            });
        }
        if let (true, Some(sret), Some(&last)) = (explicit_sret_return, sret, insts.last()) {
            function.dfg.replace(last).return_(&[sret]);
        }
    }
    if explicit_sret_return {
        let entry_returns = function
            .layout
            .block_insts(entry)
            .filter(|&inst| function.dfg.insts[inst].opcode() == Opcode::Return)
            .collect::<Vec<_>>();
        for inst in entry_returns {
            function.dfg.replace(inst).return_(&[invariants.sret]);
        }
        function.signature.params[0] = AbiParam::new(pointer_type);
        function.signature.returns.push(AbiParam::new(pointer_type));
    }
}

/// Emit an admitted effect-free region. Every failure reconstructs the
/// original caller before CALL; no callee effect may precede that recovery.
#[allow(clippy::too_many_arguments)]
fn emit_opt_inlined_call(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    ir: &OptimizedIr,
    node: &crate::ir::OptimizedNode,
    depth: usize,
    env: &OptEnv<'_>,
    provenance: &[OptProvenance],
    values: &std::collections::BTreeMap<crate::ir::ScalarValueId, OptPair>,
    proven: &[Option<crate::ir::ScalarNumericMode>],
    target_guarded: bool,
) -> Result<OptPair, CompileFailure> {
    use crate::ir::{ScalarBinaryOp, ScalarNumericMode, ScalarValue};
    use crate::runtime::FeedbackRepresentation;
    use cranelift_codegen::ir::{types, InstBuilder};
    use rquickjs_core::qjs;
    let graph = ir.scalar_graph();
    let call = graph
        .call(node.id())
        .ok_or(CompileFailure::InvalidArtifact)?;
    let region = call
        .inline
        .as_ref()
        .ok_or(CompileFailure::InvalidArtifact)?;
    let base = depth
        .checked_sub(call.arguments.len() + 1)
        .ok_or(CompileFailure::InvalidArtifact)?;
    if call.receiver.is_some()
        || !matches!(
            provenance[base],
            OptProvenance::Argument(_) | OptProvenance::Local(_)
        )
        || region.arguments.len() != call.arguments.len()
    {
        return Err(CompileFailure::InvalidArtifact);
    }
    let target = *values
        .get(&call.target)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let parameters = builder.create_block();
    let deopt = builder.create_block();
    builder.set_cold_block(deopt);
    if target_guarded {
        builder.ins().jump(parameters, &[]);
    } else {
        super::emit_guarded_direct_callee_identity(
            builder,
            target.tag,
            target.payload,
            super::DirectCalleeIdentity {
                object: region.object_identity,
                bytecode: region.bytecode_identity,
            },
            env.pointer_type,
            parameters,
            deopt,
        );
    }
    builder.switch_to_block(parameters);
    let mut condition = None;
    for (&value, &representation) in call.arguments.iter().zip(region.arguments.iter()) {
        let known = match representation {
            FeedbackRepresentation::Int32 => {
                proven.get(value.index()).copied().flatten() == Some(ScalarNumericMode::Int32)
            }
            FeedbackRepresentation::Bool => {
                matches!(graph.values()[value.index()], ScalarValue::Bool(_))
            }
            _ => return Err(CompileFailure::InvalidArtifact),
        };
        if known {
            continue;
        }
        let pair = *values.get(&value).ok_or(CompileFailure::InvalidArtifact)?;
        let tag = if representation == FeedbackRepresentation::Bool {
            qjs::JS_TAG_BOOL
        } else {
            qjs::JS_TAG_INT
        };
        let valid = opt_tag_is(builder, pair.tag, tag);
        condition = Some(match condition {
            Some(previous) => builder.ins().band(previous, valid),
            None => valid,
        });
    }
    if let Some(condition) = condition {
        let invoke = builder.create_block();
        builder.ins().brif(condition, invoke, &[], deopt, &[]);
        builder.switch_to_block(invoke);
    }
    let mut produced = std::collections::BTreeMap::new();
    for step in region.steps.iter() {
        let pair = match graph.values()[step.value.index()] {
            ScalarValue::Int32(value) => {
                let value = builder.ins().iconst(types::I32, i64::from(value));
                opt_int_pair(builder, env, value)
            }
            ScalarValue::Bool(value) => {
                let value = builder.ins().iconst(types::I8, i64::from(value));
                opt_bool_pair(builder, env, value)
            }
            ScalarValue::Binary {
                mode: ScalarNumericMode::Int32,
                op,
                lhs,
                rhs,
            } => {
                let state = step.frame.as_ref().ok_or(CompileFailure::InvalidArtifact)?;
                if state.parent != Some(node.id())
                    || state.arguments != call.arguments
                    || state.stack.len() < 2
                    || state.stack[state.stack.len() - 2..] != [lhs, rhs]
                {
                    return Err(CompileFailure::InvalidArtifact);
                }
                let lhs = *produced
                    .get(&lhs)
                    .or_else(|| values.get(&lhs))
                    .ok_or(CompileFailure::InvalidArtifact)?;
                let rhs = *produced
                    .get(&rhs)
                    .or_else(|| values.get(&rhs))
                    .ok_or(CompileFailure::InvalidArtifact)?;
                let lhs = opt_i32(builder, env, lhs);
                let rhs = opt_i32(builder, env, rhs);
                let (value, failure) = match op {
                    ScalarBinaryOp::Add => builder.ins().sadd_overflow(lhs, rhs),
                    ScalarBinaryOp::Sub => builder.ins().ssub_overflow(lhs, rhs),
                    _ => return Err(CompileFailure::InvalidArtifact),
                };
                let pass = builder.create_block();
                builder.ins().brif(failure, deopt, &[], pass, &[]);
                builder.switch_to_block(pass);
                opt_int_pair(builder, env, value)
            }
            _ => return Err(CompileFailure::InvalidArtifact),
        };
        produced.insert(step.value, pair);
    }
    let result = *produced
        .get(&region.result)
        .or_else(|| values.get(&region.result))
        .ok_or(CompileFailure::InvalidArtifact)?;
    let done = builder.create_block();
    builder.ins().jump(done, &[]);
    builder.switch_to_block(deopt);
    emit_opt_deopt(
        builder,
        env,
        provenance,
        depth,
        node.pc(),
        node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
    )?;
    builder.switch_to_block(done);
    Ok(result)
}

/// Restore compiler bindings from semantic SSA without writing interpreter memory.
fn opt_restore_scalar_frame(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    ir: &OptimizedIr,
    node: &crate::ir::OptimizedNode,
    depth: usize,
    env: &OptEnv<'_>,
    values: &std::collections::BTreeMap<crate::ir::ScalarValueId, OptPair>,
) -> Result<(), CompileFailure> {
    let graph = ir.scalar_graph();
    let state = graph
        .frame_state_for_node(node.id())
        .ok_or(CompileFailure::InvalidArtifact)?;
    if state.pc != node.pc()
        || state.stack.len() != depth
        || state.arguments.len() != env.arguments.len()
        || state.locals.len() != env.locals.len()
        || depth > env.stack.len()
    {
        return Err(CompileFailure::InvalidArtifact);
    }
    for (ids, variables) in [
        (state.arguments.as_ref(), env.arguments),
        (state.locals.as_ref(), env.locals),
        (state.stack.as_ref(), &env.stack[..depth]),
    ] {
        for (&id, &vars) in ids.iter().zip(variables) {
            opt_define(
                builder,
                vars,
                *values.get(&id).ok_or(CompileFailure::InvalidArtifact)?,
            );
        }
    }
    Ok(())
}

/// Restore compiler-variable bindings from the pre-effect state and consume
/// the graph's remaining type checks. Actual frame stores stay on cold exits.
#[allow(clippy::too_many_arguments)]
fn opt_prepare_scalar_operation(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    ir: &OptimizedIr,
    node: &crate::ir::OptimizedNode,
    depth: usize,
    env: &OptEnv<'_>,
    provenance: &[OptProvenance],
    values: &std::collections::BTreeMap<crate::ir::ScalarValueId, OptPair>,
    proven_numeric: &[Option<crate::ir::ScalarNumericMode>],
) -> Result<(OptPair, Option<OptPair>), CompileFailure> {
    use crate::ir::ScalarNumericMode;
    use cranelift_codegen::ir::InstBuilder;
    use rquickjs_core::qjs;
    let graph = ir.scalar_graph();
    opt_restore_scalar_frame(builder, ir, node, depth, env, values)?;
    let (mode, lhs, rhs) = if let Some((mode, input, _)) = graph.update_operation(node.id()) {
        (mode, input, None)
    } else if let Some((_, lhs, rhs)) = graph.bitwise_operation(node.id()) {
        (ScalarNumericMode::Int32, lhs, Some(rhs))
    } else if let Some((_, mode)) = graph.binary_operation(node.id()) {
        let (lhs, rhs) = graph
            .binary_operands(node.id())
            .ok_or(CompileFailure::InvalidArtifact)?;
        (mode, lhs, Some(rhs))
    } else {
        let (_, lhs, rhs) = graph
            .comparison(node.id())
            .ok_or(CompileFailure::InvalidArtifact)?;
        (ScalarNumericMode::Number, lhs, Some(rhs))
    };
    let checks = graph.checks_for_node(node.id());
    if checks.len() != 1 + usize::from(rhs.is_some()) {
        return Err(CompileFailure::InvalidArtifact);
    }
    let mut passed = None;
    for (check, operand) in checks.iter().zip([Some(lhs), rhs].into_iter().flatten()) {
        if check.frame_state_node != node.id() || check.mode != mode || check.value != operand {
            return Err(CompileFailure::InvalidArtifact);
        }
        // Representation analysis includes every CFG entry and backedge and
        // uses each argument's actual entry guard; poll aliases additionally
        // require the register-preserving raw loop path.
        // Skip the branch itself: folding tag comparisons to true still leaves
        // constant branches and cold materialization blocks in machine code.
        if check.eliminated
            || proven_numeric
                .get(check.value.index())
                .copied()
                .flatten()
                .is_some_and(|proven| {
                    check.mode == proven || check.mode == ScalarNumericMode::Number
                })
        {
            continue;
        }
        let pair = *values
            .get(&check.value)
            .ok_or(CompileFailure::InvalidArtifact)?;
        let condition = match check.mode {
            ScalarNumericMode::Int32 => opt_tag_is(builder, pair.tag, qjs::JS_TAG_INT),
            ScalarNumericMode::Float64 => opt_tag_is(builder, pair.tag, qjs::JS_TAG_FLOAT64),
            ScalarNumericMode::Number => {
                let int = opt_tag_is(builder, pair.tag, qjs::JS_TAG_INT);
                let float = opt_tag_is(builder, pair.tag, qjs::JS_TAG_FLOAT64);
                builder.ins().bor(int, float)
            }
        };
        passed = Some(match passed {
            Some(previous) => builder.ins().band(previous, condition),
            None => condition,
        });
    }
    if let Some(passed) = passed {
        emit_opt_guard_branch(
            builder,
            env,
            provenance,
            depth,
            node.pc(),
            node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
            passed,
        )?;
    }
    Ok((
        *values.get(&lhs).ok_or(CompileFailure::InvalidArtifact)?,
        rhs.map(|rhs| {
            values
                .get(&rhs)
                .copied()
                .ok_or(CompileFailure::InvalidArtifact)
        })
        .transpose()?,
    ))
}

/// Bind a completed bridge operation to its graph definitions. Numeric
/// operations and CFG edges subsequently read those definitions by ValueId.
fn opt_bind_scalar_node(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    ir: &OptimizedIr,
    node: u32,
    depth: usize,
    env: &OptEnv<'_>,
    values: &mut std::collections::BTreeMap<crate::ir::ScalarValueId, OptPair>,
) -> Result<(), CompileFailure> {
    let outputs = ir.scalar_graph().outputs_for_node(node);
    let base = depth
        .checked_sub(outputs.len())
        .ok_or(CompileFailure::InvalidArtifact)?;
    for (index, &value) in outputs.iter().enumerate() {
        values.insert(value, opt_use(builder, env.stack[base + index]));
    }
    for &(slot, value) in ir.scalar_graph().frame_definitions_for_node(node) {
        let variables = match slot {
            crate::ir::FrameSlot::Argument(index) => env.arguments[usize::from(index)],
            crate::ir::FrameSlot::Local(index) => env.locals[usize::from(index)],
            crate::ir::FrameSlot::Stack(_) => return Err(CompileFailure::InvalidArtifact),
        };
        values.insert(value, opt_use(builder, variables));
    }
    Ok(())
}

/// Each target has distinct SSA variables. Defining both successors before a
/// conditional branch is safe: only the taken edge reaches that target's use.
/// Cranelift merges these definitions according to the actual machine CFG.
fn opt_define_scalar_edges(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    ir: &OptimizedIr,
    block: &crate::ir::OptimizedBlock,
    values: &std::collections::BTreeMap<crate::ir::ScalarValueId, OptPair>,
    variables: &std::collections::BTreeMap<crate::ir::ScalarValueId, OptVars>,
) -> Result<(), CompileFailure> {
    for &successor in block.successors() {
        for &(_, target) in ir.scalar_graph().inputs_for_block(successor) {
            let crate::ir::ScalarValue::Phi { inputs, .. } =
                &ir.scalar_graph().values()[target.index()]
            else {
                return Err(CompileFailure::InvalidArtifact);
            };
            let source = inputs
                .iter()
                .find(|input| input.predecessor == Some(block.start_pc()))
                .ok_or(CompileFailure::InvalidArtifact)?
                .value;
            let pair = *values.get(&source).ok_or(CompileFailure::InvalidArtifact)?;
            opt_define(builder, variables[&target], pair);
        }
    }
    Ok(())
}

/// Builds the secondary, scalar-only entry used by monomorphic native call
/// edges. Its ABI is `(output:*scalar, unboxed arguments...) -> status:i32`;
/// status zero stores the numeric result, and non-zero asks the caller to retry at
/// the CALL bytecode.  The entry deliberately has no `JSValue`, frame, or
/// helper parameters, so neither arguments nor the result can be boxed on the
/// compiled-to-compiled fast path.
pub(crate) fn lower_direct_call_machine(
    isa: &cranelift_codegen::isa::OwnedTargetIsa,
    function: &VerifiedFunction,
    signature: &crate::runtime::BoundedSpecializationSignature,
    feedback: Option<&crate::runtime::FeedbackSnapshot>,
    control: Option<&CompileControl>,
) -> Result<super::baseline::RelocatableCode, CompileFailure> {
    use crate::runtime::FeedbackRepresentation;
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, AbiParam, Function, InstBuilder, Signature};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

    if signature.function().id != function.snapshot().function_id()
        || signature.function().generation != function.snapshot().generation()
        || signature.arity() != usize::from(function.snapshot().arg_count())
    {
        return Err(CompileFailure::InvalidArtifact);
    }
    if signature
        .arguments()
        .contains(&FeedbackRepresentation::HeapRef)
    {
        return super::tagged_call_link::lower_target_only_linked_leaf(
            isa, function, signature, feedback, control,
        );
    }
    if signature
        .arguments()
        .contains(&FeedbackRepresentation::Bool)
    {
        return lower_direct_bool_leaf(isa, function, signature, control);
    }
    let representation = signature.result();
    if signature
        .arguments()
        .iter()
        .any(|arg| *arg != representation)
    {
        return Err(CompileFailure::InvalidArtifact);
    }
    let scalar = match representation {
        FeedbackRepresentation::Int32 => types::I32,
        FeedbackRepresentation::Float64 => types::F64,
        FeedbackRepresentation::Bool | FeedbackRepresentation::HeapRef => {
            return Err(CompileFailure::InvalidArtifact)
        }
    };
    let mut abi = Signature::new(isa.default_call_conv());
    abi.params.push(AbiParam::new(isa.pointer_type()));
    abi.params
        .extend((0..signature.arity()).map(|_| AbiParam::new(scalar)));
    abi.returns.push(AbiParam::new(types::I32));
    let mut clif = Function::with_name_signature(Default::default(), abi);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut clif, &mut context);
        let entry = builder.create_block();
        builder.append_block_params_for_function_params(entry);
        builder.switch_to_block(entry);
        let output = builder.block_params(entry)[0];
        let arguments = builder.block_params(entry)[1..].to_vec();
        let mut stack = Vec::new();
        for instruction in function.instructions() {
            let name = instruction.opcode().name();
            let bytes = instruction.bytes();
            let push_int =
                |value: i64,
                 builder: &mut FunctionBuilder<'_>,
                 stack: &mut Vec<cranelift_codegen::ir::Value>| {
                    stack.push(match representation {
                        FeedbackRepresentation::Int32 => builder.ins().iconst(types::I32, value),
                        FeedbackRepresentation::Float64 => builder.ins().f64const(value as f64),
                        FeedbackRepresentation::Bool | FeedbackRepresentation::HeapRef => {
                            unreachable!("direct calls are scalar-only")
                        }
                    });
                };
            match name {
                "nop" => {}
                "push_minus1" => push_int(-1, &mut builder, &mut stack),
                "push_i8" => push_int(i64::from(bytes[1] as i8), &mut builder, &mut stack),
                "push_i16" => push_int(
                    i64::from(i16::from_le_bytes([bytes[1], bytes[2]])),
                    &mut builder,
                    &mut stack,
                ),
                "push_i32" => push_int(
                    i64::from(i32::from_le_bytes(
                        bytes[1..5]
                            .try_into()
                            .map_err(|_| CompileFailure::InvalidArtifact)?,
                    )),
                    &mut builder,
                    &mut stack,
                ),
                "push_0" | "push_1" | "push_2" | "push_3" | "push_4" | "push_5" | "push_6"
                | "push_7" => push_int(
                    i64::from(name.as_bytes()[5] - b'0'),
                    &mut builder,
                    &mut stack,
                ),
                n if opt_index(n, bytes, "get_arg")?.is_some() => {
                    let index = opt_index(n, bytes, "get_arg")?.unwrap();
                    let parameter = arguments
                        .get(index)
                        .copied()
                        .ok_or(CompileFailure::UnsupportedOpcode)?;
                    stack.push(parameter);
                }
                "add" | "sub" | "mul" | "div" => {
                    let rhs = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    let lhs = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    let value = match representation {
                        FeedbackRepresentation::Float64 => match name {
                            "add" => builder.ins().fadd(lhs, rhs),
                            "sub" => builder.ins().fsub(lhs, rhs),
                            "mul" => builder.ins().fmul(lhs, rhs),
                            _ => builder.ins().fdiv(lhs, rhs),
                        },
                        FeedbackRepresentation::Int32 => {
                            if name == "div" {
                                return Err(CompileFailure::UnsupportedOpcode);
                            }
                            let pair = match name {
                                "add" => builder.ins().sadd_overflow(lhs, rhs),
                                "sub" => builder.ins().ssub_overflow(lhs, rhs),
                                _ => builder.ins().smul_overflow(lhs, rhs),
                            };
                            let (result, overflow_flag) = pair;
                            let ok = builder.create_block();
                            builder.append_block_param(ok, types::I32);
                            let overflow =
                                builder.ins().icmp_imm(IntCC::NotEqual, overflow_flag, 0);
                            let fail = builder.create_block();
                            builder.ins().brif(overflow, fail, &[], ok, &[result]);
                            builder.switch_to_block(fail);
                            let status = builder.ins().iconst(types::I32, 1);
                            builder.ins().return_(&[status]);
                            builder.switch_to_block(ok);
                            builder.block_params(ok)[0]
                        }
                        FeedbackRepresentation::Bool | FeedbackRepresentation::HeapRef => {
                            unreachable!("direct calls are scalar-only")
                        }
                    };
                    stack.push(value);
                }
                "neg" => {
                    let value = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    let negated = match representation {
                        FeedbackRepresentation::Float64 => builder.ins().fneg(value),
                        FeedbackRepresentation::Int32 => {
                            // `-0` and `-INT32_MIN` are Float64 results the
                            // scalar Int32 ABI cannot return.
                            let zero = builder.ins().icmp_imm(IntCC::Equal, value, 0);
                            let min =
                                builder
                                    .ins()
                                    .icmp_imm(IntCC::Equal, value, i64::from(i32::MIN));
                            let unrepresentable = builder.ins().bor(zero, min);
                            let ok = builder.create_block();
                            let fail = builder.create_block();
                            builder.ins().brif(unrepresentable, fail, &[], ok, &[]);
                            builder.switch_to_block(fail);
                            let status = builder.ins().iconst(types::I32, 1);
                            builder.ins().return_(&[status]);
                            builder.switch_to_block(ok);
                            builder.ins().ineg(value)
                        }
                        FeedbackRepresentation::Bool | FeedbackRepresentation::HeapRef => {
                            unreachable!("direct calls are scalar-only")
                        }
                    };
                    stack.push(negated);
                }
                "mod" => {
                    if representation != FeedbackRepresentation::Int32 {
                        return Err(CompileFailure::UnsupportedOpcode);
                    }
                    let rhs = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    let lhs = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    // A non-negative dividend and non-zero divisor make the
                    // Int32 remainder exact (no `-0`, no trap).
                    let non_negative =
                        builder
                            .ins()
                            .icmp_imm(IntCC::SignedGreaterThanOrEqual, lhs, 0);
                    let non_zero = builder.ins().icmp_imm(IntCC::NotEqual, rhs, 0);
                    let exact = builder.ins().band(non_negative, non_zero);
                    let ok = builder.create_block();
                    let fail = builder.create_block();
                    builder.ins().brif(exact, ok, &[], fail, &[]);
                    builder.switch_to_block(fail);
                    let status = builder.ins().iconst(types::I32, 1);
                    builder.ins().return_(&[status]);
                    builder.switch_to_block(ok);
                    let remainder = builder.ins().srem(lhs, rhs);
                    stack.push(remainder);
                }
                "return" => {
                    let result = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                    if !stack.is_empty() {
                        return Err(CompileFailure::InvalidArtifact);
                    }
                    let status = builder.ins().iconst(types::I32, 0);
                    builder
                        .ins()
                        .store(cranelift_codegen::ir::MemFlags::new(), result, output, 0);
                    builder.ins().return_(&[status]);
                }
                _ => return Err(CompileFailure::UnsupportedOpcode),
            }
        }
        builder.seal_all_blocks();
        builder.finalize();
    }
    super::baseline::finalize_optimized_machine(isa, clif, control, false)
}

/// A deliberately small, effect-free leaf ABI. Bool is accepted only as a
/// branch condition; numeric operations and returns require Int32. Every CFG
/// edge is forward and carries an empty operand stack, so no frame, ownership
/// merge, safepoint, or nested deoptimization state is needed. A failed checked
/// operation returns before storing output and the caller retries the CALL.
fn lower_direct_bool_leaf(
    isa: &cranelift_codegen::isa::OwnedTargetIsa,
    function: &VerifiedFunction,
    signature: &crate::runtime::BoundedSpecializationSignature,
    control: Option<&CompileControl>,
) -> Result<super::baseline::RelocatableCode, CompileFailure> {
    use crate::runtime::FeedbackRepresentation::{Bool, Int32};
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, AbiParam, Function, InstBuilder, MemFlags, Signature};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

    if signature.result() != Int32
        || signature
            .arguments()
            .iter()
            .any(|arg| !matches!(arg, Int32 | Bool))
        || !function.snapshot().exception_map().is_empty()
    {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    let cfg = function.control_flow_graph();
    if cfg
        .blocks()
        .iter()
        .any(|block| block.successors().iter().any(|pc| *pc <= block.start_pc()))
    {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    let mut abi = Signature::new(isa.default_call_conv());
    abi.params.push(AbiParam::new(isa.pointer_type()));
    abi.params.extend(
        signature
            .arguments()
            .iter()
            .map(|_| AbiParam::new(types::I32)),
    );
    abi.returns.push(AbiParam::new(types::I32));
    let mut clif = Function::with_name_signature(Default::default(), abi);
    let mut context = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut clif, &mut context);
        let blocks = cfg
            .blocks()
            .iter()
            .map(|block| (block.start_pc(), builder.create_block()))
            .collect::<std::collections::BTreeMap<_, _>>();
        let entry = *blocks.get(&0).ok_or(CompileFailure::InvalidArtifact)?;
        builder.append_block_params_for_function_params(entry);
        let output = builder.block_params(entry)[0];
        let arguments = builder.block_params(entry)[1..].to_vec();
        for block in cfg.blocks() {
            builder.switch_to_block(blocks[&block.start_pc()]);
            let mut stack = Vec::new();
            let mut terminated = false;
            for instruction in &function.instructions()[block.instruction_range()] {
                if terminated {
                    return Err(CompileFailure::InvalidArtifact);
                }
                let name = instruction.opcode().name();
                let bytes = instruction.bytes();
                let constant = match name {
                    "push_minus1" => Some(-1),
                    "push_i8" => Some(i64::from(bytes[1] as i8)),
                    "push_i16" => Some(i64::from(i16::from_le_bytes([bytes[1], bytes[2]]))),
                    "push_i32" => Some(i64::from(i32::from_le_bytes(
                        bytes[1..5]
                            .try_into()
                            .map_err(|_| CompileFailure::InvalidArtifact)?,
                    ))),
                    "push_0" | "push_1" | "push_2" | "push_3" | "push_4" | "push_5" | "push_6"
                    | "push_7" => Some(i64::from(name.as_bytes()[5] - b'0')),
                    _ => None,
                };
                if let Some(value) = constant {
                    stack.push((builder.ins().iconst(types::I32, value), Int32));
                    continue;
                }
                match name {
                    "nop" => {}
                    n if opt_index(n, bytes, "get_arg")?.is_some() => {
                        let index = opt_index(n, bytes, "get_arg")?.unwrap();
                        stack.push((
                            *arguments
                                .get(index)
                                .ok_or(CompileFailure::InvalidArtifact)?,
                            signature.arguments()[index],
                        ));
                    }
                    "add" | "sub" => {
                        let (rhs, rhs_type) = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        let (lhs, lhs_type) = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        if lhs_type != Int32 || rhs_type != Int32 {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        let (result, overflow) = if name == "add" {
                            builder.ins().sadd_overflow(lhs, rhs)
                        } else {
                            builder.ins().ssub_overflow(lhs, rhs)
                        };
                        let ok = builder.create_block();
                        let fail = builder.create_block();
                        builder.ins().brif(overflow, fail, &[], ok, &[]);
                        builder.switch_to_block(fail);
                        let status = builder.ins().iconst(types::I32, 1);
                        builder.ins().return_(&[status]);
                        builder.switch_to_block(ok);
                        stack.push((result, Int32));
                    }
                    "if_false8" | "if_true8" | "if_false" | "if_true" => {
                        let (condition, kind) =
                            stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        if kind != Bool || !stack.is_empty() {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        let target_pc = u32::try_from(
                            instruction
                                .branch_target()
                                .ok_or(CompileFailure::InvalidArtifact)?,
                        )
                        .map_err(|_| CompileFailure::InvalidArtifact)?;
                        let target = *blocks
                            .get(&target_pc)
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let fallthrough = *blocks
                            .get(&block.end_pc())
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        let truth = builder.ins().icmp_imm(IntCC::NotEqual, condition, 0);
                        if name.starts_with("if_false") {
                            builder.ins().brif(truth, fallthrough, &[], target, &[]);
                        } else {
                            builder.ins().brif(truth, target, &[], fallthrough, &[]);
                        }
                        terminated = true;
                    }
                    "goto" | "goto8" | "goto16" => {
                        if !stack.is_empty() {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        let target_pc = u32::try_from(
                            instruction
                                .branch_target()
                                .ok_or(CompileFailure::InvalidArtifact)?,
                        )
                        .map_err(|_| CompileFailure::InvalidArtifact)?;
                        let target = *blocks
                            .get(&target_pc)
                            .ok_or(CompileFailure::InvalidArtifact)?;
                        builder.ins().jump(target, &[]);
                        terminated = true;
                    }
                    "return" => {
                        let (result, kind) = stack.pop().ok_or(CompileFailure::InvalidArtifact)?;
                        if kind != Int32 || !stack.is_empty() {
                            return Err(CompileFailure::UnsupportedOpcode);
                        }
                        builder.ins().store(MemFlags::new(), result, output, 0);
                        let status = builder.ins().iconst(types::I32, 0);
                        builder.ins().return_(&[status]);
                        terminated = true;
                    }
                    _ => return Err(CompileFailure::UnsupportedOpcode),
                }
            }
            if !terminated {
                if !stack.is_empty() || block.successors().len() != 1 {
                    return Err(CompileFailure::UnsupportedOpcode);
                }
                builder.ins().jump(blocks[&block.successors()[0]], &[]);
            }
        }
        builder.seal_all_blocks();
        builder.finalize();
    }
    super::baseline::finalize_optimized_machine(isa, clif, control, false)
}

fn opt_define(builder: &mut cranelift_frontend::FunctionBuilder<'_>, vars: OptVars, pair: OptPair) {
    builder.def_var(vars.payload, pair.payload);
    builder.def_var(vars.tag, pair.tag);
}
fn opt_use(builder: &mut cranelift_frontend::FunctionBuilder<'_>, vars: OptVars) -> OptPair {
    OptPair {
        payload: builder.use_var(vars.payload),
        tag: builder.use_var(vars.tag),
    }
}
fn opt_load(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    base: cranelift_codegen::ir::Value,
    index: usize,
) -> OptPair {
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    let offset = i32::try_from(index * 16).expect("verified frame");
    OptPair {
        payload: builder
            .ins()
            .load(types::I64, MemFlags::new(), base, offset),
        tag: builder
            .ins()
            .load(types::I64, MemFlags::new(), base, offset + 8),
    }
}
fn opt_store(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    base: cranelift_codegen::ir::Value,
    index: usize,
    pair: OptPair,
) {
    opt_store_at(
        builder,
        base,
        i32::try_from(index * 16).expect("verified frame"),
        pair,
    )
}
fn opt_store_at(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    base: cranelift_codegen::ir::Value,
    offset: i32,
    pair: OptPair,
) {
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    let payload = if builder.func.dfg.value_type(pair.payload) == types::I32 {
        builder.ins().sextend(types::I64, pair.payload)
    } else {
        pair.payload
    };
    builder.ins().store(MemFlags::new(), payload, base, offset);
    builder
        .ins()
        .store(MemFlags::new(), pair.tag, base, offset + 8);
}
fn opt_f64(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    pair: OptPair,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    let is_int = builder.ins().icmp_imm(
        IntCC::Equal,
        pair.tag,
        i64::from(rquickjs_core::qjs::JS_TAG_INT),
    );
    let int = builder.ins().ireduce(types::I32, pair.payload);
    let intf = builder.ins().fcvt_from_sint(types::F64, int);
    let float = builder
        .ins()
        .bitcast(types::F64, MemFlags::new(), pair.payload);
    builder.ins().select(is_int, intf, float)
}
fn opt_u16(bytes: &[u8]) -> Result<usize, CompileFailure> {
    let raw = bytes.get(1..3).ok_or(CompileFailure::InvalidArtifact)?;
    Ok(usize::from(u16::from_le_bytes([raw[0], raw[1]])))
}
fn opt_index(name: &str, bytes: &[u8], prefix: &str) -> Result<Option<usize>, CompileFailure> {
    if !name.starts_with(prefix) {
        return Ok(None);
    }
    // QuickJS's `*_loc8` opcodes carry an 8-bit operand; the trailing 8 is
    // the operand width, not the fixed local index used by `*_loc0..3`.
    if name.strip_prefix(prefix) == Some("8") {
        return bytes
            .get(1)
            .copied()
            .map(usize::from)
            .map(Some)
            .ok_or(CompileFailure::InvalidArtifact);
    }
    if let Some(last) = name.as_bytes().last().filter(|last| last.is_ascii_digit()) {
        return Ok(Some(usize::from(*last - b'0')));
    }
    Ok(Some(opt_u16(bytes)?))
}

/// Frame-level values and variable tables every guarded lowering arm needs to
/// spill state and leave through an exact deoptimization exit.
#[derive(Clone, Copy)]
struct OptEnv<'a> {
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    arg_buf: cranelift_codegen::ir::Value,
    var_buf: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    pointer_type: cranelift_codegen::ir::Type,
    payload_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
    int32_loop: bool,
    arguments: &'a [OptVars],
    locals: &'a [OptVars],
    stack: &'a [OptVars],
    helper_signatures: &'a [cranelift_codegen::ir::SigRef],
    property_cache: &'a property_cache::PropertyCache,
}

/// Spills the complete frame, records the resume pc and leaves through the
/// exact deoptimization exit of `guard`. `depth` is the operand-stack depth
/// before the deoptimizing instruction pops its inputs, so the interpreter
/// re-executes it with every operand materialized and no effect applied.
fn emit_opt_deopt(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &[OptProvenance],
    depth: usize,
    pc: u32,
    guard: u32,
) -> Result<(), CompileFailure> {
    use cranelift_codegen::ir::{InstBuilder, MemFlags};
    env.property_cache.flush(builder);
    for (index, vars) in env.arguments.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.arg_buf, index, value);
    }
    for (index, vars) in env.locals.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.var_buf, index, value);
    }
    for (index, vars) in env.stack.iter().take(depth).enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.stack_base, index, value);
    }
    opt_set_stack_top(
        builder,
        env.frame,
        env.stack_base,
        depth,
        env.pointer_type,
        env.layout,
    );
    let start = builder.ins().load(
        env.pointer_type,
        MemFlags::new(),
        env.frame,
        env.layout.bytecode_start,
    );
    let resume = builder.ins().iadd_imm(start, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), resume, env.frame, env.layout.pc);
    // The raw-i32 loop shape admits only unboxed scalars (Int32 entry guard,
    // numeric constants, no calls), so the values just stored are already
    // exact interpreter stack owners. Every other shape may alias borrowed
    // arguments or locals and must materialize an owner per slot.
    if !env.int32_loop {
        opt_own_stack_for_exit(
            builder,
            env.frame,
            env.sret,
            env.stack_base,
            depth,
            env.arguments.len() + env.locals.len(),
            provenance,
            env.helper_signatures,
            env.pointer_type,
            env.layout,
        )?;
    }
    emit_opt_exit(
        builder,
        env.sret,
        rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
        Some(resume),
        env.pointer_type,
        guard,
    );
    Ok(())
}

/// Branches to a fresh pass block when `condition` holds and otherwise to an
/// exact deoptimization exit; the builder is left positioned on the pass
/// block. A condition that the emitted instructions already prove true (for
/// example a tag test of a constant tag carried by a guarded value) emits no
/// branch or exit: Cranelift folds such tests to constants but never removes
/// the resulting constant branches from the hot path.
fn emit_opt_guard_branch(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &[OptProvenance],
    depth: usize,
    pc: u32,
    guard: u32,
    condition: cranelift_codegen::ir::Value,
) -> Result<(), CompileFailure> {
    use cranelift_codegen::ir::InstBuilder;
    // A condition folded from literal constants holds on every execution:
    // the deoptimization edge is unreachable, so emit no branch at all.
    if opt_known_condition(builder.func, condition, 8) == Some(true)
        || opt_known_const(builder, condition).is_some_and(|known| known != 0)
    {
        return Ok(());
    }
    let pass = builder.create_block();
    let deopt = builder.create_block();
    // Deoptimization exits are cold: keep them out of the hot fallthrough
    // path so a guarded loop body stays straight-line machine code.
    builder.set_cold_block(deopt);
    builder.ins().brif(condition, pass, &[], deopt, &[]);
    // Both blocks are fresh and this branch is their only predecessor.
    // Sealing lets variable reads resolve to dominating definitions
    // (including literal tags) instead of placeholder block parameters.
    builder.seal_block(deopt);
    builder.seal_block(pass);
    builder.switch_to_block(deopt);
    emit_opt_deopt(builder, env, provenance, depth, pc, guard)?;
    builder.switch_to_block(pass);
    Ok(())
}

/// Evaluates a canonical 0/1 boolean built only from `iconst 0/1`,
/// `icmp_imm eq/ne` of a constant and `band`/`bor` of such booleans. `None` means "not statically known";
/// callers must then emit the dynamic check. `fuel` bounds the recursion.
fn opt_known_condition(
    func: &cranelift_codegen::ir::Function,
    value: cranelift_codegen::ir::Value,
    fuel: u32,
) -> Option<bool> {
    use cranelift_codegen::ir::{condcodes::IntCC, InstructionData, Opcode, ValueDef};
    let fuel = fuel.checked_sub(1)?;
    let ValueDef::Result(inst, 0) = func.dfg.value_def(func.dfg.resolve_aliases(value)) else {
        return None;
    };
    let constant = |value: cranelift_codegen::ir::Value| -> Option<i64> {
        let value = func.dfg.resolve_aliases(value);
        let ValueDef::Result(inst, 0) = func.dfg.value_def(value) else {
            return None;
        };
        match func.dfg.insts[inst] {
            InstructionData::UnaryImm {
                opcode: Opcode::Iconst,
                imm,
            } => {
                let bits = func.dfg.value_type(value).bits();
                let raw = imm.bits();
                Some(if bits >= 64 {
                    raw
                } else {
                    raw & ((1i64 << bits) - 1)
                })
            }
            _ => None,
        }
    };
    match func.dfg.insts[inst] {
        InstructionData::UnaryImm {
            opcode: Opcode::Iconst,
            ..
        } => match constant(value)? {
            // Only canonical booleans: band/bor below are bitwise.
            0 => Some(false),
            1 => Some(true),
            _ => None,
        },
        InstructionData::IntCompareImm {
            opcode: Opcode::IcmpImm,
            cond,
            arg,
            imm,
        } => {
            let bits = func.dfg.value_type(arg).bits();
            let lhs = constant(arg)?;
            let rhs = if bits >= 64 {
                imm.bits()
            } else {
                imm.bits() & ((1i64 << bits) - 1)
            };
            match cond {
                IntCC::Equal => Some(lhs == rhs),
                IntCC::NotEqual => Some(lhs != rhs),
                _ => None,
            }
        }
        InstructionData::Binary {
            opcode: opcode @ (Opcode::Band | Opcode::Bor),
            args: [lhs, rhs],
        } => {
            let lhs = opt_known_condition(func, lhs, fuel);
            let rhs = opt_known_condition(func, rhs, fuel);
            match (opcode, lhs, rhs) {
                (Opcode::Band, Some(false), _) | (Opcode::Band, _, Some(false)) => Some(false),
                (Opcode::Band, Some(true), Some(true)) => Some(true),
                (Opcode::Bor, Some(true), _) | (Opcode::Bor, _, Some(true)) => Some(true),
                (Opcode::Bor, Some(false), Some(false)) => Some(false),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Exact compile-time value of an integer SSA value already emitted into the
/// current function: an `iconst`, or an `icmp_imm`/`icmp` equality, `band`,
/// `bor`, `uextend`, `sextend` or `ireduce` of such values. Every other
/// definition (block parameters, loads, calls) is unknown. Because this folds only instructions whose operands are literal
/// constants, a condition reported as known holds on every execution, so a
/// guard on it can be omitted without changing any taken path. Results are
/// sign-extended from the value's type width.
fn opt_known_const(
    builder: &cranelift_frontend::FunctionBuilder<'_>,
    value: cranelift_codegen::ir::Value,
) -> Option<i64> {
    fn fold(
        dfg: &cranelift_codegen::ir::DataFlowGraph,
        value: cranelift_codegen::ir::Value,
        depth: u8,
    ) -> Option<i64> {
        use cranelift_codegen::ir::{condcodes::IntCC, InstructionData, Opcode, ValueDef};
        let value = dfg.resolve_aliases(value);
        let bits = dfg.value_type(value).bits();
        if !(1..=64).contains(&bits) || !dfg.value_type(value).is_int() {
            return None;
        }
        let normalize = |raw: i64| -> i64 {
            if bits >= 64 {
                raw
            } else {
                let shift = 64 - bits;
                (raw << shift) >> shift
            }
        };
        let ValueDef::Result(inst, 0) = dfg.value_def(value) else {
            return None;
        };
        match dfg.insts[inst] {
            InstructionData::UnaryImm {
                opcode: Opcode::Iconst,
                imm,
            } => Some(normalize(imm.bits())),
            InstructionData::IntCompareImm {
                opcode: Opcode::IcmpImm,
                cond,
                arg,
                imm,
            } if depth > 0 => {
                let arg_bits = dfg.value_type(arg).bits();
                let lhs = fold(dfg, arg, depth - 1)?;
                let rhs = if arg_bits >= 64 {
                    imm.bits()
                } else {
                    let shift = 64 - arg_bits;
                    (imm.bits() << shift) >> shift
                };
                match cond {
                    IntCC::Equal => Some(i64::from(lhs == rhs)),
                    IntCC::NotEqual => Some(i64::from(lhs != rhs)),
                    _ => None,
                }
            }
            InstructionData::Unary {
                opcode: Opcode::Uextend,
                arg,
            } if depth > 0 => {
                let arg_bits = dfg.value_type(arg).bits();
                let known = fold(dfg, arg, depth - 1)?;
                Some(if arg_bits >= 64 {
                    known
                } else {
                    known & ((1_i64 << arg_bits) - 1)
                })
            }
            InstructionData::Unary {
                opcode: Opcode::Sextend | Opcode::Ireduce,
                arg,
            } if depth > 0 => fold(dfg, arg, depth - 1).map(normalize),
            InstructionData::IntCompare {
                opcode: Opcode::Icmp,
                cond: cond @ (IntCC::Equal | IntCC::NotEqual),
                args: [lhs, rhs],
            } if depth > 0 => {
                let equal = fold(dfg, lhs, depth - 1)? == fold(dfg, rhs, depth - 1)?;
                Some(i64::from(if cond == IntCC::Equal { equal } else { !equal }))
            }
            InstructionData::Binary {
                opcode: opcode @ (Opcode::Band | Opcode::Bor),
                args: [lhs, rhs],
            } if depth > 0 => {
                let lhs = fold(dfg, lhs, depth - 1)?;
                let rhs = fold(dfg, rhs, depth - 1)?;
                Some(normalize(if opcode == Opcode::Band {
                    lhs & rhs
                } else {
                    lhs | rhs
                }))
            }
            _ => None,
        }
    }
    fold(&builder.func.dfg, value, 8)
}

/// True when `pair.tag` is a compile-time constant equal to `expected`.
fn opt_tag_known(
    builder: &cranelift_frontend::FunctionBuilder<'_>,
    tag: cranelift_codegen::ir::Value,
    expected: i32,
) -> bool {
    opt_known_const(builder, tag) == Some(i64::from(expected))
}

fn opt_tag_is(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    tag: cranelift_codegen::ir::Value,
    expected: i32,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::{condcodes::IntCC, InstBuilder};
    builder
        .ins()
        .icmp_imm(IntCC::Equal, tag, i64::from(expected))
}

/// The Int32 payload of a pair whose tag was (or is about to be) checked.
fn opt_i32(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    pair: OptPair,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::{types, InstBuilder};
    if env.int32_loop {
        pair.payload
    } else {
        builder.ins().ireduce(types::I32, pair.payload)
    }
}

fn opt_int_pair(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    value: cranelift_codegen::ir::Value,
) -> OptPair {
    use cranelift_codegen::ir::{types, InstBuilder};
    OptPair {
        payload: if env.int32_loop {
            value
        } else {
            builder.ins().sextend(types::I64, value)
        },
        tag: builder
            .ins()
            .iconst(types::I64, i64::from(rquickjs_core::qjs::JS_TAG_INT)),
    }
}

fn opt_bool_pair(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    truth: cranelift_codegen::ir::Value,
) -> OptPair {
    use cranelift_codegen::ir::{types, InstBuilder};
    OptPair {
        payload: builder.ins().uextend(env.payload_type, truth),
        tag: builder
            .ins()
            .iconst(types::I64, i64::from(rquickjs_core::qjs::JS_TAG_BOOL)),
    }
}

/// ECMAScript ToInt32 of a value already proven Int32 or Float64: exact
/// modulo 2^32 truncation, NaN and infinities map to zero.
fn opt_to_i32(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    pair: OptPair,
) -> cranelift_codegen::ir::Value {
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    let is_int = opt_tag_is(builder, pair.tag, rquickjs_core::qjs::JS_TAG_INT);
    let direct = builder.ins().ireduce(types::I32, pair.payload);
    let number = builder
        .ins()
        .bitcast(types::F64, MemFlags::new(), pair.payload);
    let modulus = builder.ins().f64const(4_294_967_296.0);
    let quotient = builder.ins().fdiv(number, modulus);
    let quotient = builder.ins().trunc(quotient);
    let multiple = builder.ins().fmul(quotient, modulus);
    let remainder = builder.ins().fsub(number, multiple);
    let converted = builder.ins().fcvt_to_sint_sat(types::I64, remainder);
    let converted = builder.ins().ireduce(types::I32, converted);
    builder.ins().select(is_int, direct, converted)
}

/// Stack slots that alias a frame slot stop describing it once the slot is
/// redefined; their SSA value is the primitive the slot held before.
fn opt_invalidate_provenance(provenance: &mut [OptProvenance], depth: usize, stale: OptProvenance) {
    for slot in provenance.iter_mut().take(depth) {
        if *slot == stale {
            *slot = OptProvenance::ImmediatePrimitive;
        }
    }
}

fn opt_u8(bytes: &[u8]) -> Result<usize, CompileFailure> {
    bytes
        .get(1)
        .copied()
        .map(usize::from)
        .ok_or(CompileFailure::InvalidArtifact)
}

/// QuickJS stack shuffles as (values consumed, sources of the values pushed
/// back), identical to the interpreter and the Tier 1 lowering.
fn opt_stack_permutation(name: &str) -> Option<(usize, &'static [usize])> {
    crate::ir::stack_permutation(name)
}

/// `lhs + rhs` with exact JavaScript numeric semantics for two operands whose
/// tags are checked here: Int32 + Int32 stays Int32 unless it overflows into
/// Float64, any other numeric mix is a Float64 add, and non-numeric operands
/// deoptimize before any effect. In the raw-i32 loop shape a Float64 result
/// cannot be represented, so overflow deoptimizes instead.
#[allow(clippy::too_many_arguments)]
fn emit_opt_checked_update(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    mode: crate::ir::ScalarNumericMode,
    provenance: &[OptProvenance],
    depth: usize,
    lhs: OptPair,
    rhs: OptPair,
    pc: u32,
    guard: u32,
) -> Result<OptPair, CompileFailure> {
    use cranelift_codegen::ir::InstBuilder;
    if env.int32_loop || mode != crate::ir::ScalarNumericMode::Int32 {
        return emit_opt_checked_add(builder, env, provenance, depth, lhs, rhs, pc, guard);
    }
    let lhs = opt_i32(builder, env, lhs);
    let rhs = opt_i32(builder, env, rhs);
    let (sum, overflow) = builder.ins().sadd_overflow(lhs, rhs);
    let valid = builder.ins().bxor_imm(overflow, 1);
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, valid)?;
    Ok(opt_int_pair(builder, env, sum))
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_checked_add(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &[OptProvenance],
    depth: usize,
    lhs: OptPair,
    rhs: OptPair,
    pc: u32,
    guard: u32,
) -> Result<OptPair, CompileFailure> {
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;
    if env.int32_loop {
        // Every value in the raw-i32 loop shape is a proven Int32 (entry and
        // header guards), so only the overflow exit remains.
        let (sum, overflow) = builder.ins().sadd_overflow(lhs.payload, rhs.payload);
        let pass = builder.create_block();
        let deopt = builder.create_block();
        builder.set_cold_block(deopt);
        builder.append_block_param(pass, types::I32);
        builder.ins().brif(overflow, deopt, &[], pass, &[sum]);
        builder.switch_to_block(deopt);
        emit_opt_deopt(builder, env, provenance, depth, pc, guard)?;
        builder.switch_to_block(pass);
        let sum = builder.block_params(pass)[0];
        return Ok(opt_int_pair(builder, env, sum));
    }
    let lhs_int = opt_tag_is(builder, lhs.tag, qjs::JS_TAG_INT);
    let rhs_int = opt_tag_is(builder, rhs.tag, qjs::JS_TAG_INT);
    let both_int = builder.ins().band(lhs_int, rhs_int);
    let lhs_float = opt_tag_is(builder, lhs.tag, qjs::JS_TAG_FLOAT64);
    let rhs_float = opt_tag_is(builder, rhs.tag, qjs::JS_TAG_FLOAT64);
    let lhs_numeric = builder.ins().bor(lhs_int, lhs_float);
    let rhs_numeric = builder.ins().bor(rhs_int, rhs_float);
    let numeric = builder.ins().band(lhs_numeric, rhs_numeric);
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, numeric)?;
    let li = builder.ins().ireduce(types::I32, lhs.payload);
    let ri = builder.ins().ireduce(types::I32, rhs.payload);
    let (sum, overflow) = builder.ins().sadd_overflow(li, ri);
    let no_overflow = builder.ins().bxor_imm(overflow, 1);
    let keep_int = builder.ins().band(both_int, no_overflow);
    let lf = opt_f64(builder, lhs);
    let rf = opt_f64(builder, rhs);
    let float_sum = builder.ins().fadd(lf, rf);
    let int_payload = builder.ins().sextend(types::I64, sum);
    let float_payload = builder
        .ins()
        .bitcast(types::I64, MemFlags::new(), float_sum);
    let int_tag = builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT));
    let float_tag = builder
        .ins()
        .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64));
    Ok(OptPair {
        payload: builder.ins().select(keep_int, int_payload, float_payload),
        tag: builder.ins().select(keep_int, int_tag, float_tag),
    })
}

/// Unary numeric opcodes (`neg`, `plus`, `not`, `lnot`) on the stack top.
/// Numeric (and, for `lnot`, boolean/nullish) inputs are computed natively
/// with exact JavaScript results; every other tag deoptimizes to the
/// interpreter before any effect.
#[allow(clippy::too_many_arguments)]
fn emit_opt_unary(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &mut [OptProvenance],
    depth: usize,
    operation: &str,
    pc: u32,
    guard: u32,
) -> Result<(), CompileFailure> {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;
    let index = depth
        .checked_sub(1)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let value = opt_use(builder, env.stack[index]);
    let is_int = opt_tag_is(builder, value.tag, qjs::JS_TAG_INT);
    let is_float = opt_tag_is(builder, value.tag, qjs::JS_TAG_FLOAT64);
    let numeric = if env.int32_loop {
        is_int
    } else {
        builder.ins().bor(is_int, is_float)
    };
    let result = match operation {
        "plus" => {
            emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, numeric)?;
            value
        }
        "neg" => {
            if env.int32_loop {
                // Zero and INT32_MIN negate to Float64 values that the raw
                // i32 loop shape cannot hold.
                let nonzero = builder.ins().icmp_imm(IntCC::NotEqual, value.payload, 0);
                let not_min =
                    builder
                        .ins()
                        .icmp_imm(IntCC::NotEqual, value.payload, i64::from(i32::MIN));
                let representable = builder.ins().band(nonzero, not_min);
                let ok = builder.ins().band(is_int, representable);
                emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, ok)?;
                let negated = builder.ins().ineg(value.payload);
                opt_int_pair(builder, env, negated)
            } else {
                emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, numeric)?;
                let iv = builder.ins().ireduce(types::I32, value.payload);
                let int_result = builder.ins().ineg(iv);
                let fv = opt_f64(builder, value);
                let float_result = builder.ins().fneg(fv);
                let nonzero = builder.ins().icmp_imm(IntCC::NotEqual, iv, 0);
                let not_min = builder
                    .ins()
                    .icmp_imm(IntCC::NotEqual, iv, i64::from(i32::MIN));
                let representable = builder.ins().band(nonzero, not_min);
                let keep_int = builder.ins().band(is_int, representable);
                let int_payload = builder.ins().sextend(types::I64, int_result);
                let float_payload =
                    builder
                        .ins()
                        .bitcast(types::I64, MemFlags::new(), float_result);
                let int_tag = builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT));
                let float_tag = builder
                    .ins()
                    .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64));
                OptPair {
                    payload: builder.ins().select(keep_int, int_payload, float_payload),
                    tag: builder.ins().select(keep_int, int_tag, float_tag),
                }
            }
        }
        "not" => {
            emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, numeric)?;
            let iv = if env.int32_loop {
                value.payload
            } else {
                opt_to_i32(builder, value)
            };
            let inverted = builder.ins().bnot(iv);
            opt_int_pair(builder, env, inverted)
        }
        "lnot" => {
            let is_bool = opt_tag_is(builder, value.tag, qjs::JS_TAG_BOOL);
            let is_null = opt_tag_is(builder, value.tag, qjs::JS_TAG_NULL);
            let is_undefined = opt_tag_is(builder, value.tag, qjs::JS_TAG_UNDEFINED);
            let scalar = builder.ins().bor(is_int, is_bool);
            let empty = builder.ins().bor(is_null, is_undefined);
            let numeric = if env.int32_loop {
                scalar
            } else {
                builder.ins().bor(scalar, is_float)
            };
            let allowed = builder.ins().bor(numeric, empty);
            emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, allowed)?;
            let iv = opt_i32(builder, env, value);
            let integer_truth = builder.ins().icmp_imm(IntCC::NotEqual, iv, 0);
            let numeric_truth = if env.int32_loop {
                integer_truth
            } else {
                let float_truth = super::helpers::emit_f64_bits_truthy(builder, value.payload);
                builder.ins().select(is_float, float_truth, integer_truth)
            };
            let false_value = builder.ins().iconst(types::I8, 0);
            let truth = builder.ins().select(empty, false_value, numeric_truth);
            let negated = builder.ins().bxor_imm(truth, 1);
            opt_bool_pair(builder, env, negated)
        }
        _ => return Err(CompileFailure::UnsupportedOpcode),
    };
    opt_define(builder, env.stack[index], result);
    provenance[index] = OptProvenance::ImmediatePrimitive;
    Ok(())
}

/// `%` on two Int32 operands whose result is provably an Int32: a non-negative
/// dividend never yields `-0` and rules out `INT32_MIN % -1`, and a non-zero
/// divisor cannot trap. Everything else (Float64 operands, negative
/// dividends, zero divisors, non-numeric tags) deoptimizes.
fn emit_opt_mod(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &mut [OptProvenance],
    depth: usize,
    pc: u32,
    guard: u32,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::{condcodes::IntCC, InstBuilder};
    use rquickjs_core::qjs;
    let output = depth
        .checked_sub(2)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let lhs = opt_use(builder, env.stack[output]);
    let rhs = opt_use(builder, env.stack[output + 1]);
    let lhs_int = opt_tag_is(builder, lhs.tag, qjs::JS_TAG_INT);
    let rhs_int = opt_tag_is(builder, rhs.tag, qjs::JS_TAG_INT);
    let both_int = builder.ins().band(lhs_int, rhs_int);
    let li = opt_i32(builder, env, lhs);
    let ri = opt_i32(builder, env, rhs);
    let non_negative = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, li, 0);
    let non_zero = builder.ins().icmp_imm(IntCC::NotEqual, ri, 0);
    let exact = builder.ins().band(non_negative, non_zero);
    let ok = builder.ins().band(both_int, exact);
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, ok)?;
    let remainder = builder.ins().srem(li, ri);
    let pair = opt_int_pair(builder, env, remainder);
    opt_define(builder, env.stack[output], pair);
    provenance[output] = OptProvenance::ImmediatePrimitive;
    provenance[output + 1] = OptProvenance::Unknown;
    Ok(output + 1)
}

/// `==`, `!=`, `===` and `!==` with exact results for operands that need no
/// coercion, i.e. both tagged Int32, Float64, Bool, `undefined` or `null`:
/// two numbers compare as Float64 (NaN is unequal to itself, `-0` equals
/// `0`), two booleans compare payloads, `undefined`/`null` pairs are equal
/// under `==` and equal under `===` only with matching tags, and any other
/// combination of these tags is unequal. Loose equality of a boolean with a
/// number coerces and deoptimizes, as does every string, object, symbol or
/// BigInt operand, so those run in the interpreter.
#[allow(clippy::too_many_arguments)]
fn emit_opt_equality(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &mut [OptProvenance],
    depth: usize,
    operation: &str,
    pc: u32,
    guard: u32,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
    use cranelift_codegen::ir::{types, InstBuilder};
    use rquickjs_core::qjs;
    let output = depth
        .checked_sub(2)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let lhs = opt_use(builder, env.stack[output]);
    let rhs = opt_use(builder, env.stack[output + 1]);
    let strict = matches!(operation, "strict_eq" | "strict_neq");
    let lhs_int = opt_tag_is(builder, lhs.tag, qjs::JS_TAG_INT);
    let rhs_int = opt_tag_is(builder, rhs.tag, qjs::JS_TAG_INT);
    let (lhs_numeric, rhs_numeric) = if env.int32_loop {
        (lhs_int, rhs_int)
    } else {
        let lhs_float = opt_tag_is(builder, lhs.tag, qjs::JS_TAG_FLOAT64);
        let rhs_float = opt_tag_is(builder, rhs.tag, qjs::JS_TAG_FLOAT64);
        (
            builder.ins().bor(lhs_int, lhs_float),
            builder.ins().bor(rhs_int, rhs_float),
        )
    };
    let both_numeric = builder.ins().band(lhs_numeric, rhs_numeric);
    let lhs_bool = opt_tag_is(builder, lhs.tag, qjs::JS_TAG_BOOL);
    let rhs_bool = opt_tag_is(builder, rhs.tag, qjs::JS_TAG_BOOL);
    let lhs_undefined = opt_tag_is(builder, lhs.tag, qjs::JS_TAG_UNDEFINED);
    let rhs_undefined = opt_tag_is(builder, rhs.tag, qjs::JS_TAG_UNDEFINED);
    let lhs_null = opt_tag_is(builder, lhs.tag, qjs::JS_TAG_NULL);
    let rhs_null = opt_tag_is(builder, rhs.tag, qjs::JS_TAG_NULL);
    let lhs_nullish = builder.ins().bor(lhs_undefined, lhs_null);
    let rhs_nullish = builder.ins().bor(rhs_undefined, rhs_null);
    let lhs_primitive = builder.ins().bor(lhs_numeric, lhs_bool);
    let lhs_primitive = builder.ins().bor(lhs_primitive, lhs_nullish);
    let rhs_primitive = builder.ins().bor(rhs_numeric, rhs_bool);
    let rhs_primitive = builder.ins().bor(rhs_primitive, rhs_nullish);
    let mut allowed = builder.ins().band(lhs_primitive, rhs_primitive);
    if !strict {
        let lhs_coerces = builder.ins().band(lhs_bool, rhs_numeric);
        let rhs_coerces = builder.ins().band(rhs_bool, lhs_numeric);
        let coerces = builder.ins().bor(lhs_coerces, rhs_coerces);
        let pure = builder.ins().bxor_imm(coerces, 1);
        allowed = builder.ins().band(allowed, pure);
    }
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, allowed)?;
    let numeric_equal = if env.int32_loop {
        builder.ins().icmp(IntCC::Equal, lhs.payload, rhs.payload)
    } else {
        let lf = opt_f64(builder, lhs);
        let rf = opt_f64(builder, rhs);
        builder.ins().fcmp(FloatCC::Equal, lf, rf)
    };
    let li = opt_i32(builder, env, lhs);
    let ri = opt_i32(builder, env, rhs);
    let payload_equal = builder.ins().icmp(IntCC::Equal, li, ri);
    let both_bool = builder.ins().band(lhs_bool, rhs_bool);
    let both_nullish = builder.ins().band(lhs_nullish, rhs_nullish);
    let nullish_equal = if strict {
        builder.ins().icmp(IntCC::Equal, lhs.tag, rhs.tag)
    } else {
        builder.ins().iconst(types::I8, 1)
    };
    let falsehood = builder.ins().iconst(types::I8, 0);
    let simple_equal = builder.ins().select(both_nullish, nullish_equal, falsehood);
    let simple_equal = builder.ins().select(both_bool, payload_equal, simple_equal);
    let equal = builder
        .ins()
        .select(both_numeric, numeric_equal, simple_equal);
    let result = if matches!(operation, "neq" | "strict_neq") {
        builder.ins().bxor_imm(equal, 1)
    } else {
        equal
    };
    let pair = opt_bool_pair(builder, env, result);
    opt_define(builder, env.stack[output], pair);
    provenance[output] = OptProvenance::ImmediatePrimitive;
    provenance[output + 1] = OptProvenance::Unknown;
    Ok(output + 1)
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_guarded_propkey(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    arg_buf: cranelift_codegen::ir::Value,
    var_buf: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    arguments: &[OptVars],
    locals: &[OptVars],
    stack: &[OptVars],
    stack_provenance: &mut [OptProvenance],
    depth: usize,
    pc: u32,
    guard: u32,
    helper_signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{InstBuilder, MemFlags};
    use rquickjs_core::qjs;
    let index = depth
        .checked_sub(1)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let value = opt_use(builder, stack[index]);
    if opt_tag_known(builder, value.tag, qjs::JS_TAG_INT) {
        // A literal Int32 key is already canonical: ToPropertyKey is the
        // identity and the exit is unreachable.
        return Ok(depth);
    }
    let int = builder
        .ins()
        .icmp_imm(IntCC::Equal, value.tag, i64::from(qjs::JS_TAG_INT));
    let string = builder
        .ins()
        .icmp_imm(IntCC::Equal, value.tag, i64::from(qjs::JS_TAG_STRING));
    let symbol = builder
        .ins()
        .icmp_imm(IntCC::Equal, value.tag, i64::from(qjs::JS_TAG_SYMBOL));
    let valid = builder.ins().bor(int, string);
    let valid = builder.ins().bor(valid, symbol);
    let continuation = builder.create_block();
    let deopt = builder.create_block();
    builder.ins().brif(valid, continuation, &[], deopt, &[]);
    builder.seal_block(continuation);
    builder.seal_block(deopt);
    builder.switch_to_block(deopt);
    for (slot, vars) in arguments.iter().enumerate() {
        let current = opt_use(builder, *vars);
        opt_store(builder, arg_buf, slot, current);
    }
    for (slot, vars) in locals.iter().enumerate() {
        let current = opt_use(builder, *vars);
        opt_store(builder, var_buf, slot, current);
    }
    for (slot, vars) in stack.iter().take(depth).enumerate() {
        let current = opt_use(builder, *vars);
        opt_store(builder, stack_base, slot, current);
    }
    opt_set_stack_top(builder, frame, stack_base, depth, pointer_type, layout);
    let start = builder
        .ins()
        .load(pointer_type, MemFlags::new(), frame, layout.bytecode_start);
    let resume = builder.ins().iadd_imm(start, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), resume, frame, layout.pc);
    opt_own_stack_for_exit(
        builder,
        frame,
        sret,
        stack_base,
        depth,
        arguments.len() + locals.len(),
        stack_provenance,
        helper_signatures,
        pointer_type,
        layout,
    )?;
    emit_opt_exit(
        builder,
        sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
        Some(resume),
        pointer_type,
        guard,
    );
    builder.switch_to_block(continuation);
    Ok(depth)
}

/// Bitwise and shift opcodes on two Int32 operands. Shift counts are masked
/// to five bits exactly like ToUint32(count) & 31 in the specification, and
/// `>>>` renormalizes results at or above 2^31 to Float64 (or deoptimizes in
/// the raw-i32 loop shape, which cannot hold a Float64). Non-Int32 operands
/// deoptimize so the interpreter performs ToInt32/ToNumeric with effects.
#[allow(clippy::too_many_arguments)]
fn emit_opt_guarded_int_binary(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &mut [OptProvenance],
    depth: usize,
    operation: crate::ir::ScalarBitwiseOp,
    lhs: OptPair,
    rhs: OptPair,
    pc: u32,
    guard: u32,
) -> Result<usize, CompileFailure> {
    use crate::ir::ScalarBitwiseOp;
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;

    let output = depth
        .checked_sub(2)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let li = opt_i32(builder, env, lhs);
    let ri = opt_i32(builder, env, rhs);
    let value = match operation {
        ScalarBitwiseOp::Or => builder.ins().bor(li, ri),
        ScalarBitwiseOp::And => builder.ins().band(li, ri),
        ScalarBitwiseOp::Xor => builder.ins().bxor(li, ri),
        ScalarBitwiseOp::Shl | ScalarBitwiseOp::Sar | ScalarBitwiseOp::Shr => {
            let count = builder.ins().band_imm(ri, 31);
            match operation {
                ScalarBitwiseOp::Shl => builder.ins().ishl(li, count),
                ScalarBitwiseOp::Sar => builder.ins().sshr(li, count),
                _ => builder.ins().ushr(li, count),
            }
        }
    };
    // `x >>> c` with a constant count whose low five bits are non-zero is
    // below 2^31, so it is always an Int32 and needs neither a guard nor a
    // Float64 alternative.
    let shr_always_int32 = operation == ScalarBitwiseOp::Shr
        && opt_constant(builder, ri).is_some_and(|count| count & 31 != 0);
    let result = if operation == ScalarBitwiseOp::Shr && !shr_always_int32 {
        let fits_int32 = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, value, 0);
        if env.int32_loop {
            emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, fits_int32)?;
            opt_int_pair(builder, env, value)
        } else {
            let int_payload = builder.ins().sextend(types::I64, value);
            let uint_float = builder.ins().fcvt_from_uint(types::F64, value);
            let float_payload = builder
                .ins()
                .bitcast(types::I64, MemFlags::new(), uint_float);
            let int_tag = builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT));
            let float_tag = builder
                .ins()
                .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64));
            OptPair {
                payload: builder.ins().select(fits_int32, int_payload, float_payload),
                tag: builder.ins().select(fits_int32, int_tag, float_tag),
            }
        }
    } else {
        opt_int_pair(builder, env, value)
    };
    opt_define(builder, env.stack[output], result);
    provenance[output] = OptProvenance::ImmediatePrimitive;
    provenance[output + 1] = OptProvenance::Unknown;
    Ok(output + 1)
}

/// The integer constant `value` is defined as, looking through the integer
/// width conversions the lowering inserts between payloads and operands.
fn opt_constant(
    builder: &cranelift_frontend::FunctionBuilder<'_>,
    value: cranelift_codegen::ir::Value,
) -> Option<i64> {
    use cranelift_codegen::ir::{InstructionData, Opcode, ValueDef};
    let mut value = builder.func.dfg.resolve_aliases(value);
    loop {
        let ValueDef::Result(inst, 0) = builder.func.dfg.value_def(value) else {
            return None;
        };
        match builder.func.dfg.insts[inst] {
            InstructionData::UnaryImm {
                opcode: Opcode::Iconst,
                imm,
            } => return Some(imm.bits()),
            InstructionData::Unary {
                opcode: Opcode::Ireduce | Opcode::Sextend | Opcode::Uextend,
                arg,
            } => value = builder.func.dfg.resolve_aliases(arg),
            _ => return None,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_packed_metadata_guard(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &[OptProvenance],
    depth: usize,
    pc: u32,
    guard: u32,
    object: OptPair,
    source_provenance: OptProvenance,
    block_pc: u32,
    element_layout: crate::abi::ElementLayout,
) -> Result<GuardedElementSource, CompileFailure> {
    use cranelift_codegen::ir::{condcodes::IntCC, types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;

    let object_ok = builder
        .ins()
        .icmp_imm(IntCC::Equal, object.tag, i64::from(qjs::JS_TAG_OBJECT));
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, object_ok)?;
    let flags = builder.ins().load(
        types::I8,
        MemFlags::new(),
        object.payload,
        element_layout.object_flags_offset,
    );
    let fast = builder
        .ins()
        .band_imm(flags, element_layout.object_fast_array_mask);
    let fast = builder.ins().icmp_imm(IntCC::NotEqual, fast, 0);
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, fast)?;
    let class = builder.ins().load(
        types::I16,
        MemFlags::new(),
        object.payload,
        element_layout.object_class_id_offset,
    );
    let class = builder.ins().uextend(types::I64, class);
    let packed = builder
        .ins()
        .icmp_imm(IntCC::Equal, class, element_layout.array_class_id);
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, packed)?;

    let count = builder.ins().load(
        types::I32,
        MemFlags::new(),
        object.payload,
        element_layout.array_count_offset,
    );
    let data = builder.ins().load(
        env.pointer_type,
        MemFlags::new(),
        object.payload,
        element_layout.array_data_offset,
    );
    let property_layout = crate::abi::AbiInfo::linked()
        .map_err(|_| CompileFailure::InvalidArtifact)?
        .property_layout();
    let properties = builder.ins().load(
        env.pointer_type,
        MemFlags::new(),
        object.payload,
        property_layout.object_properties_offset,
    );
    let length = opt_load(builder, properties, 0);
    let length_is_int =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, length.tag, i64::from(qjs::JS_TAG_INT));
    let length_payload = builder.ins().ireduce(types::I32, length.payload);
    let exact_dense = builder.ins().icmp(IntCC::Equal, length_payload, count);
    let exact_dense = builder.ins().band(length_is_int, exact_dense);
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, exact_dense)?;
    let empty = builder.ins().icmp_imm(IntCC::Equal, count, 0);
    let has_data = builder.ins().icmp_imm(IntCC::NotEqual, data, 0);
    let usable_data = builder.ins().bor(empty, has_data);
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, usable_data)?;
    Ok(GuardedElementSource {
        provenance: source_provenance,
        block_pc,
        data,
        count,
        kind: builder.ins().iconst(types::I8, 0),
        exact_length: true,
        typed_mode: None,
        static_kind: Some(0),
        unguarded: false,
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_typed_metadata_guard(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &[OptProvenance],
    depth: usize,
    pc: u32,
    guard: u32,
    object: OptPair,
    source_provenance: OptProvenance,
    block_pc: u32,
    mode: crate::runtime::ArrayMode,
    array_query: usize,
    needs_length: bool,
    side_exit: bool,
) -> Result<GuardedElementSource, CompileFailure> {
    use cranelift_codegen::ir::{
        condcodes::IntCC, types, AbiParam, InstBuilder, MemFlags, Signature, StackSlotData,
        StackSlotKind,
    };
    use rquickjs_core::qjs;

    let (expected_mode, kind) = match mode {
        crate::runtime::ArrayMode::Int32 => (qjs::JSJitArrayMode_JS_JIT_ARRAY_MODE_INT32, 1_i64),
        crate::runtime::ArrayMode::Float64 => {
            (qjs::JSJitArrayMode_JS_JIT_ARRAY_MODE_FLOAT64, 2_i64)
        }
        _ => return Err(CompileFailure::InvalidArtifact),
    };
    let receiver = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        u32::try_from(core::mem::size_of::<qjs::JSValue>())
            .map_err(|_| CompileFailure::ResourceLimit)?,
        3,
    ));
    let receiver = builder.ins().stack_addr(env.pointer_type, receiver, 0);
    builder
        .ins()
        .store(MemFlags::new(), object.payload, receiver, 0);
    builder
        .ins()
        .store(MemFlags::new(), object.tag, receiver, env.layout.value_tag);

    let metadata = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        u32::try_from(core::mem::size_of::<qjs::JSJitArrayMetadata>())
            .map_err(|_| CompileFailure::ResourceLimit)?,
        3,
    ));
    let metadata = builder.ins().stack_addr(env.pointer_type, metadata, 0);
    let metadata_size = builder.ins().iconst(
        types::I32,
        i64::try_from(core::mem::size_of::<qjs::JSJitArrayMetadata>())
            .map_err(|_| CompileFailure::ResourceLimit)?,
    );
    builder
        .ins()
        .store(MemFlags::new(), metadata_size, metadata, 0);

    let mut signature = Signature::new(builder.func.signature.call_conv);
    signature.params.extend([
        AbiParam::new(env.pointer_type),
        AbiParam::new(env.pointer_type),
        AbiParam::new(types::I32),
        AbiParam::new(types::I32),
        AbiParam::new(env.pointer_type),
    ]);
    signature.returns.push(AbiParam::new(types::I32));
    let signature = builder.import_signature(signature);
    let query = builder.ins().iconst(
        env.pointer_type,
        i64::try_from(array_query).map_err(|_| CompileFailure::ResourceLimit)?,
    );
    let ctx = builder
        .ins()
        .load(env.pointer_type, MemFlags::new(), env.frame, env.layout.ctx);
    let expected_mode_value = builder.ins().iconst(types::I32, i64::from(expected_mode));
    let flags = builder.ins().iconst(
        types::I32,
        if needs_length {
            i64::from(qjs::JS_JIT_ARRAY_QUERY_LENGTH)
        } else {
            0
        },
    );
    super::emit_msan_unpoison(
        builder,
        env.pointer_type,
        receiver,
        core::mem::size_of::<qjs::JSValue>(),
    );
    super::emit_msan_unpoison(
        builder,
        env.pointer_type,
        metadata,
        core::mem::size_of::<qjs::JSJitArrayMetadata>(),
    );
    let call = super::emit_external_call(
        builder,
        signature,
        query,
        &[ctx, receiver, expected_mode_value, flags, metadata],
        env.pointer_type,
        None,
        None,
    );
    let status = builder.inst_results(call)[0];
    let accepted = builder.ins().icmp_imm(
        IntCC::Equal,
        status,
        i64::from(qjs::JSJitArrayQueryStatus_JS_JIT_ARRAY_QUERY_OK),
    );
    if side_exit {
        emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, accepted)?;
    }
    let live_mode = builder.ins().load(types::I32, MemFlags::new(), metadata, 4);
    let mode_matches = builder
        .ins()
        .icmp(IntCC::Equal, live_mode, expected_mode_value);
    if side_exit {
        emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, mode_matches)?;
    }
    let mut count = builder.ins().load(types::I32, MemFlags::new(), metadata, 8);
    let mut data = builder
        .ins()
        .load(env.pointer_type, MemFlags::new(), metadata, 16);
    if !side_exit {
        // A missed query publishes an empty view instead of exiting: every
        // consumer's bounds check then exits at the access that actually
        // runs. The slot is a live stack slot, so the loads discarded on the
        // miss path read valid (if stale) memory.
        let usable = builder.ins().band(accepted, mode_matches);
        let miss = builder.create_block();
        let join = builder.create_block();
        builder.append_block_param(join, types::I32);
        builder.append_block_param(join, env.pointer_type);
        builder.set_cold_block(miss);
        builder.ins().brif(usable, join, &[count, data], miss, &[]);
        builder.seal_block(miss);
        builder.switch_to_block(miss);
        let empty_count = builder.ins().iconst(types::I32, 0);
        let null_data = builder.ins().iconst(env.pointer_type, 0);
        builder.ins().jump(join, &[empty_count, null_data]);
        builder.seal_block(join);
        builder.switch_to_block(join);
        count = builder.block_params(join)[0];
        data = builder.block_params(join)[1];
    }
    Ok(GuardedElementSource {
        provenance: source_provenance,
        block_pc,
        data,
        count,
        kind: builder.ins().iconst(types::I8, kind),
        exact_length: needs_length && side_exit,
        typed_mode: Some(mode),
        static_kind: Some(kind),
        unguarded: !side_exit,
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_packed_loop_revalidate(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &[OptProvenance],
    depth: usize,
    pc: u32,
    guard: u32,
    expected: GuardedElementSource,
    element_layout: crate::abi::ElementLayout,
    array_query: usize,
) -> Result<(), CompileFailure> {
    use cranelift_codegen::ir::{condcodes::IntCC, InstBuilder};
    // An unguarded source whose query missed (or found an empty view) holds
    // `count == 0`: every consumer's bounds check exits before touching
    // `data`, whatever the receiver has become, so there is nothing to
    // revalidate. Requiring the current state here would reintroduce the
    // speculative exit the unguarded hoist exists to avoid.
    let skip = expected.unguarded.then(|| {
        let empty = builder.ins().icmp_imm(IntCC::Equal, expected.count, 0);
        let check = builder.create_block();
        let done = builder.create_block();
        builder.ins().brif(empty, done, &[], check, &[]);
        builder.seal_block(check);
        builder.switch_to_block(check);
        done
    });
    let object = opt_receiver_object(builder, env, expected.provenance)?;
    let current = if let Some(mode) = expected.typed_mode {
        emit_opt_typed_metadata_guard(
            builder,
            env,
            provenance,
            depth,
            pc,
            guard,
            object,
            expected.provenance,
            expected.block_pc,
            mode,
            array_query,
            expected.exact_length,
            true,
        )?
    } else {
        emit_opt_packed_metadata_guard(
            builder,
            env,
            provenance,
            depth,
            pc,
            guard,
            object,
            expected.provenance,
            expected.block_pc,
            element_layout,
        )?
    };
    let same_count = builder
        .ins()
        .icmp(IntCC::Equal, current.count, expected.count);
    let same_data = builder
        .ins()
        .icmp(IntCC::Equal, current.data, expected.data);
    let unchanged = builder.ins().band(same_count, same_data);
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, unchanged)?;
    if let Some(done) = skip {
        builder.ins().jump(done, &[]);
        builder.seal_block(done);
        builder.switch_to_block(done);
    }
    Ok(())
}

fn opt_array_receiver_provenance(receiver: array_cache::ArrayReceiver) -> OptProvenance {
    match receiver {
        array_cache::ArrayReceiver::Argument(argument) => {
            OptProvenance::Argument(usize::from(argument))
        }
        array_cache::ArrayReceiver::Local(local) => OptProvenance::Local(usize::from(local)),
    }
}

/// Constant-pool index of a `push_const`/`push_const8` node.
fn opt_push_const_index(node: &crate::ir::OptimizedNode) -> Option<u32> {
    let crate::ir::OptimizedNodeKind::Bytecode { opcode } = node.kind() else {
        return None;
    };
    match opcode.as_ref() {
        "push_const8" => node.bytes().get(1).copied().map(u32::from),
        "push_const" => Some(u32::from_le_bytes(node.bytes().get(1..5)?.try_into().ok()?)),
        _ => None,
    }
}

/// The current borrowed value of the frame slot that keys guarded metadata.
fn opt_receiver_object(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: OptProvenance,
) -> Result<OptPair, CompileFailure> {
    let vars = match provenance {
        OptProvenance::Argument(argument) => env.arguments.get(argument),
        OptProvenance::Local(local) => env.locals.get(local),
        _ => None,
    }
    .ok_or(CompileFailure::InvalidArtifact)?;
    Ok(opt_use(builder, *vars))
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_array_loop_hoists(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &[OptProvenance],
    depth: usize,
    plan: &array_cache::ArrayPlan,
    preheader: u32,
    successor: u32,
    sources: &mut std::collections::BTreeMap<u32, Vec<GuardedElementSource>>,
    element_layout: crate::abi::ElementLayout,
    array_query: usize,
    ir: &OptimizedIr,
) -> Result<(), CompileFailure> {
    for hoist in plan
        .hoists()
        .iter()
        .filter(|hoist| hoist.preheader == preheader && hoist.header == successor)
    {
        let candidate = plan
            .candidates()
            .get(hoist.candidate)
            .ok_or(CompileFailure::InvalidArtifact)?;
        let receiver = opt_array_receiver_provenance(candidate.receiver);
        let object = opt_receiver_object(builder, env, receiver)?;
        let guard = ir
            .blocks()
            .iter()
            .find(|block| block.start_pc() == hoist.header)
            .and_then(|block| {
                block
                    .nodes()
                    .iter()
                    .find_map(|&id| match ir.nodes()[id as usize].kind() {
                        crate::ir::OptimizedNodeKind::GuardNumeric {
                            guard,
                            mid_loop: true,
                        } => Some(*guard),
                        _ => None,
                    })
            })
            .ok_or(CompileFailure::InvalidArtifact)?;
        let source = match candidate.mode {
            crate::runtime::ArrayMode::Packed => emit_opt_packed_metadata_guard(
                builder,
                env,
                provenance,
                depth,
                hoist.header,
                guard,
                object,
                receiver,
                hoist.header,
                element_layout,
            )?,
            crate::runtime::ArrayMode::Int32 | crate::runtime::ArrayMode::Float64 => {
                emit_opt_typed_metadata_guard(
                    builder,
                    env,
                    provenance,
                    depth,
                    hoist.header,
                    guard,
                    object,
                    receiver,
                    hoist.header,
                    candidate.mode,
                    array_query,
                    // This hoist owns the loop's single length read, so it
                    // certifies the intrinsic lookup and side-exits: range
                    // analysis deletes bounds checks against its count.
                    true,
                    true,
                )?
            }
            crate::runtime::ArrayMode::Generic => return Err(CompileFailure::InvalidArtifact),
        };
        opt_push_hoisted_source(sources, hoist.header, source)?;
    }
    for hoist in plan
        .storage_hoists()
        .iter()
        .filter(|hoist| hoist.preheader == preheader && hoist.header == successor)
    {
        let candidate = plan
            .candidates()
            .get(hoist.candidate)
            .ok_or(CompileFailure::InvalidArtifact)?;
        let receiver = opt_array_receiver_provenance(candidate.receiver);
        let object = opt_receiver_object(builder, env, receiver)?;
        let guard = opt_loop_header_guard(ir, hoist.header)?;
        let source = emit_opt_typed_metadata_guard(
            builder,
            env,
            provenance,
            depth,
            hoist.header,
            guard,
            object,
            receiver,
            hoist.header,
            candidate.mode,
            array_query,
            false,
            // A storage-only hoist covers no bounds check, so it need not
            // (and, being speculative for zero-trip loops, must not) side-exit
            // here: a miss publishes an empty view and the access that
            // actually runs exits at its own retained bounds check.
            false,
        )?;
        opt_push_hoisted_source(sources, hoist.header, source)?;
    }
    Ok(())
}

/// Every hoisted tuple that poll `node` must revalidate, from all loops that
/// contain it. Fails closed when an enclosing header's tuples are not seeded
/// exactly as planned (for example when lowering order would let a poll run
/// before the enclosing preheader emitted its guards).
fn opt_poll_revalidations(
    plan: &array_cache::ArrayPlan,
    node: u32,
    sources: &std::collections::BTreeMap<u32, Vec<GuardedElementSource>>,
) -> Result<Vec<GuardedElementSource>, CompileFailure> {
    let mut revalidations = Vec::new();
    for &header in plan.revalidating_headers(node) {
        let seeded = sources.get(&header).map_or(&[][..], Vec::as_slice);
        if seeded.len() != plan.hoisted_sources_at(header) {
            return Err(CompileFailure::InvalidArtifact);
        }
        revalidations.extend_from_slice(seeded);
    }
    Ok(revalidations)
}

fn opt_push_hoisted_source(
    sources: &mut std::collections::BTreeMap<u32, Vec<GuardedElementSource>>,
    header: u32,
    source: GuardedElementSource,
) -> Result<(), CompileFailure> {
    let seeded = sources.entry(header).or_default();
    if seeded.len() >= array_cache::ArrayPlan::MAX_LIVE_SOURCES
        || seeded
            .iter()
            .any(|existing| existing.provenance == source.provenance)
    {
        return Err(CompileFailure::InvalidArtifact);
    }
    seeded.push(source);
    Ok(())
}

/// The exact loop-header poll guard that owns every preheader metadata exit.
fn opt_loop_header_guard(ir: &OptimizedIr, header: u32) -> Result<u32, CompileFailure> {
    ir.blocks()
        .iter()
        .find(|block| block.start_pc() == header)
        .and_then(|block| {
            block
                .nodes()
                .iter()
                .find_map(|&id| match ir.nodes()[id as usize].kind() {
                    crate::ir::OptimizedNodeKind::GuardNumeric {
                        guard,
                        mid_loop: true,
                    } => Some(*guard),
                    _ => None,
                })
        })
        .ok_or(CompileFailure::InvalidArtifact)
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_array_length(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    stack_provenance: &mut [OptProvenance],
    depth: usize,
    pc: u32,
    element_layout: crate::abi::ElementLayout,
    guarded_source: &mut ElementSources,
    typed_length: Option<(crate::runtime::ArrayMode, u32, u32, usize)>,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;

    if env.int32_loop {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    let index = depth
        .checked_sub(1)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let source_provenance = stack_provenance[index];
    if let Some(source) = guarded_source
        .find(source_provenance)
        .filter(|source| source.exact_length)
    {
        // This is the V8/JSC-style checked-field path: the preheader guard
        // established exact logical-length == dense-count and all intervening
        // effects were admitted as non-reentrant.  Preserve the metadata for
        // the following element load; the amortized poll cold edge performs
        // the matching current-state revalidation.
        let result = OptPair {
            payload: builder.ins().sextend(types::I64, source.count),
            tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
        };
        opt_define(builder, env.stack[index], result);
        stack_provenance[index] = OptProvenance::ImmediatePrimitive;
        return Ok(depth);
    }
    if let Some((mode, guard, block_pc, array_query)) = typed_length {
        // The versioned leaf query validates the exact intrinsic getter chain
        // and fixed attached backing without running script; a miss resumes
        // QuickJS before this `get_length`, whose operand is still borrowed.
        let object = opt_use(builder, env.stack[index]);
        let source = emit_opt_typed_metadata_guard(
            builder,
            env,
            stack_provenance,
            depth,
            pc,
            guard,
            object,
            source_provenance,
            block_pc,
            mode,
            array_query,
            true,
            true,
        )?;
        let result = OptPair {
            payload: builder.ins().sextend(types::I64, source.count),
            tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
        };
        opt_define(builder, env.stack[index], result);
        stack_provenance[index] = OptProvenance::ImmediatePrimitive;
        guarded_source.insert(source);
        return Ok(depth);
    }
    // A generic lookup borrows its receiver from this existing owning root.
    // Its output replaces the consumed borrowed stack alias, without creating
    // an extra receiver owner that would need releasing after an accessor.
    let receiver_slot = match stack_provenance.get(index).copied() {
        Some(OptProvenance::Argument(argument)) => {
            u32::try_from(argument).map_err(|_| CompileFailure::ResourceLimit)?
        }
        Some(OptProvenance::Local(local)) => opt_flat_local_slot(env, local)?,
        _ => return Err(CompileFailure::UnsupportedOpcode),
    };
    let object = opt_use(builder, env.stack[index]);
    // Both arms leave the same ownership state. Publish surviving operands
    // before the generic arm can call an accessor or raise an exception.
    for (slot, vars) in env.arguments.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.arg_buf, slot, value);
    }
    for (slot, vars) in env.locals.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.var_buf, slot, value);
    }
    for (slot, vars) in env.stack.iter().take(index).enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.stack_base, slot, value);
    }
    let bytecode = builder.ins().load(
        env.pointer_type,
        MemFlags::new(),
        env.frame,
        env.layout.bytecode_start,
    );
    let current_pc = builder.ins().iadd_imm(bytecode, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), current_pc, env.frame, env.layout.pc);
    opt_own_stack_for_helper(
        builder,
        env.frame,
        env.sret,
        env.stack_base,
        index,
        env.arguments.len() + env.locals.len(),
        stack_provenance,
        env.helper_signatures,
        env.pointer_type,
        env.layout,
    )?;
    for source in stack_provenance.iter_mut().take(index) {
        if matches!(source, OptProvenance::Argument(_) | OptProvenance::Local(_)) {
            *source = OptProvenance::OwnedSlot;
        }
    }

    let classify = builder.create_block();
    let packed = builder.create_block();
    let generic = builder.create_block();
    let continuation = builder.create_block();
    builder.append_block_param(continuation, types::I64);
    builder.append_block_param(continuation, types::I64);
    let object_ok = builder
        .ins()
        .icmp_imm(IntCC::Equal, object.tag, i64::from(qjs::JS_TAG_OBJECT));
    builder.ins().brif(object_ok, classify, &[], generic, &[]);
    builder.switch_to_block(classify);
    let class = builder.ins().load(
        types::I16,
        MemFlags::new(),
        object.payload,
        element_layout.object_class_id_offset,
    );
    let class = builder.ins().uextend(types::I64, class);
    let is_array = builder
        .ins()
        .icmp_imm(IntCC::Equal, class, element_layout.array_class_id);
    builder.ins().brif(is_array, packed, &[], generic, &[]);

    builder.switch_to_block(packed);
    let property_layout = crate::abi::AbiInfo::linked()
        .map_err(|_| CompileFailure::InvalidArtifact)?
        .property_layout();
    let properties = builder.ins().load(
        env.pointer_type,
        MemFlags::new(),
        object.payload,
        property_layout.object_properties_offset,
    );
    // QuickJS Array's nonconfigurable length is always property slot zero,
    // including sparse arrays. It is a tagged uint32, not u.array.count:
    // extending length may leave dense storage unchanged, and large lengths
    // use Float64. Copy both words to preserve the complete unsigned range.
    let length = opt_load(builder, properties, 0);
    builder
        .ins()
        .jump(continuation, &[length.payload, length.tag]);

    builder.switch_to_block(generic);
    // TypedArray .length is an ordinary property lookup whose intrinsic
    // getter can be shadowed or replaced. Internal element count alone is
    // never a proof of that lookup. Until prototype/descriptor guards exist,
    // use the audited lookup bridge and keep arbitrary results owned.
    emit_opt_owned_helper_push(
        builder,
        env,
        stack_provenance,
        index,
        pc,
        qjs::JSJitHelperId_JS_JIT_HELPER_GET_PROPERTY as usize,
        &[receiver_slot, qjs::JS_ATOM_length],
    )?;
    let result = opt_use(builder, env.stack[index]);
    builder
        .ins()
        .jump(continuation, &[result.payload, result.tag]);

    builder.switch_to_block(continuation);
    let result = OptPair {
        payload: builder.block_params(continuation)[0],
        tag: builder.block_params(continuation)[1],
    };
    opt_define(builder, env.stack[index], result);
    opt_store(builder, env.stack_base, index, result);
    stack_provenance[index] = OptProvenance::OwnedSlot;
    opt_set_stack_top(
        builder,
        env.frame,
        env.stack_base,
        depth,
        env.pointer_type,
        env.layout,
    );
    // The logical result is not a dense-storage bounds proof, and the generic
    // arm may have changed storage through an accessor.
    guarded_source.clear();
    Ok(depth)
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_element_get(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    arg_buf: cranelift_codegen::ir::Value,
    var_buf: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    arguments: &[OptVars],
    locals: &[OptVars],
    stack: &[OptVars],
    stack_provenance: &mut [OptProvenance],
    depth: usize,
    pc: u32,
    guard: u32,
    helper_signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
    element_layout: crate::abi::ElementLayout,
    block_pc: u32,
    guarded_source: &mut ElementSources,
    expected_mode: Option<crate::runtime::ArrayMode>,
    bounds_covered_by_hoist: bool,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;

    let object_index = depth
        .checked_sub(2)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let object = opt_use(builder, stack[object_index]);
    let key = opt_use(builder, stack[object_index + 1]);
    let source_provenance = stack_provenance[object_index];
    let direct = builder.create_block();
    let deopt = builder.create_block();
    let continuation = builder.create_block();
    builder.append_block_param(continuation, types::I64);
    builder.append_block_param(continuation, types::I64);
    builder.append_block_param(continuation, pointer_type);
    builder.append_block_param(continuation, types::I32);
    builder.append_block_param(continuation, types::I8);

    let object_ok = builder
        .ins()
        .icmp_imm(IntCC::Equal, object.tag, i64::from(qjs::JS_TAG_OBJECT));
    let key_ok = builder
        .ins()
        .icmp_imm(IntCC::Equal, key.tag, i64::from(qjs::JS_TAG_INT));
    let tags_ok = builder.ins().band(object_ok, key_ok);
    let cached = guarded_source.find(stack_provenance[object_index]);
    let expected_kind = expected_mode.map(|mode| match mode {
        crate::runtime::ArrayMode::Packed => 0,
        crate::runtime::ArrayMode::Int32 => 1,
        crate::runtime::ArrayMode::Float64 => 2,
        // A generic site is never admitted by ArrayPlan. Keep this
        // fail-closed if that invariant changes.
        crate::runtime::ArrayMode::Generic => -1,
    });
    // Range analysis proved `index < bound` for the loop's single length
    // read; that deletes this check only against a count guarded equal to
    // that length. Only a hoist guard establishes `exact_length` (packed
    // logical length == dense count, or the intrinsic typed getter), so a
    // source established by an earlier load or store, or by an unguarded
    // metadata-only hoist, keeps its bounds check even when the planned
    // hoist that the proof relied on was not emitted on this path.
    let bounds_covered_by_hoist =
        bounds_covered_by_hoist && cached.is_some_and(|source| source.exact_length);
    // A cached source whose storage kind is a compile-time constant needs no
    // runtime kind dispatch. Its receiver was tag/class/storage guarded when
    // the source was established for this exact frame slot, and every frame
    // write or unadmitted effect since then invalidated it, so only the key
    // representation (unless it is a literal Int32 tag), the bounds check
    // (unless range analysis covered it) and the per-element packed value
    // check remain. A statically mismatched expected mode keeps the dynamic
    // path below, whose kind guard deopts.
    let static_cached = cached.and_then(|source| {
        let kind = source.static_kind?;
        ((0..=2).contains(&kind) && expected_kind.is_none_or(|expected| expected == kind))
            .then_some((source, kind))
    });
    let mut static_result = None;
    if let Some((source, kind)) = static_cached {
        // A literal Int32 key needs no representation exit.
        if !opt_tag_known(builder, key.tag, qjs::JS_TAG_INT) {
            let key_checked = builder.create_block();
            builder.ins().brif(key_ok, key_checked, &[], deopt, &[]);
            builder.seal_block(key_checked);
            builder.switch_to_block(key_checked);
        }
        let index = builder.ins().ireduce(types::I32, key.payload);
        if !bounds_covered_by_hoist {
            let in_bounds = builder
                .ins()
                .icmp(IntCC::UnsignedLessThan, index, source.count);
            let bounded = builder.create_block();
            builder.ins().brif(in_bounds, bounded, &[], deopt, &[]);
            builder.seal_block(bounded);
            builder.switch_to_block(bounded);
        }
        let result = match kind {
            0 => {
                let address = element_address::emit(builder, source.data, index, 16, pointer_type);
                let payload = builder.ins().load(types::I64, MemFlags::new(), address, 0);
                let tag =
                    builder
                        .ins()
                        .load(types::I64, MemFlags::new(), address, layout.value_tag);
                let primitive = builder
                    .ins()
                    .icmp_imm(IntCC::SignedGreaterThanOrEqual, tag, 0);
                let loaded = builder.create_block();
                builder.ins().brif(primitive, loaded, &[], deopt, &[]);
                builder.seal_block(loaded);
                builder.switch_to_block(loaded);
                OptPair { payload, tag }
            }
            1 => {
                let address = element_address::emit(builder, source.data, index, 4, pointer_type);
                let value = builder.ins().load(types::I32, MemFlags::new(), address, 0);
                OptPair {
                    payload: builder.ins().sextend(types::I64, value),
                    tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
                }
            }
            _ => {
                let address = element_address::emit(builder, source.data, index, 8, pointer_type);
                let value = builder.ins().load(types::F64, MemFlags::new(), address, 0);
                OptPair {
                    payload: builder.ins().bitcast(types::I64, MemFlags::new(), value),
                    tag: builder
                        .ins()
                        .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64)),
                }
            }
        };
        builder.ins().jump(
            continuation,
            &[
                result.payload,
                result.tag,
                source.data,
                source.count,
                source.kind,
            ],
        );
        // The continuation's only predecessor is this block, so these values
        // dominate it; using them directly keeps literal tags visible to the
        // consumers' representation checks.
        static_result = Some(result);
    }
    let retained_source = cached;
    let cached = cached.filter(|_| static_cached.is_none());
    let cached_index = cached.map(|_| builder.create_block());
    if static_cached.is_none() {
        builder
            .ins()
            .brif(tags_ok, cached_index.unwrap_or(direct), &[], deopt, &[]);
    }

    if let (Some(source), Some(cached_index)) = (cached, cached_index) {
        builder.switch_to_block(cached_index);
        if let Some(expected_kind) = expected_kind {
            let mode_ok = builder
                .ins()
                .icmp_imm(IntCC::Equal, source.kind, expected_kind);
            let mode_checked = builder.create_block();
            builder.ins().brif(mode_ok, mode_checked, &[], deopt, &[]);
            builder.switch_to_block(mode_checked);
        }
        let cached_dispatch = builder.create_block();
        let index = builder.ins().ireduce(types::I32, key.payload);
        if bounds_covered_by_hoist {
            // IntegerRangeAnalysis proved the Int32 induction value is
            // nonnegative and below the exact logical length guarded equal to
            // this cached dense count. Representation and overflow guards stay
            // in the loop; only this repeated comparison is deleted.
            builder.ins().jump(cached_dispatch, &[]);
        } else {
            let in_bounds = builder
                .ins()
                .icmp(IntCC::UnsignedLessThan, index, source.count);
            builder
                .ins()
                .brif(in_bounds, cached_dispatch, &[], deopt, &[]);
        }
        builder.switch_to_block(cached_dispatch);
        let packed_kind = builder.ins().icmp_imm(IntCC::Equal, source.kind, 0);
        let cached_packed = builder.create_block();
        let cached_typed = builder.create_block();
        builder
            .ins()
            .brif(packed_kind, cached_packed, &[], cached_typed, &[]);
        builder.switch_to_block(cached_packed);
        let address = element_address::emit(builder, source.data, index, 16, pointer_type);
        let payload = builder.ins().load(types::I64, MemFlags::new(), address, 0);
        let tag = builder
            .ins()
            .load(types::I64, MemFlags::new(), address, layout.value_tag);
        let primitive = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, tag, 0);
        let cached_packed_done = builder.create_block();
        builder
            .ins()
            .brif(primitive, cached_packed_done, &[], deopt, &[]);
        builder.switch_to_block(cached_packed_done);
        builder.ins().jump(
            continuation,
            &[payload, tag, source.data, source.count, source.kind],
        );
        builder.switch_to_block(cached_typed);
        let int_kind = builder.ins().icmp_imm(IntCC::Equal, source.kind, 1);
        let cached_int = builder.create_block();
        let cached_float = builder.create_block();
        builder
            .ins()
            .brif(int_kind, cached_int, &[], cached_float, &[]);
        builder.switch_to_block(cached_int);
        let address = element_address::emit(builder, source.data, index, 4, pointer_type);
        let value = builder.ins().load(types::I32, MemFlags::new(), address, 0);
        let value = builder.ins().sextend(types::I64, value);
        let tag = builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT));
        builder.ins().jump(
            continuation,
            &[value, tag, source.data, source.count, source.kind],
        );
        builder.switch_to_block(cached_float);
        let address = element_address::emit(builder, source.data, index, 8, pointer_type);
        let value = builder.ins().load(types::F64, MemFlags::new(), address, 0);
        let value = builder.ins().bitcast(types::I64, MemFlags::new(), value);
        let tag = builder
            .ins()
            .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64));
        builder.ins().jump(
            continuation,
            &[value, tag, source.data, source.count, source.kind],
        );
    }

    if cached.is_none() && static_cached.is_none() {
        let classify = builder.create_block();
        let packed = builder.create_block();
        let int32 = builder.create_block();
        let float64 = builder.create_block();
        let typed_common = builder.create_block();
        builder.append_block_param(typed_common, types::I8);
        builder.switch_to_block(direct);
        let index = builder.ins().ireduce(types::I32, key.payload);
        let non_negative = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, index, 0);
        builder.ins().brif(non_negative, classify, &[], deopt, &[]);

        builder.switch_to_block(classify);
        let flags = builder.ins().load(
            types::I8,
            MemFlags::new(),
            object.payload,
            element_layout.object_flags_offset,
        );
        let fast = builder
            .ins()
            .band_imm(flags, element_layout.object_fast_array_mask);
        let fast = builder.ins().icmp_imm(IntCC::NotEqual, fast, 0);
        let class_check = builder.create_block();
        builder.ins().brif(fast, class_check, &[], deopt, &[]);
        builder.switch_to_block(class_check);
        let class = builder.ins().load(
            types::I16,
            MemFlags::new(),
            object.payload,
            element_layout.object_class_id_offset,
        );
        let class = builder.ins().uextend(types::I64, class);
        let is_packed = builder
            .ins()
            .icmp_imm(IntCC::Equal, class, element_layout.array_class_id);
        let not_packed = builder.create_block();
        let packed_allowed =
            expected_mode.is_none_or(|mode| mode == crate::runtime::ArrayMode::Packed);
        let packed_match = if packed_allowed { packed } else { deopt };
        let packed_miss = if expected_mode == Some(crate::runtime::ArrayMode::Packed) {
            deopt
        } else {
            not_packed
        };
        builder
            .ins()
            .brif(is_packed, packed_match, &[], packed_miss, &[]);
        builder.switch_to_block(not_packed);
        let is_int32 =
            builder
                .ins()
                .icmp_imm(IntCC::Equal, class, element_layout.int32_array_class_id);
        let not_int32 = builder.create_block();
        let int32_allowed =
            expected_mode.is_none_or(|mode| mode == crate::runtime::ArrayMode::Int32);
        let int32_match = if int32_allowed { int32 } else { deopt };
        let int32_miss = if expected_mode == Some(crate::runtime::ArrayMode::Int32) {
            deopt
        } else {
            not_int32
        };
        builder
            .ins()
            .brif(is_int32, int32_match, &[], int32_miss, &[]);
        builder.switch_to_block(not_int32);
        let is_float64 =
            builder
                .ins()
                .icmp_imm(IntCC::Equal, class, element_layout.float64_array_class_id);
        let float64_allowed =
            expected_mode.is_none_or(|mode| mode == crate::runtime::ArrayMode::Float64);
        let float64_match = if float64_allowed { float64 } else { deopt };
        builder
            .ins()
            .brif(is_float64, float64_match, &[], deopt, &[]);

        builder.switch_to_block(packed);
        let count = builder.ins().load(
            types::I32,
            MemFlags::new(),
            object.payload,
            element_layout.array_count_offset,
        );
        let in_bounds = builder.ins().icmp(IntCC::UnsignedLessThan, index, count);
        let packed_load = builder.create_block();
        builder.ins().brif(in_bounds, packed_load, &[], deopt, &[]);
        builder.switch_to_block(packed_load);
        let data = builder.ins().load(
            pointer_type,
            MemFlags::new(),
            object.payload,
            element_layout.array_data_offset,
        );
        let has_data = builder.ins().icmp_imm(IntCC::NotEqual, data, 0);
        let packed_value = builder.create_block();
        builder.ins().brif(has_data, packed_value, &[], deopt, &[]);
        builder.switch_to_block(packed_value);
        let address = element_address::emit(builder, data, index, 16, pointer_type);
        let payload = builder.ins().load(types::I64, MemFlags::new(), address, 0);
        let tag = builder
            .ins()
            .load(types::I64, MemFlags::new(), address, layout.value_tag);
        let primitive = builder
            .ins()
            .icmp_imm(IntCC::SignedGreaterThanOrEqual, tag, 0);
        let packed_done = builder.create_block();
        builder.ins().brif(primitive, packed_done, &[], deopt, &[]);
        builder.switch_to_block(packed_done);
        let cached_kind = builder.ins().iconst(types::I8, 0);
        builder
            .ins()
            .jump(continuation, &[payload, tag, data, count, cached_kind]);

        builder.switch_to_block(int32);
        let kind = builder.ins().iconst(types::I8, 0);
        builder.ins().jump(typed_common, &[kind]);
        builder.switch_to_block(float64);
        let kind = builder.ins().iconst(types::I8, 1);
        builder.ins().jump(typed_common, &[kind]);

        builder.switch_to_block(typed_common);
        let typed = builder.ins().load(
            pointer_type,
            MemFlags::new(),
            object.payload,
            element_layout.typed_array_ptr_offset,
        );
        let has_typed = builder.ins().icmp_imm(IntCC::NotEqual, typed, 0);
        let typed_guard = builder.create_block();
        builder.ins().brif(has_typed, typed_guard, &[], deopt, &[]);
        builder.switch_to_block(typed_guard);
        let tracks_resizable = builder.ins().load(
            types::I8,
            MemFlags::new(),
            typed,
            element_layout.typed_array_track_rab_offset,
        );
        let stable = builder.ins().icmp_imm(IntCC::Equal, tracks_resizable, 0);
        let stable_buffer = builder.create_block();
        builder.ins().brif(stable, stable_buffer, &[], deopt, &[]);
        builder.switch_to_block(stable_buffer);
        let buffer = builder.ins().load(
            pointer_type,
            MemFlags::new(),
            typed,
            element_layout.typed_array_buffer_offset,
        );
        let has_buffer = builder.ins().icmp_imm(IntCC::NotEqual, buffer, 0);
        let buffer_object = builder.create_block();
        builder
            .ins()
            .brif(has_buffer, buffer_object, &[], deopt, &[]);
        builder.switch_to_block(buffer_object);
        let array_buffer = builder.ins().load(
            pointer_type,
            MemFlags::new(),
            buffer,
            element_layout.object_union_offset,
        );
        let has_array_buffer = builder.ins().icmp_imm(IntCC::NotEqual, array_buffer, 0);
        let detach_guard = builder.create_block();
        builder
            .ins()
            .brif(has_array_buffer, detach_guard, &[], deopt, &[]);
        builder.switch_to_block(detach_guard);
        let detached = builder.ins().load(
            types::I8,
            MemFlags::new(),
            array_buffer,
            element_layout.array_buffer_detached_offset,
        );
        let attached = builder.ins().icmp_imm(IntCC::Equal, detached, 0);
        let typed_bounds = builder.create_block();
        builder.ins().brif(attached, typed_bounds, &[], deopt, &[]);
        builder.switch_to_block(typed_bounds);
        let count = builder.ins().load(
            types::I32,
            MemFlags::new(),
            object.payload,
            element_layout.array_count_offset,
        );
        let in_bounds = builder.ins().icmp(IntCC::UnsignedLessThan, index, count);
        let typed_data = builder.create_block();
        builder.ins().brif(in_bounds, typed_data, &[], deopt, &[]);
        builder.switch_to_block(typed_data);
        let data = builder.ins().load(
            pointer_type,
            MemFlags::new(),
            object.payload,
            element_layout.array_data_offset,
        );
        let has_data = builder.ins().icmp_imm(IntCC::NotEqual, data, 0);
        let typed_load = builder.create_block();
        builder.ins().brif(has_data, typed_load, &[], deopt, &[]);
        builder.switch_to_block(typed_load);
        let kind = builder.block_params(typed_common)[0];
        let is_float = builder.ins().icmp_imm(IntCC::NotEqual, kind, 0);
        let load_i32 = builder.create_block();
        let load_f64 = builder.create_block();
        builder.ins().brif(is_float, load_f64, &[], load_i32, &[]);
        builder.switch_to_block(load_i32);
        let address = element_address::emit(builder, data, index, 4, pointer_type);
        let value = builder.ins().load(types::I32, MemFlags::new(), address, 0);
        let value = builder.ins().sextend(types::I64, value);
        let tag = builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT));
        let cached_kind = builder.ins().iconst(types::I8, 1);
        builder
            .ins()
            .jump(continuation, &[value, tag, data, count, cached_kind]);
        builder.switch_to_block(load_f64);
        let address = element_address::emit(builder, data, index, 8, pointer_type);
        let value = builder.ins().load(types::F64, MemFlags::new(), address, 0);
        let value = builder.ins().bitcast(types::I64, MemFlags::new(), value);
        let tag = builder
            .ins()
            .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64));
        let cached_kind = builder.ins().iconst(types::I8, 2);
        builder
            .ins()
            .jump(continuation, &[value, tag, data, count, cached_kind]);
    }

    // All edges into the shared exit and the continuation are declared once
    // the arms above are emitted; nothing below branches to either again.
    builder.seal_block(deopt);
    builder.seal_block(continuation);
    builder.switch_to_block(deopt);
    for (index, vars) in arguments.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, arg_buf, index, value);
    }
    for (index, vars) in locals.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, var_buf, index, value);
    }
    for (index, vars) in stack.iter().take(depth).enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, stack_base, index, value);
    }
    opt_set_stack_top(builder, frame, stack_base, depth, pointer_type, layout);
    let start = builder
        .ins()
        .load(pointer_type, MemFlags::new(), frame, layout.bytecode_start);
    let resume = builder.ins().iadd_imm(start, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), resume, frame, layout.pc);
    opt_own_stack_for_exit(
        builder,
        frame,
        sret,
        stack_base,
        depth,
        arguments.len() + locals.len(),
        stack_provenance,
        helper_signatures,
        pointer_type,
        layout,
    )?;
    emit_opt_exit(
        builder,
        sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
        Some(resume),
        pointer_type,
        guard,
    );

    builder.switch_to_block(continuation);
    let result = static_result.unwrap_or_else(|| OptPair {
        payload: builder.block_params(continuation)[0],
        tag: builder.block_params(continuation)[1],
    });
    let params = builder.block_params(continuation);
    // A load neither reenters nor alters storage, so other live tuples stay
    // valid. Only an uncached packed load publishes its own metadata.
    if retained_source.is_none() && expected_mode == Some(crate::runtime::ArrayMode::Packed) {
        guarded_source.insert(GuardedElementSource {
            provenance: source_provenance,
            block_pc,
            data: params[2],
            count: params[3],
            kind: params[4],
            exact_length: false,
            typed_mode: None,
            // Only the packed arm reaches the continuation for this mode.
            static_kind: Some(0),
            unguarded: false,
        });
    }
    opt_define(builder, stack[object_index], result);
    stack_provenance[object_index] = OptProvenance::ImmediatePrimitive;
    stack_provenance[object_index + 1] = OptProvenance::Unknown;
    Ok(object_index + 1)
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_typed_store(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &mut [OptProvenance],
    depth: usize,
    pc: u32,
    guard: u32,
    block_pc: u32,
    mode: crate::runtime::ArrayMode,
    array_query: usize,
    guarded_source: &mut ElementSources,
) -> Result<usize, CompileFailure> {
    use crate::runtime::ArrayMode;
    use cranelift_codegen::ir::{condcodes::IntCC, types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;

    let base = depth
        .checked_sub(3)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let object = opt_use(builder, env.stack[base]);
    let key = opt_use(builder, env.stack[base + 1]);
    let value = opt_use(builder, env.stack[base + 2]);
    let key_int = opt_tag_is(builder, key.tag, qjs::JS_TAG_INT);
    let value_int = opt_tag_is(builder, value.tag, qjs::JS_TAG_INT);
    let value_ok = match mode {
        // ToInt32 of an Int32 is exact. Float64 -> Int32, BigInt and all
        // potentially reentrant coercions resume QuickJS at the store.
        ArrayMode::Int32 => value_int,
        ArrayMode::Float64 => {
            let value_float = opt_tag_is(builder, value.tag, qjs::JS_TAG_FLOAT64);
            builder.ins().bor(value_int, value_float)
        }
        _ => return Err(CompileFailure::InvalidArtifact),
    };
    let operands_ok = builder.ins().band(key_int, value_ok);
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, operands_ok)?;
    let source = match guarded_source
        .find(provenance[base])
        .filter(|source| source.typed_mode == Some(mode))
    {
        Some(source) => source,
        None => emit_opt_typed_metadata_guard(
            builder,
            env,
            provenance,
            depth,
            pc,
            guard,
            object,
            provenance[base],
            block_pc,
            mode,
            array_query,
            false,
            true,
        )?,
    };
    let index = builder.ins().ireduce(types::I32, key.payload);
    let in_bounds = builder
        .ins()
        .icmp(IntCC::UnsignedLessThan, index, source.count);
    emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, in_bounds)?;
    let width = if mode == ArrayMode::Int32 { 4 } else { 8 };
    let address = element_address::emit(builder, source.data, index, width, env.pointer_type);
    let scalar = if mode == ArrayMode::Int32 {
        builder.ins().ireduce(types::I32, value.payload)
    } else {
        opt_f64(builder, value)
    };
    // All exits precede this write. The continuing path neither allocates nor
    // releases an owner, and fixed nonshared storage cannot change underneath
    // it. Aliased views observe the write immediately; no element is forwarded.
    builder.ins().store(MemFlags::new(), scalar, address, 0);
    guarded_source.insert(source);
    provenance[base..depth].fill(OptProvenance::Unknown);
    Ok(base)
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_element_put(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    arg_buf: cranelift_codegen::ir::Value,
    var_buf: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    arguments: &[OptVars],
    locals: &[OptVars],
    stack: &[OptVars],
    stack_provenance: &mut [OptProvenance],
    depth: usize,
    pc: u32,
    guard: u32,
    helper_signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
    element_layout: crate::abi::ElementLayout,
    block_pc: u32,
    source_provenance: OptProvenance,
    guarded_source: &mut ElementSources,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;

    let object_index = depth
        .checked_sub(3)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let object = opt_use(builder, stack[object_index]);
    let key = opt_use(builder, stack[object_index + 1]);
    let value = opt_use(builder, stack[object_index + 2]);
    let direct = builder.create_block();
    let deopt = builder.create_block();
    let classify = builder.create_block();
    let packed = builder.create_block();
    let int32 = builder.create_block();
    let float64 = builder.create_block();
    let typed_common = builder.create_block();
    let continuation = builder.create_block();
    builder.append_block_param(typed_common, types::I8);
    builder.append_block_param(continuation, types::I32);
    builder.append_block_param(continuation, pointer_type);
    builder.append_block_param(continuation, types::I8);

    let object_ok = builder
        .ins()
        .icmp_imm(IntCC::Equal, object.tag, i64::from(qjs::JS_TAG_OBJECT));
    let key_ok = builder
        .ins()
        .icmp_imm(IntCC::Equal, key.tag, i64::from(qjs::JS_TAG_INT));
    let tags_ok = builder.ins().band(object_ok, key_ok);
    builder.ins().brif(tags_ok, direct, &[], deopt, &[]);
    builder.switch_to_block(direct);
    let index = builder.ins().ireduce(types::I32, key.payload);
    let non_negative = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, index, 0);
    builder.ins().brif(non_negative, classify, &[], deopt, &[]);

    builder.switch_to_block(classify);
    let flags = builder.ins().load(
        types::I8,
        MemFlags::new(),
        object.payload,
        element_layout.object_flags_offset,
    );
    let fast = builder
        .ins()
        .band_imm(flags, element_layout.object_fast_array_mask);
    let fast = builder.ins().icmp_imm(IntCC::NotEqual, fast, 0);
    let class_check = builder.create_block();
    builder.ins().brif(fast, class_check, &[], deopt, &[]);
    builder.switch_to_block(class_check);
    let class = builder.ins().load(
        types::I16,
        MemFlags::new(),
        object.payload,
        element_layout.object_class_id_offset,
    );
    let class = builder.ins().uextend(types::I64, class);
    let is_packed = builder
        .ins()
        .icmp_imm(IntCC::Equal, class, element_layout.array_class_id);
    let not_packed = builder.create_block();
    builder.ins().brif(is_packed, packed, &[], not_packed, &[]);
    builder.switch_to_block(not_packed);
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
    builder.ins().brif(is_float64, float64, &[], deopt, &[]);

    builder.switch_to_block(packed);
    let count = builder.ins().load(
        types::I32,
        MemFlags::new(),
        object.payload,
        element_layout.array_count_offset,
    );
    let in_bounds = builder.ins().icmp(IntCC::UnsignedLessThan, index, count);
    let primitive = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, value.tag, 0);
    let packed_ok = builder.ins().band(in_bounds, primitive);
    let packed_data = builder.create_block();
    builder.ins().brif(packed_ok, packed_data, &[], deopt, &[]);
    builder.switch_to_block(packed_data);
    let data = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        object.payload,
        element_layout.array_data_offset,
    );
    let has_data = builder.ins().icmp_imm(IntCC::NotEqual, data, 0);
    let packed_store = builder.create_block();
    builder.ins().brif(has_data, packed_store, &[], deopt, &[]);
    builder.switch_to_block(packed_store);
    let address = element_address::emit(builder, data, index, 16, pointer_type);
    let old_tag = builder
        .ins()
        .load(types::I64, MemFlags::new(), address, layout.value_tag);
    let old_primitive = builder
        .ins()
        .icmp_imm(IntCC::SignedGreaterThanOrEqual, old_tag, 0);
    let do_packed_store = builder.create_block();
    builder
        .ins()
        .brif(old_primitive, do_packed_store, &[], deopt, &[]);
    builder.switch_to_block(do_packed_store);
    builder
        .ins()
        .store(MemFlags::new(), value.payload, address, 0);
    builder
        .ins()
        .store(MemFlags::new(), value.tag, address, layout.value_tag);
    let kind = builder.ins().iconst(types::I8, 0);
    builder.ins().jump(continuation, &[count, data, kind]);

    builder.switch_to_block(int32);
    let int_value = builder
        .ins()
        .icmp_imm(IntCC::Equal, value.tag, i64::from(qjs::JS_TAG_INT));
    let int_typed = builder.create_block();
    builder.ins().brif(int_value, int_typed, &[], deopt, &[]);
    builder.switch_to_block(int_typed);
    let kind = builder.ins().iconst(types::I8, 0);
    builder.ins().jump(typed_common, &[kind]);
    builder.switch_to_block(float64);
    let value_int = builder
        .ins()
        .icmp_imm(IntCC::Equal, value.tag, i64::from(qjs::JS_TAG_INT));
    let value_float =
        builder
            .ins()
            .icmp_imm(IntCC::Equal, value.tag, i64::from(qjs::JS_TAG_FLOAT64));
    let numeric = builder.ins().bor(value_int, value_float);
    let float_typed = builder.create_block();
    builder.ins().brif(numeric, float_typed, &[], deopt, &[]);
    builder.switch_to_block(float_typed);
    let kind = builder.ins().iconst(types::I8, 1);
    builder.ins().jump(typed_common, &[kind]);

    builder.switch_to_block(typed_common);
    let typed_data = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        object.payload,
        element_layout.typed_array_ptr_offset,
    );
    let has_typed = builder.ins().icmp_imm(IntCC::NotEqual, typed_data, 0);
    let stable_check = builder.create_block();
    builder.ins().brif(has_typed, stable_check, &[], deopt, &[]);
    builder.switch_to_block(stable_check);
    let tracks_resizable = builder.ins().load(
        types::I8,
        MemFlags::new(),
        typed_data,
        element_layout.typed_array_track_rab_offset,
    );
    let stable = builder.ins().icmp_imm(IntCC::Equal, tracks_resizable, 0);
    let buffer_check = builder.create_block();
    builder.ins().brif(stable, buffer_check, &[], deopt, &[]);
    builder.switch_to_block(buffer_check);
    let buffer = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        typed_data,
        element_layout.typed_array_buffer_offset,
    );
    let has_buffer = builder.ins().icmp_imm(IntCC::NotEqual, buffer, 0);
    let array_buffer_check = builder.create_block();
    builder
        .ins()
        .brif(has_buffer, array_buffer_check, &[], deopt, &[]);
    builder.switch_to_block(array_buffer_check);
    let array_buffer = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        buffer,
        element_layout.object_union_offset,
    );
    let has_array_buffer = builder.ins().icmp_imm(IntCC::NotEqual, array_buffer, 0);
    let buffer_state = builder.create_block();
    builder
        .ins()
        .brif(has_array_buffer, buffer_state, &[], deopt, &[]);
    builder.switch_to_block(buffer_state);
    let detached = builder.ins().load(
        types::I8,
        MemFlags::new(),
        array_buffer,
        element_layout.array_buffer_detached_offset,
    );
    let immutable = builder.ins().load(
        types::I8,
        MemFlags::new(),
        array_buffer,
        element_layout.array_buffer_immutable_offset,
    );
    let attached = builder.ins().icmp_imm(IntCC::Equal, detached, 0);
    let mutable = builder.ins().icmp_imm(IntCC::Equal, immutable, 0);
    let usable = builder.ins().band(attached, mutable);
    let bounds_check = builder.create_block();
    builder.ins().brif(usable, bounds_check, &[], deopt, &[]);
    builder.switch_to_block(bounds_check);
    let count = builder.ins().load(
        types::I32,
        MemFlags::new(),
        object.payload,
        element_layout.array_count_offset,
    );
    let in_bounds = builder.ins().icmp(IntCC::UnsignedLessThan, index, count);
    let data_check = builder.create_block();
    builder.ins().brif(in_bounds, data_check, &[], deopt, &[]);
    builder.switch_to_block(data_check);
    let data = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        object.payload,
        element_layout.array_data_offset,
    );
    let has_data = builder.ins().icmp_imm(IntCC::NotEqual, data, 0);
    let typed_store = builder.create_block();
    builder.ins().brif(has_data, typed_store, &[], deopt, &[]);
    builder.switch_to_block(typed_store);
    let kind = builder.block_params(typed_common)[0];
    let is_float = builder.ins().icmp_imm(IntCC::NotEqual, kind, 0);
    let store_i32 = builder.create_block();
    let store_f64 = builder.create_block();
    builder.ins().brif(is_float, store_f64, &[], store_i32, &[]);
    builder.switch_to_block(store_i32);
    let address = element_address::emit(builder, data, index, 4, pointer_type);
    let scalar = builder.ins().ireduce(types::I32, value.payload);
    builder.ins().store(MemFlags::new(), scalar, address, 0);
    let cached_kind = builder.ins().iconst(types::I8, 1);
    builder
        .ins()
        .jump(continuation, &[count, data, cached_kind]);
    builder.switch_to_block(store_f64);
    let address = element_address::emit(builder, data, index, 8, pointer_type);
    let scalar = opt_f64(builder, value);
    builder.ins().store(MemFlags::new(), scalar, address, 0);
    let cached_kind = builder.ins().iconst(types::I8, 2);
    builder
        .ins()
        .jump(continuation, &[count, data, cached_kind]);

    builder.switch_to_block(deopt);
    for (slot, vars) in arguments.iter().enumerate() {
        let current = opt_use(builder, *vars);
        opt_store(builder, arg_buf, slot, current);
    }
    for (slot, vars) in locals.iter().enumerate() {
        let current = opt_use(builder, *vars);
        opt_store(builder, var_buf, slot, current);
    }
    for (slot, vars) in stack.iter().take(depth).enumerate() {
        let current = opt_use(builder, *vars);
        opt_store(builder, stack_base, slot, current);
    }
    opt_set_stack_top(builder, frame, stack_base, depth, pointer_type, layout);
    let start = builder
        .ins()
        .load(pointer_type, MemFlags::new(), frame, layout.bytecode_start);
    let resume = builder.ins().iadd_imm(start, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), resume, frame, layout.pc);
    opt_own_stack_for_exit(
        builder,
        frame,
        sret,
        stack_base,
        depth,
        arguments.len() + locals.len(),
        stack_provenance,
        helper_signatures,
        pointer_type,
        layout,
    )?;
    emit_opt_exit(
        builder,
        sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
        Some(resume),
        pointer_type,
        guard,
    );

    builder.switch_to_block(continuation);
    let params = builder.block_params(continuation);
    // In-bounds primitive-over-primitive stores neither grow, reallocate nor
    // release; other live tuples (including aliases of this array) stay valid.
    guarded_source.insert(GuardedElementSource {
        provenance: source_provenance,
        block_pc,
        count: params[0],
        data: params[1],
        kind: params[2],
        exact_length: false,
        typed_mode: None,
        static_kind: None,
        unguarded: false,
    });
    for provenance in &mut stack_provenance[object_index..depth] {
        *provenance = OptProvenance::Unknown;
    }
    Ok(object_index)
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_guarded_property(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    arg_buf: cranelift_codegen::ir::Value,
    var_buf: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    arguments: &[OptVars],
    locals: &[OptVars],
    stack: &[OptVars],
    stack_provenance: &mut [OptProvenance],
    depth: usize,
    store: bool,
    properties: &[crate::runtime::ShapeObservation],
    pc: u32,
    guard: u32,
    helper_signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;
    let object_index = depth
        .checked_sub(if store { 2 } else { 1 })
        .ok_or(CompileFailure::InvalidArtifact)?;
    // A leaf shape guard can borrow the receiver from an existing root. This
    // requires every live stack value to be either rooted there or primitive;
    // no temporary owner may disappear from the helper's visible stack.
    let borrowed_slot = if stack_provenance[..depth].iter().all(|source| {
        matches!(
            source,
            OptProvenance::Argument(_)
                | OptProvenance::Local(_)
                | OptProvenance::ImmediatePrimitive
        )
    }) {
        match stack_provenance[object_index] {
            OptProvenance::Argument(index) => Some(index),
            OptProvenance::Local(index) => arguments.len().checked_add(index),
            _ => None,
        }
    } else {
        None
    };
    if borrowed_slot.is_none() {
        for (index, vars) in arguments.iter().enumerate() {
            let v = opt_use(builder, *vars);
            opt_store(builder, arg_buf, index, v);
        }
        for (index, vars) in locals.iter().enumerate() {
            let v = opt_use(builder, *vars);
            opt_store(builder, var_buf, index, v);
        }
        for (index, vars) in stack.iter().take(depth).enumerate() {
            let v = opt_use(builder, *vars);
            opt_store(builder, stack_base, index, v);
        }
        let bytecode =
            builder
                .ins()
                .load(pointer_type, MemFlags::new(), frame, layout.bytecode_start);
        let current_pc = builder.ins().iadd_imm(bytecode, i64::from(pc));
        builder
            .ins()
            .store(MemFlags::new(), current_pc, frame, layout.pc);
        opt_own_stack_for_helper(
            builder,
            frame,
            sret,
            stack_base,
            depth,
            arguments.len() + locals.len(),
            stack_provenance,
            helper_signatures,
            pointer_type,
            layout,
        )?;
    }
    if properties.is_empty() || properties.len() > crate::runtime::POLYMORPHIC_PROPERTY_LIMIT {
        return Err(CompileFailure::InvalidArtifact);
    }
    for (index, property) in properties.iter().enumerate() {
        // Feedback retains one observation per shape identity; a duplicate
        // would make the first matching compare select a stale slot.
        if property.shape().identity() == 0
            || property.shape().generation() == 0
            || properties[..index]
                .iter()
                .any(|prior| prior.shape().identity() == property.shape().identity())
        {
            return Err(CompileFailure::InvalidArtifact);
        }
    }
    let property_layout = crate::abi::AbiInfo::linked()
        .map_err(|_| CompileFailure::InvalidArtifact)?
        .property_layout();
    let deopt = builder.create_block();
    let continuation = builder.create_block();
    if !store {
        builder.append_block_param(continuation, types::I64);
        builder.append_block_param(continuation, types::I64);
    }
    // Inline polymorphic dispatch for both borrowed and owned receivers: the
    // receiver's shape pointer and monotonic layout generation are exactly the
    // SHAPE_GUARD helper predicate, so no helper crossing is needed. The owned
    // path above has already published the frame for the deopt exit.
    let object = opt_use(builder, stack[object_index]);
    let is_object = builder
        .ins()
        .icmp_imm(IntCC::Equal, object.tag, i64::from(qjs::JS_TAG_OBJECT));
    let dispatch = builder.create_block();
    builder.ins().brif(is_object, dispatch, &[], deopt, &[]);
    builder.switch_to_block(dispatch);
    // Read only the live receiver's shape. The feedback pointer is an integer
    // identity, never a pointer we dereference or retain. A live object's
    // shape is always valid, so its generation is loaded once for the chain.
    let shape = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        object.payload,
        property_layout.object_shape_offset,
    );
    let live_generation = builder.ins().load(
        types::I64,
        MemFlags::new(),
        shape,
        property_layout.shape_generation_offset,
    );
    for (index, property) in properties.iter().copied().enumerate() {
        let id = property.shape().identity();
        let generation = property.shape().generation();
        let access = builder.create_block();
        let check = builder.create_block();
        let next = if index + 1 == properties.len() {
            deopt
        } else {
            builder.create_block()
        };
        builder.ins().jump(check, &[]);
        builder.switch_to_block(check);
        let same_shape = builder.ins().icmp_imm(IntCC::Equal, shape, id as i64);
        let same_generation =
            builder
                .ins()
                .icmp_imm(IntCC::Equal, live_generation, generation as i64);
        let matches = builder.ins().band(same_shape, same_generation);
        builder.ins().brif(matches, access, &[], next, &[]);
        builder.switch_to_block(access);
        let object = opt_use(builder, stack[object_index]);
        let props = builder.ins().load(
            pointer_type,
            MemFlags::new(),
            object.payload,
            property_layout.object_properties_offset,
        );
        let offset = i32::try_from(
            usize::try_from(property.offset())
                .map_err(|_| CompileFailure::ResourceLimit)?
                .checked_mul(16)
                .ok_or(CompileFailure::ResourceLimit)?,
        )
        .map_err(|_| CompileFailure::ResourceLimit)?;
        let current = OptPair {
            payload: builder
                .ins()
                .load(types::I64, MemFlags::new(), props, offset),
            tag: builder
                .ins()
                .load(types::I64, MemFlags::new(), props, offset + 8),
        };
        let expected_tag = match property.value() {
            crate::runtime::ObservedType::Int32 => qjs::JS_TAG_INT,
            crate::runtime::ObservedType::Float64 => qjs::JS_TAG_FLOAT64,
            crate::runtime::ObservedType::Bool => qjs::JS_TAG_BOOL,
            crate::runtime::ObservedType::Null => qjs::JS_TAG_NULL,
            crate::runtime::ObservedType::Undefined => qjs::JS_TAG_UNDEFINED,
            _ => return Err(CompileFailure::InvalidArtifact),
        };
        let mut tags_ok =
            builder
                .ins()
                .icmp_imm(IntCC::Equal, current.tag, i64::from(expected_tag));
        if store {
            let value = opt_use(builder, stack[depth - 1]);
            let input_ok = builder
                .ins()
                .icmp_imm(IntCC::Equal, value.tag, i64::from(expected_tag));
            tags_ok = builder.ins().band(tags_ok, input_ok);
            let do_access = builder.create_block();
            builder.ins().brif(tags_ok, do_access, &[], deopt, &[]);
            builder.switch_to_block(do_access);
            opt_store_at(builder, props, offset, value);
            builder.ins().jump(continuation, &[]);
        } else {
            let do_access = builder.create_block();
            builder.ins().brif(tags_ok, do_access, &[], deopt, &[]);
            builder.switch_to_block(do_access);
            builder
                .ins()
                .jump(continuation, &[current.payload, current.tag]);
        }
        if index + 1 != properties.len() {
            builder.switch_to_block(next);
        }
    }
    builder.switch_to_block(deopt);
    if borrowed_slot.is_some() {
        // No state publication or helper crossing occurs on the native hit.
        // Rebuild exactly the frame the ownership bridge expects on a miss.
        for (index, vars) in arguments.iter().enumerate() {
            let value = opt_use(builder, *vars);
            opt_store(builder, arg_buf, index, value);
        }
        for (index, vars) in locals.iter().enumerate() {
            let value = opt_use(builder, *vars);
            opt_store(builder, var_buf, index, value);
        }
        let bytecode =
            builder
                .ins()
                .load(pointer_type, MemFlags::new(), frame, layout.bytecode_start);
        let current_pc = builder.ins().iadd_imm(bytecode, i64::from(pc));
        builder
            .ins()
            .store(MemFlags::new(), current_pc, frame, layout.pc);
        opt_set_stack_top(builder, frame, stack_base, 0, pointer_type, layout);
        // The leaf guard sees only argument/local roots. Materialize the exact
        // pre-op stack now, including primitive results from earlier accesses,
        // before the owner bridge publishes it to the interpreter.
        for (index, vars) in stack.iter().take(depth).enumerate() {
            let v = opt_use(builder, *vars);
            opt_store(builder, stack_base, index, v);
        }
        opt_own_stack_for_exit(
            builder,
            frame,
            sret,
            stack_base,
            depth,
            arguments.len() + locals.len(),
            stack_provenance,
            helper_signatures,
            pointer_type,
            layout,
        )?;
    }
    let start = builder
        .ins()
        .load(pointer_type, MemFlags::new(), frame, layout.bytecode_start);
    let resume = builder.ins().iadd_imm(start, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), resume, frame, layout.pc);
    emit_opt_exit(
        builder,
        sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
        Some(resume),
        pointer_type,
        guard,
    );
    builder.switch_to_block(continuation);
    if borrowed_slot.is_some() {
        // Publish only the surviving stack prefix. Borrowed aliases remain in
        // SSA and get non-owning placeholders; primitives retain their values.
        // The consumed receiver/value never needs a stack store on a hit.
        let undefined = OptPair {
            payload: builder.ins().iconst(types::I64, 0),
            tag: builder
                .ins()
                .iconst(types::I64, i64::from(qjs::JS_TAG_UNDEFINED)),
        };
        for (index, provenance) in stack_provenance.iter().take(object_index).enumerate() {
            let value = if matches!(provenance, OptProvenance::ImmediatePrimitive) {
                opt_use(builder, stack[index])
            } else {
                undefined
            };
            opt_store(builder, stack_base, index, value);
        }
        opt_set_stack_top(
            builder,
            frame,
            stack_base,
            object_index,
            pointer_type,
            layout,
        );
    } else {
        // Every borrowed alias below the operands was materialized as an owner
        // for the guard's exception path; hand those references back before the
        // operand slots are released, or each guarded access leaks one.
        opt_release_materialized_aliases(
            builder,
            frame,
            sret,
            stack_base,
            stack_provenance,
            0..object_index,
            depth,
            arguments.len() + locals.len(),
            helper_signatures,
            pointer_type,
            layout,
        )?;
        opt_release_owned_stack(
            builder,
            frame,
            sret,
            stack_base,
            object_index,
            depth,
            arguments.len() + locals.len(),
            helper_signatures,
            pointer_type,
            layout,
        )?;
    }
    if store {
        for provenance in &mut stack_provenance[object_index..depth] {
            *provenance = OptProvenance::Unknown;
        }
        Ok(depth - 2)
    } else {
        let params = builder.block_params(continuation);
        let current = OptPair {
            payload: params[0],
            tag: params[1],
        };
        opt_define(builder, stack[object_index], current);
        stack_provenance[object_index] = OptProvenance::ImmediatePrimitive;
        opt_store(builder, stack_base, object_index, current);
        opt_set_stack_top(builder, frame, stack_base, depth, pointer_type, layout);
        Ok(depth)
    }
}

/// Releases the owned duplicates that `opt_own_stack_for_exit` created for
/// borrowed argument/local aliases *below* an operation's operands, once
/// that operation continues natively. The aliases keep their provenance:
/// the SSA value still borrows from the argument or local buffer, and the
/// interpreter slot must not own a reference that nobody consumes.
#[allow(clippy::too_many_arguments)]
fn opt_release_materialized_aliases(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    provenance: &[OptProvenance],
    range: core::ops::Range<usize>,
    exception_depth: usize,
    flat_stack_base: usize,
    signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
) -> Result<(), CompileFailure> {
    for index in range {
        if !matches!(
            provenance.get(index),
            Some(OptProvenance::Argument(_) | OptProvenance::Local(_))
        ) {
            continue;
        }
        let slot = flat_stack_base
            .checked_add(index)
            .and_then(|slot| u32::try_from(slot).ok())
            .ok_or(CompileFailure::ResourceLimit)?;
        emit_opt_free_slot(
            builder,
            frame,
            sret,
            stack_base,
            exception_depth,
            signatures,
            stack_base,
            index,
            slot,
            pointer_type,
            layout,
        )?;
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn opt_release_owned_stack(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    start: usize,
    end: usize,
    flat_stack_base: usize,
    signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
) -> Result<(), CompileFailure> {
    for index in start..end {
        let slot = flat_stack_base
            .checked_add(index)
            .and_then(|slot| u32::try_from(slot).ok())
            .ok_or(CompileFailure::ResourceLimit)?;
        emit_opt_free_slot(
            builder,
            frame,
            sret,
            stack_base,
            end,
            signatures,
            stack_base,
            index,
            slot,
            pointer_type,
            layout,
        )?;
    }
    opt_set_stack_top(builder, frame, stack_base, start, pointer_type, layout);
    Ok(())
}

/// Turns borrowed argument/local aliases on the operand stack into real
/// interpreter owners before a deopt or exception exit, using the audited
/// MATERIALIZE_OWNER helper (these exits are rare).
#[allow(clippy::too_many_arguments)]
fn opt_own_stack_for_exit(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    depth: usize,
    flat_stack_base: usize,
    provenance: &[OptProvenance],
    signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
) -> Result<(), CompileFailure> {
    opt_own_stack(
        builder,
        frame,
        sret,
        stack_base,
        depth,
        flat_stack_base,
        provenance,
        signatures,
        pointer_type,
        layout,
        false,
    )
}

/// The same ownership transition on paths that continue into a helper or
/// runtime call and may run every iteration: outside stress GC each owner is
/// taken with an inline `js_dup` instead of a MATERIALIZE_OWNER helper call.
#[allow(clippy::too_many_arguments)]
fn opt_own_stack_for_helper(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    depth: usize,
    flat_stack_base: usize,
    provenance: &[OptProvenance],
    signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
) -> Result<(), CompileFailure> {
    opt_own_stack(
        builder,
        frame,
        sret,
        stack_base,
        depth,
        flat_stack_base,
        provenance,
        signatures,
        pointer_type,
        layout,
        true,
    )
}

#[allow(clippy::too_many_arguments)]
fn opt_own_stack(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    depth: usize,
    _flat_stack_base: usize,
    provenance: &[OptProvenance],
    signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
    inline_owner: bool,
) -> Result<(), CompileFailure> {
    use cranelift_codegen::ir::{condcodes::IntCC, types, InstBuilder, MemFlags};
    const MATERIALIZE_OWNER_HELPER: usize =
        rquickjs_core::qjs::JSJitHelperId_JS_JIT_HELPER_MATERIALIZE_OWNER as usize;
    const MATERIALIZED: i64 = rquickjs_core::qjs::JS_JIT_HELPER_MATERIALIZED as i64;
    const SOURCE_ARGUMENT: u32 =
        rquickjs_core::qjs::JSJitOwnerSourceKind_JS_JIT_OWNER_SOURCE_ARGUMENT;
    const SOURCE_LOCAL: u32 = rquickjs_core::qjs::JSJitOwnerSourceKind_JS_JIT_OWNER_SOURCE_LOCAL;
    let undefined = OptPair {
        payload: builder.ins().iconst(types::I64, 0),
        tag: builder
            .ins()
            .iconst(types::I64, i64::from(rquickjs_core::qjs::JS_TAG_UNDEFINED)),
    };
    opt_set_stack_top(builder, frame, stack_base, 0, pointer_type, layout);
    let cold = builder
        .current_block()
        .is_some_and(|block| builder.func.layout.is_cold(block));
    for index in 0..depth {
        if matches!(
            provenance.get(index),
            Some(OptProvenance::ImmediatePrimitive | OptProvenance::OwnedSlot)
        ) {
            opt_set_stack_top(builder, frame, stack_base, index + 1, pointer_type, layout);
            continue;
        }
        let (source_kind, source_index) = match provenance.get(index).copied() {
            Some(OptProvenance::Argument(slot)) => (SOURCE_ARGUMENT, slot),
            Some(OptProvenance::Local(slot)) => (SOURCE_LOCAL, slot),
            _ => return Err(CompileFailure::InvalidArtifact),
        };
        opt_store(builder, stack_base, index, undefined);
        let materialized = inline_owner.then(|| builder.create_block());
        if let Some(materialized) = materialized {
            // Outside stress GC the owner is an exact inline `js_dup` of the
            // published argument/local slot; stress GC keeps the helper and
            // its collection points out of line.
            let inline = builder.create_block();
            let helper_block = builder.create_block();
            let heap = builder.create_block();
            let copied = builder.create_block();
            builder.set_cold_block(helper_block);
            let no_stress = super::refcount::emit_no_stress(builder, frame, layout.flags);
            builder
                .ins()
                .brif(no_stress, inline, &[], helper_block, &[]);
            builder.switch_to_block(inline);
            let source_buffer = builder.ins().load(
                pointer_type,
                MemFlags::new(),
                frame,
                if source_kind == SOURCE_ARGUMENT {
                    layout.arg_buf
                } else {
                    layout.var_buf
                },
            );
            let source = opt_load(builder, source_buffer, source_index);
            let refcounted = super::refcount::emit_has_ref_count(builder, source.tag);
            builder.ins().brif(refcounted, heap, &[], copied, &[]);
            builder.switch_to_block(heap);
            super::refcount::emit_increment(builder, source.payload);
            builder.ins().jump(copied, &[]);
            builder.switch_to_block(copied);
            opt_store(builder, stack_base, index, source);
            opt_set_stack_top(builder, frame, stack_base, index + 1, pointer_type, layout);
            builder.ins().jump(materialized, &[]);
            builder.switch_to_block(helper_block);
        }
        let signature = *signatures
            .get(MATERIALIZE_OWNER_HELPER)
            .ok_or(CompileFailure::InvalidArtifact)?;
        let api = builder
            .ins()
            .load(pointer_type, MemFlags::new(), frame, layout.runtime_api);
        let helper = builder.ins().load(
            pointer_type,
            MemFlags::new(),
            api,
            layout.helper_offsets[MATERIALIZE_OWNER_HELPER],
        );
        let params = [
            frame,
            builder.ins().iconst(types::I32, 0),
            builder.ins().iconst(types::I32, index as i64),
            builder.ins().iconst(types::I32, i64::from(source_kind)),
            builder.ins().iconst(
                types::I32,
                i64::try_from(source_index).map_err(|_| CompileFailure::ResourceLimit)?,
            ),
        ];
        let call = super::emit_external_call(
            builder,
            signature,
            helper,
            &params,
            pointer_type,
            Some(frame),
            None,
        );
        let status = builder.inst_results(call)[0];
        let ok = builder.ins().icmp_imm(IntCC::Equal, status, MATERIALIZED);
        let continuation = builder.create_block();
        let exception = builder.create_block();
        // The helper call is out of line whenever the bridge itself is (a
        // cold deopt path) or the inline owner path makes it stress-only.
        if inline_owner || cold {
            builder.set_cold_block(continuation);
            builder.set_cold_block(exception);
        }
        builder.ins().brif(ok, continuation, &[], exception, &[]);
        builder.switch_to_block(exception);
        emit_opt_exit(
            builder,
            sret,
            rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_EXCEPTION,
            None,
            pointer_type,
            0,
        );
        builder.switch_to_block(continuation);
        if let Some(materialized) = materialized {
            builder.ins().jump(materialized, &[]);
            builder.switch_to_block(materialized);
        }
    }
    Ok(())
}
/// The CLIF block of an IR successor edge. Blocks are sealed as soon as all of
/// their IR predecessors have been lowered, so lowering may branch to an IR
/// block only along an edge the IR's successor lists already declare.
fn opt_ir_edge(
    blocks: &std::collections::BTreeMap<u32, cranelift_codegen::ir::Block>,
    block: &crate::ir::OptimizedBlock,
    target: Option<u32>,
) -> Result<cranelift_codegen::ir::Block, CompileFailure> {
    let target = target.ok_or(CompileFailure::InvalidArtifact)?;
    if !block.successors().contains(&target) {
        return Err(CompileFailure::InvalidArtifact);
    }
    blocks
        .get(&target)
        .copied()
        .ok_or(CompileFailure::InvalidArtifact)
}

fn next_block_pc(ir: &OptimizedIr, pc: u32) -> Result<u32, CompileFailure> {
    ir.blocks()
        .iter()
        .position(|block| block.start_pc() == pc)
        .and_then(|index| ir.blocks().get(index + 1))
        .map(|block| block.start_pc())
        .ok_or(CompileFailure::InvalidArtifact)
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_specialized_call(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    arg_buf: cranelift_codegen::ir::Value,
    var_buf: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    arguments: &[OptVars],
    locals: &[OptVars],
    stack: &[OptVars],
    stack_provenance: &mut [OptProvenance],
    depth: usize,
    argc: usize,
    has_this: bool,
    pc: u32,
    signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
    direct: Option<&DirectCallSite>,
    native: Option<&NativeCallSite>,
    guard: u32,
    scalar_result: bool,
    diagnostic_key: Option<crate::runtime::FunctionKey>,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;

    let pop = argc + 1 + usize::from(has_this);
    let Some(base) = depth.checked_sub(pop) else {
        #[cfg(feature = "test-support")]
        if let Some(key) = diagnostic_key {
            record_tier2_stage(
                key,
                Tier2CompileStage::GenericCallBaseUnderflow,
                Some(CompileFailure::InvalidArtifact),
            );
        }
        return Err(CompileFailure::InvalidArtifact);
    };
    // The CFG prepass admits a live prefix only when all incoming paths agree
    // on its ownership/value origin. Unknown is an ownership conflict, never a
    // request to guess or materialize a path-local state.
    if stack_provenance[..base].contains(&OptProvenance::Unknown) {
        #[cfg(feature = "test-support")]
        if let Some(key) = diagnostic_key {
            record_tier2_stage(
                key,
                Tier2CompileStage::GenericCallPrefixUnknown,
                Some(CompileFailure::InvalidArtifact),
            );
        }
        return Err(CompileFailure::InvalidArtifact);
    }
    let this_index = if has_this { base } else { depth };
    let function_index = if has_this { base + 1 } else { base };
    let argv_index = function_index + 1;
    let output_index = if has_this { depth } else { depth + 1 };
    if output_index >= stack.len() || (has_this && output_index + 1 >= stack.len()) {
        #[cfg(feature = "test-support")]
        if let Some(key) = diagnostic_key {
            record_tier2_stage(
                key,
                Tier2CompileStage::GenericCallOutputCapacity,
                Some(CompileFailure::ResourceLimit),
            );
        }
        return Err(CompileFailure::ResourceLimit);
    }
    if let Some(native) = native.filter(|native| {
        !has_this
            && native.arguments.len() == argc
            && native.callee_identity != 0
            && native.callee_bytecode_identity != 0
            && matches!(
                stack_provenance[function_index],
                OptProvenance::Argument(_) | OptProvenance::Local(_) | OptProvenance::OwnedSlot
            )
    }) {
        return emit_opt_native_call(
            builder,
            frame,
            sret,
            arg_buf,
            var_buf,
            stack_base,
            arguments,
            locals,
            stack,
            stack_provenance,
            depth,
            base,
            argc,
            pc,
            signatures,
            pointer_type,
            layout,
            native,
            guard,
        );
    }
    if let Some(direct) = direct.filter(|direct| {
        !has_this
            && direct.call.arity() == argc
            && direct.call.callee_identity() != 0
            && direct.call.callee_bytecode_identity() != 0
            && matches!(
                stack_provenance[function_index],
                OptProvenance::Argument(_) | OptProvenance::Local(_)
            )
            && direct
                .call
                .arguments()
                .iter()
                .enumerate()
                .all(|(index, representation)| {
                    *representation != crate::runtime::FeedbackRepresentation::HeapRef
                        || matches!(
                            stack_provenance[argv_index + index],
                            OptProvenance::Argument(_) | OptProvenance::Local(_)
                        )
                })
    }) {
        use crate::runtime::FeedbackRepresentation;
        use cranelift_codegen::ir::condcodes::IntCC;
        use cranelift_codegen::ir::{AbiParam, Signature, StackSlotData, StackSlotKind};
        let function = opt_use(builder, stack[function_index]);
        let signature = builder.create_block();
        let invoke = builder.create_block();
        let deopt = builder.create_block();
        builder.set_cold_block(deopt);
        super::emit_guarded_direct_callee_identity(
            builder,
            function.tag,
            function.payload,
            super::DirectCalleeIdentity {
                object: direct.call.callee_identity(),
                bytecode: direct.call.callee_bytecode_identity(),
            },
            pointer_type,
            signature,
            deopt,
        );
        builder.switch_to_block(signature);
        let mut matches = builder.ins().iconst(types::I8, 1);
        for (index, representation) in direct.call.arguments().iter().enumerate() {
            let value = opt_use(builder, stack[argv_index + index]);
            let tag = match representation {
                FeedbackRepresentation::Int32 => qjs::JS_TAG_INT,
                FeedbackRepresentation::Float64 => qjs::JS_TAG_FLOAT64,
                FeedbackRepresentation::Bool => qjs::JS_TAG_BOOL,
                FeedbackRepresentation::HeapRef => qjs::JS_TAG_OBJECT,
            };
            let typed = builder
                .ins()
                .icmp_imm(IntCC::Equal, value.tag, i64::from(tag));
            matches = builder.ins().band(matches, typed);
        }
        builder.ins().brif(matches, invoke, &[], deopt, &[]);
        builder.switch_to_block(deopt);
        for (index, vars) in arguments.iter().enumerate() {
            let value = opt_use(builder, *vars);
            opt_store(builder, arg_buf, index, value);
        }
        for (index, vars) in locals.iter().enumerate() {
            let value = opt_use(builder, *vars);
            opt_store(builder, var_buf, index, value);
        }
        for (index, vars) in stack.iter().take(depth).enumerate() {
            let value = opt_use(builder, *vars);
            opt_store(builder, stack_base, index, value);
        }
        opt_set_stack_top(builder, frame, stack_base, 0, pointer_type, layout);
        let start = builder
            .ins()
            .load(pointer_type, MemFlags::new(), frame, layout.bytecode_start);
        let resume = builder.ins().iadd_imm(start, i64::from(pc));
        builder
            .ins()
            .store(MemFlags::new(), resume, frame, layout.pc);
        opt_own_stack_for_exit(
            builder,
            frame,
            sret,
            stack_base,
            depth,
            arguments.len() + locals.len(),
            stack_provenance,
            signatures,
            pointer_type,
            layout,
        )?;
        emit_opt_exit(
            builder,
            sret,
            qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
            Some(resume),
            pointer_type,
            guard,
        );
        builder.switch_to_block(invoke);
        let scalar = match direct.call.result() {
            FeedbackRepresentation::Int32 | FeedbackRepresentation::Bool => types::I32,
            FeedbackRepresentation::Float64 => types::F64,
            FeedbackRepresentation::HeapRef => {
                unreachable!("direct calls cannot return owned heap values")
            }
        };
        let mut signature = Signature::new(builder.func.signature.call_conv);
        signature.params.push(AbiParam::new(pointer_type));
        for argument in direct.call.arguments() {
            signature.params.push(AbiParam::new(match argument {
                FeedbackRepresentation::Int32 | FeedbackRepresentation::Bool => types::I32,
                FeedbackRepresentation::Float64 => types::F64,
                FeedbackRepresentation::HeapRef => pointer_type,
            }));
        }
        signature.returns.push(AbiParam::new(types::I32));
        let signature = builder.import_signature(signature);
        let target = builder.ins().iconst(pointer_type, direct.entry as i64);
        let output = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            scalar.bytes(),
            0,
        ));
        let output_address = builder.ins().stack_addr(pointer_type, output, 0);
        let mut params = Vec::with_capacity(argc + 1);
        params.push(output_address);
        for (index, representation) in direct.call.arguments().iter().enumerate() {
            let value = opt_use(builder, stack[argv_index + index]);
            params.push(match representation {
                FeedbackRepresentation::Int32 | FeedbackRepresentation::Bool => {
                    builder.ins().ireduce(types::I32, value.payload)
                }
                FeedbackRepresentation::Float64 => {
                    builder
                        .ins()
                        .bitcast(types::F64, MemFlags::new(), value.payload)
                }
                FeedbackRepresentation::HeapRef => value.payload,
            });
        }
        let call = super::emit_external_call(
            builder,
            signature,
            target,
            &params,
            pointer_type,
            None,
            None,
        );
        let results = builder.inst_results(call);
        let status = results[0];
        let success = builder.ins().icmp_imm(IntCC::Equal, status, 0);
        let done = builder.create_block();
        builder.ins().brif(success, done, &[], deopt, &[]);
        builder.switch_to_block(done);
        let raw_result = builder
            .ins()
            .load(scalar, MemFlags::new(), output_address, 0);
        let result = match direct.call.result() {
            FeedbackRepresentation::Int32 => OptPair {
                payload: builder.ins().sextend(types::I64, raw_result),
                tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
            },
            FeedbackRepresentation::Float64 => OptPair {
                payload: builder
                    .ins()
                    .bitcast(types::I64, MemFlags::new(), raw_result),
                tag: builder
                    .ins()
                    .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64)),
            },
            FeedbackRepresentation::Bool => OptPair {
                payload: builder.ins().uextend(types::I64, raw_result),
                tag: builder
                    .ins()
                    .iconst(types::I64, i64::from(qjs::JS_TAG_BOOL)),
            },
            FeedbackRepresentation::HeapRef => {
                unreachable!("direct calls cannot return owned heap values")
            }
        };
        opt_define(builder, stack[base], result);
        stack_provenance[base] = OptProvenance::ImmediatePrimitive;
        return Ok(base + 1);
    }
    let undefined = OptPair {
        payload: builder.ins().iconst(types::I64, 0),
        tag: builder
            .ins()
            .iconst(types::I64, i64::from(qjs::JS_TAG_UNDEFINED)),
    };
    let mut call_ownership = vec![crate::ir::SsaValueOwnership::Borrowed; pop];
    if !has_this {
        opt_define(builder, stack[this_index], undefined);
    }
    opt_define(builder, stack[output_index], undefined);
    for (index, vars) in arguments.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, arg_buf, index, value);
    }
    for (index, vars) in locals.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, var_buf, index, value);
    }
    for (index, vars) in stack.iter().take(output_index + 1).enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, stack_base, index, value);
    }
    opt_set_stack_top(builder, frame, stack_base, 0, pointer_type, layout);
    let bytecode = builder
        .ins()
        .load(pointer_type, MemFlags::new(), frame, layout.bytecode_start);
    let current_pc = builder.ins().iadd_imm(bytecode, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), current_pc, frame, layout.pc);
    let flat_base = arguments.len() + locals.len();
    let slot = |index: usize| -> Result<u32, CompileFailure> {
        flat_base
            .checked_add(index)
            .and_then(|value| u32::try_from(value).ok())
            .ok_or(CompileFailure::ResourceLimit)
    };
    opt_own_stack_for_helper(
        builder,
        frame,
        sret,
        stack_base,
        depth,
        flat_base,
        stack_provenance,
        signatures,
        pointer_type,
        layout,
    )?;
    for owner in &mut call_ownership {
        *owner = owner
            .duplicate()
            .map_err(|_| CompileFailure::InvalidArtifact)?;
    }
    opt_set_stack_top(
        builder,
        frame,
        stack_base,
        output_index + 1,
        pointer_type,
        layout,
    );
    emit_opt_helper(
        builder,
        frame,
        sret,
        stack_base,
        depth,
        signatures,
        qjs::JSJitHelperId_JS_JIT_HELPER_CALL as usize,
        &[
            0,
            slot(output_index)?,
            slot(function_index)?,
            slot(this_index)?,
            if argc == 0 {
                u32::MAX
            } else {
                slot(argv_index)?
            },
            u32::try_from(argc).map_err(|_| CompileFailure::ResourceLimit)?,
        ],
        pointer_type,
        layout,
    )?;
    let result = opt_load(builder, stack_base, output_index);
    let displaced_index = if has_this {
        output_index + 1
    } else {
        this_index
    };
    let displaced = opt_use(builder, stack[base]);
    opt_define(builder, stack[displaced_index], displaced);
    opt_store(builder, stack_base, displaced_index, displaced);
    opt_define(builder, stack[base], result);
    opt_store(builder, stack_base, base, result);
    let free_depth = core::cmp::max(displaced_index, output_index) + 1;
    opt_set_stack_top(builder, frame, stack_base, free_depth, pointer_type, layout);
    for index in core::iter::once(displaced_index).chain((base + 1)..(base + pop)) {
        let input = if index == displaced_index {
            0
        } else {
            index - base
        };
        call_ownership[input]
            .consume()
            .map_err(|_| CompileFailure::InvalidArtifact)?;
        let consumed = opt_load(builder, stack_base, index);
        let primitive = builder.create_block();
        let release = builder.create_block();
        let cleaned = builder.create_block();
        super::call_cleanup::emit_cleanup_dispatch(
            builder,
            frame,
            consumed.tag,
            layout.flags,
            primitive,
            release,
        );
        builder.switch_to_block(primitive);
        opt_store(builder, stack_base, index, undefined);
        opt_define(builder, stack[index], undefined);
        builder.ins().jump(cleaned, &[]);
        builder.switch_to_block(release);
        // Shared heap owners drop one reference inline and then take the
        // primitive path's exact post-state; the last reference and every
        // stress-mode operand keep the FREE helper.
        let refcounted = super::refcount::emit_has_ref_count(builder, consumed.tag);
        let heap = builder.create_block();
        let helper = builder.create_block();
        builder.set_cold_block(helper);
        builder.ins().brif(refcounted, heap, &[], helper, &[]);
        builder.switch_to_block(heap);
        super::refcount::emit_release_refcounted(
            builder,
            frame,
            layout.flags,
            consumed.payload,
            primitive,
            helper,
        );
        builder.switch_to_block(helper);
        emit_opt_helper(
            builder,
            frame,
            sret,
            stack_base,
            depth + 2,
            signatures,
            qjs::JSJitHelperId_JS_JIT_HELPER_FREE as usize,
            &[0, slot(index)?],
            pointer_type,
            layout,
        )?;
        let value = opt_load(builder, stack_base, index);
        opt_define(builder, stack[index], value);
        builder.ins().jump(cleaned, &[]);
        builder.switch_to_block(cleaned);
    }
    for (index, &stack_slot) in stack
        .iter()
        .enumerate()
        .take(output_index + 1)
        .skip(base + 1)
    {
        opt_define(builder, stack_slot, undefined);
        opt_store(builder, stack_base, index, undefined);
    }
    // Values already owned by prefix stack slots may be observed across the
    // reentrant bridge, so reload their authoritative slots. Borrowed aliases
    // remain rooted by the same caller argument/local identity; release only
    // the temporary owners materialized for the helper's exception path.
    for (index, provenance) in stack_provenance.iter().take(base).enumerate() {
        if *provenance == OptProvenance::OwnedSlot {
            let value = opt_load(builder, stack_base, index);
            opt_define(builder, stack[index], value);
        }
    }
    opt_release_materialized_aliases(
        builder,
        frame,
        sret,
        stack_base,
        stack_provenance,
        0..base,
        free_depth,
        flat_base,
        signatures,
        pointer_type,
        layout,
    )?;
    // Feedback-specialized results are scalars; anything else stays owned
    // by the interpreter stack slot the CALL helper wrote it to.
    stack_provenance[base] = if scalar_result {
        OptProvenance::ImmediatePrimitive
    } else {
        OptProvenance::OwnedSlot
    };
    for provenance in &mut stack_provenance[(base + 1)..=output_index] {
        *provenance = OptProvenance::Unknown;
    }
    opt_set_stack_top(builder, frame, stack_base, base + 1, pointer_type, layout);
    Ok(base + 1)
}

/// Lowers a monomorphic CALL through the P3a native-call convention: guard
/// the callee identity and Int32 argument tags, prove the callee's global
/// self binding once (`JS_JitNativeCallBegin`), then run the whole pure
/// native chain. Every refusal or chain failure deoptimizes before the CALL,
/// so the interpreter re-executes it with no effect lost or duplicated.
#[allow(clippy::too_many_arguments)] // Mirrors the specialized-call lowering state.
fn emit_opt_native_call(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    arg_buf: cranelift_codegen::ir::Value,
    var_buf: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    arguments: &[OptVars],
    locals: &[OptVars],
    stack: &[OptVars],
    stack_provenance: &mut [OptProvenance],
    depth: usize,
    base: usize,
    argc: usize,
    pc: u32,
    signatures: &[cranelift_codegen::ir::SigRef],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
    native: &NativeCallSite,
    guard: u32,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{
        types, AbiParam, ExtFuncData, ExternalName, InstBuilder, MemFlags, Signature,
        StackSlotData, StackSlotKind, UserExternalName,
    };
    use rquickjs_core::qjs;

    let function_index = base;
    let argv_index = base + 1;
    let function = opt_use(builder, stack[function_index]);
    let arguments_ok = builder.create_block();
    let invoke = builder.create_block();
    let deopt = builder.create_block();
    super::emit_guarded_direct_callee_identity(
        builder,
        function.tag,
        function.payload,
        super::DirectCalleeIdentity {
            object: native.callee_identity,
            bytecode: native.callee_bytecode_identity,
        },
        pointer_type,
        arguments_ok,
        deopt,
    );
    builder.switch_to_block(arguments_ok);
    let mut matches = builder.ins().iconst(types::I8, 1);
    for (index, representation) in native.arguments.iter().enumerate() {
        let value = opt_use(builder, stack[argv_index + index]);
        let tag = match representation {
            crate::runtime::FeedbackRepresentation::Int32 => qjs::JS_TAG_INT,
            crate::runtime::FeedbackRepresentation::Float64 => qjs::JS_TAG_FLOAT64,
            _ => return Err(CompileFailure::InvalidArtifact),
        };
        let typed = builder
            .ins()
            .icmp_imm(IntCC::Equal, value.tag, i64::from(tag));
        matches = builder.ins().band(matches, typed);
    }
    builder.ins().brif(matches, invoke, &[], deopt, &[]);

    builder.switch_to_block(deopt);
    for (index, vars) in arguments.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, arg_buf, index, value);
    }
    for (index, vars) in locals.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, var_buf, index, value);
    }
    for (index, vars) in stack.iter().take(depth).enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, stack_base, index, value);
    }
    opt_set_stack_top(builder, frame, stack_base, 0, pointer_type, layout);
    let start = builder
        .ins()
        .load(pointer_type, MemFlags::new(), frame, layout.bytecode_start);
    let resume = builder.ins().iadd_imm(start, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), resume, frame, layout.pc);
    opt_own_stack_for_exit(
        builder,
        frame,
        sret,
        stack_base,
        depth,
        arguments.len() + locals.len(),
        stack_provenance,
        signatures,
        pointer_type,
        layout,
    )?;
    emit_opt_exit(
        builder,
        sret,
        qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
        Some(resume),
        pointer_type,
        guard,
    );

    builder.switch_to_block(invoke);
    let call_conv = builder.func.signature.call_conv;
    let context_slot =
        builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 32, 3));
    let context = builder.ins().stack_addr(pointer_type, context_slot, 0);
    let output_slot =
        builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, 8, 3));
    let output = builder.ins().stack_addr(pointer_type, output_slot, 0);
    let stack_pointer = builder.ins().get_stack_pointer(pointer_type);
    let ctx = builder
        .ins()
        .load(pointer_type, MemFlags::new(), frame, layout.ctx);
    let mut begin_signature = Signature::new(call_conv);
    for ty in [
        pointer_type,
        pointer_type,
        types::I32,
        pointer_type,
        pointer_type,
    ] {
        begin_signature.params.push(AbiParam::new(ty));
    }
    begin_signature.returns.push(AbiParam::new(types::I32));
    let begin_signature = builder.import_signature(begin_signature);
    let begin = builder.ins().iconst(
        pointer_type,
        super::native_call::qjsjit_native_call_begin as *const () as usize as i64,
    );
    let atom = builder
        .ins()
        .iconst(types::I32, i64::from(native.atom as i32));
    let call = super::emit_external_call(
        builder,
        begin_signature,
        begin,
        &[ctx, function.payload, atom, context, stack_pointer],
        pointer_type,
        Some(frame),
        None,
    );
    let refused = builder.inst_results(call)[0];
    let enter = builder.create_block();
    builder.ins().brif(refused, deopt, &[], enter, &[]);

    builder.switch_to_block(enter);
    let entry_signature = builder.import_signature(super::native_call::entry_signature(
        pointer_type,
        call_conv,
        &native.arguments,
    )?);
    let target = match native.entry {
        Some(entry) => builder.ins().iconst(pointer_type, entry as i64),
        None => {
            let name = builder
                .func
                .declare_imported_user_function(UserExternalName::new(
                    super::native_call::NATIVE_ENTRY_NAMESPACE,
                    0,
                ));
            let entry = builder.import_function(ExtFuncData {
                name: ExternalName::user(name),
                signature: entry_signature,
                colocated: false,
            });
            builder.ins().func_addr(pointer_type, entry)
        }
    };
    let mut params = Vec::with_capacity(argc + 3);
    params.push(context);
    params.push(output);
    params.push(stack_pointer);
    for (index, representation) in native.arguments.iter().enumerate() {
        let value = opt_use(builder, stack[argv_index + index]);
        params.push(match representation {
            crate::runtime::FeedbackRepresentation::Float64 => {
                builder
                    .ins()
                    .bitcast(types::F64, MemFlags::new(), value.payload)
            }
            _ => builder.ins().ireduce(types::I32, value.payload),
        });
    }
    let call = super::emit_external_call(
        builder,
        entry_signature,
        target,
        &params,
        pointer_type,
        None,
        None,
    );
    let status = builder.inst_results(call)[0];
    let mut end_signature = Signature::new(call_conv);
    end_signature.params.push(AbiParam::new(pointer_type));
    end_signature.params.push(AbiParam::new(pointer_type));
    let end_signature = builder.import_signature(end_signature);
    let end = builder.ins().iconst(
        pointer_type,
        super::native_call::qjsjit_native_call_end as *const () as usize as i64,
    );
    super::emit_external_call(
        builder,
        end_signature,
        end,
        &[context, stack_pointer],
        pointer_type,
        Some(frame),
        None,
    );
    let done = builder.create_block();
    builder.ins().brif(status, deopt, &[], done, &[]);

    builder.switch_to_block(done);
    let result = match native.result {
        crate::runtime::FeedbackRepresentation::Int32 => {
            let raw = builder
                .ins()
                .load(types::I32, MemFlags::trusted(), output, 0);
            OptPair {
                payload: builder.ins().sextend(types::I64, raw),
                tag: builder.ins().iconst(types::I64, i64::from(qjs::JS_TAG_INT)),
            }
        }
        crate::runtime::FeedbackRepresentation::Float64 => OptPair {
            payload: builder
                .ins()
                .load(types::I64, MemFlags::trusted(), output, 0),
            tag: builder
                .ins()
                .iconst(types::I64, i64::from(qjs::JS_TAG_FLOAT64)),
        },
        _ => return Err(CompileFailure::InvalidArtifact),
    };
    if stack_provenance[function_index] == OptProvenance::OwnedSlot {
        // The binding proven by Begin still holds the callee, so this only
        // drops the global lookup's extra reference.
        opt_store(builder, stack_base, function_index, function);
        opt_set_stack_top(
            builder,
            frame,
            stack_base,
            function_index + 1,
            pointer_type,
            layout,
        );
        let flat_slot = (arguments.len() + locals.len())
            .checked_add(function_index)
            .and_then(|slot| u32::try_from(slot).ok())
            .ok_or(CompileFailure::ResourceLimit)?;
        emit_opt_helper(
            builder,
            frame,
            sret,
            stack_base,
            function_index + 1,
            signatures,
            qjs::JSJitHelperId_JS_JIT_HELPER_FREE as usize,
            &[0, flat_slot],
            pointer_type,
            layout,
        )?;
    }
    opt_define(builder, stack[base], result);
    stack_provenance[base] = OptProvenance::ImmediatePrimitive;
    Ok(base + 1)
}

/// Locals that ever receive a value owned by its interpreter stack slot
/// (helper results: global lookups, kept-receiver property loads and
/// non-specialized call results). Stores into such a local must first
/// release whatever it currently owns, exactly like the interpreter's
/// `set_value`. Every block starts from lowering's CFG provenance map, so
/// unrelated layout predecessors cannot erase a possible incoming owner.
/// Unknown joins are conservatively owning; releasing a primitive is a no-op.
fn owned_local_targets(
    ir: &OptimizedIr,
    specialization: &NumericSpecialization,
    cfg_entry_provenance: &std::collections::BTreeMap<u32, Box<[OptProvenance]>>,
) -> Result<Vec<bool>, CompileFailure> {
    let Some(entry) = ir.guard_maps().first() else {
        return Err(CompileFailure::InvalidArtifact);
    };
    let mut owned_locals = vec![false; usize::from(entry.shape().locals())];
    let mut owned = vec![false; usize::from(ir.max_stack()) + crate::ir::MAX_HELPER_SCRATCH_SLOTS];
    for block in ir.blocks() {
        let mut depth = usize::from(block.stack_depth());
        owned.fill(false);
        let entry_provenance = cfg_entry_provenance
            .get(&block.start_pc())
            .ok_or(CompileFailure::InvalidArtifact)?;
        if entry_provenance.len() != depth || depth > owned.len() {
            return Err(CompileFailure::InvalidArtifact);
        }
        for (slot, provenance) in owned[..depth].iter_mut().zip(entry_provenance.iter()) {
            *slot = matches!(
                provenance,
                OptProvenance::OwnedSlot | OptProvenance::Unknown
            );
        }
        for node_id in block.nodes() {
            let node = ir
                .nodes()
                .get(*node_id as usize)
                .ok_or(CompileFailure::InvalidArtifact)?;
            let name = match node.kind() {
                crate::ir::OptimizedNodeKind::Bytecode { opcode } => opcode.as_ref(),
                _ => "",
            };
            let pops = usize::from(node.pops());
            let pushes = usize::from(node.pushes());
            let base = depth
                .checked_sub(pops)
                .ok_or(CompileFailure::InvalidArtifact)?;
            if base + pushes > owned.len() {
                return Err(CompileFailure::ResourceLimit);
            }
            let top_owned = pops > 0 && owned[depth - 1];
            let produces_owned = match name {
                "get_var" | "get_length" => true,
                "get_field2" => true,
                "get_field" => {
                    specialization.generic_properties.contains(&node.pc())
                        || specialization
                            .properties
                            .get(&node.pc())
                            .is_some_and(|observations| {
                                observations.iter().any(|observation| {
                                    observation.value() == crate::runtime::ObservedType::Object
                                })
                            })
                }
                n if n.starts_with("call") => {
                    ir.scalar_graph()
                        .call(node.id())
                        .is_some_and(|call| call.frame_inline.is_some())
                        || !specialization.calls.contains_key(&node.pc())
                }
                _ => false,
            };
            if top_owned {
                let target = if name.starts_with("put_loc") {
                    opt_index(name, node.bytes(), "put_loc")?
                } else if name.starts_with("set_loc") && name != "set_loc_uninitialized" {
                    opt_index(name, node.bytes(), "set_loc")?
                } else {
                    None
                };
                if let Some(slot) = target.and_then(|local| owned_locals.get_mut(local)) {
                    *slot = true;
                }
            }
            match name {
                n if opt_stack_permutation(n).is_some() => {
                    let (take, order) = opt_stack_permutation(n).unwrap();
                    let start = depth
                        .checked_sub(take)
                        .ok_or(CompileFailure::InvalidArtifact)?;
                    let inputs = owned[start..depth].to_vec();
                    for (destination, source) in order.iter().enumerate() {
                        owned[start + destination] = inputs[*source];
                    }
                    // Owning DUP publishes any borrowed prefix roots too.
                    if n == "dup" && top_owned {
                        owned[..start].fill(true);
                    }
                }
                "get_field2" => {
                    // The receiver stays; the loaded property is owned.
                    owned[base + 1] = true;
                }
                _ => {
                    for slot in &mut owned[base..base + pushes] {
                        *slot = produces_owned;
                    }
                }
            }
            depth = base + pushes;
        }
    }
    Ok(owned_locals)
}

fn opt_u32(bytes: &[u8]) -> Result<u32, CompileFailure> {
    bytes
        .get(1..5)
        .and_then(|raw| raw.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(CompileFailure::InvalidArtifact)
}

/// Flat frame slot index (arguments, locals, then stack) of a stack slot.
fn opt_flat_stack_slot(env: &OptEnv<'_>, index: usize) -> Result<u32, CompileFailure> {
    env.arguments
        .len()
        .checked_add(env.locals.len())
        .and_then(|base| base.checked_add(index))
        .and_then(|slot| u32::try_from(slot).ok())
        .ok_or(CompileFailure::ResourceLimit)
}

fn opt_flat_local_slot(env: &OptEnv<'_>, index: usize) -> Result<u32, CompileFailure> {
    env.arguments
        .len()
        .checked_add(index)
        .and_then(|slot| u32::try_from(slot).ok())
        .ok_or(CompileFailure::ResourceLimit)
}

/// Local index written or read by a `get_loc*`/`put_loc*`/`set_loc*`
/// family opcode name, or `None` for other opcodes.
fn opt_local_slot(name: &str, bytes: &[u8]) -> Option<usize> {
    for prefix in [
        "get_loc", "put_loc", "set_loc", "inc_loc", "dec_loc", "add_loc",
    ] {
        if name == "get_loc0_loc1" || name == "set_loc_uninitialized" {
            return None;
        }
        if name.starts_with(prefix) {
            if let Ok(Some(index)) = opt_index(name, bytes, prefix) {
                return Some(index);
            }
            return opt_u16(bytes).ok();
        }
    }
    None
}

/// Increments whose Int32 result provably cannot overflow, so the raw-i32
/// loop shape may add without an overflow exit.
///
/// The proof is the canonical counted loop: inside a natural loop whose
/// header block ends with `get_loc k; <Int32 operand>; lt; if_false -> exit`,
/// the sequence `get_loc k; inc|post_inc; put_loc k` is the only write to
/// local `k` anywhere in the loop. Then `k < X <= INT32_MAX` holds at the
/// increment on every iteration, so `k + 1` fits. Every value in the raw-i32
/// shape is Int32 by its entry and header guards, which is what makes the
/// header comparison a numeric bound.
fn provably_bounded_increments(ir: &OptimizedIr) -> std::collections::BTreeSet<u32> {
    use std::collections::{BTreeMap, BTreeSet};
    let mut bounded = BTreeSet::new();
    let blocks = ir.blocks();
    let index_of: BTreeMap<u32, usize> = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.start_pc(), index))
        .collect();
    let mut predecessors: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for block in blocks {
        for successor in block.successors() {
            predecessors
                .entry(*successor)
                .or_default()
                .push(block.start_pc());
        }
    }
    fn name_of(node: &crate::ir::OptimizedNode) -> Option<&str> {
        match node.kind() {
            crate::ir::OptimizedNodeKind::Bytecode { opcode } => Some(opcode.as_ref()),
            _ => None,
        }
    }
    for header in blocks.iter().filter(|block| block.is_loop_header()) {
        // Natural loop: the header plus everything that reaches a latch
        // without passing through the header.
        let latches: Vec<u32> = blocks
            .iter()
            .filter(|block| {
                block.start_pc() >= header.start_pc()
                    && block.successors().contains(&header.start_pc())
            })
            .map(|block| block.start_pc())
            .collect();
        let mut members = BTreeSet::from([header.start_pc()]);
        let mut pending = latches;
        while let Some(pc) = pending.pop() {
            if members.insert(pc) {
                if let Some(preds) = predecessors.get(&pc) {
                    pending.extend(preds.iter().copied());
                }
            }
        }
        // Header pattern: `get_loc k; operand; lt; if_false -> outside`.
        let header_nodes: Vec<&crate::ir::OptimizedNode> = header
            .nodes()
            .iter()
            .filter_map(|id| ir.nodes().get(*id as usize))
            .filter(|node| name_of(node).is_some() && !node.eliminated())
            .collect();
        let count = header_nodes.len();
        if count < 4 {
            continue;
        }
        let [load, operand, compare, branch] = [
            header_nodes[count - 4],
            header_nodes[count - 3],
            header_nodes[count - 2],
            header_nodes[count - 1],
        ];
        let Some(counter) = name_of(load).and_then(|name| {
            name.starts_with("get_loc")
                .then(|| opt_local_slot(name, load.bytes()))
                .flatten()
        }) else {
            continue;
        };
        let operand_ok = name_of(operand).is_some_and(|name| {
            name.starts_with("get_loc") || name.starts_with("get_arg") || name.starts_with("push_")
        });
        let exits_loop = name_of(branch).is_some_and(|name| name.starts_with("if_false"))
            && branch
                .branch_target()
                .is_some_and(|target| !members.contains(&target));
        if !operand_ok || name_of(compare) != Some("lt") || !exits_loop {
            continue;
        }
        // Every write to `counter` inside the loop must be the increment's
        // own `get_loc k; inc|post_inc; put_loc k` store.
        let mut increments = Vec::new();
        let mut foreign_write = false;
        for pc in &members {
            let Some(block) = index_of.get(pc).and_then(|index| blocks.get(*index)) else {
                continue;
            };
            let nodes: Vec<&crate::ir::OptimizedNode> = block
                .nodes()
                .iter()
                .filter_map(|id| ir.nodes().get(*id as usize))
                .filter(|node| name_of(node).is_some())
                .collect();
            for (position, node) in nodes.iter().enumerate() {
                let name = name_of(node).unwrap_or_default();
                let writes_counter = (name.starts_with("put_loc")
                    || name.starts_with("set_loc")
                    || name.starts_with("inc_loc")
                    || name.starts_with("dec_loc")
                    || name.starts_with("add_loc"))
                    && opt_local_slot(name, node.bytes()) == Some(counter);
                if !writes_counter {
                    continue;
                }
                let from_increment = name.starts_with("put_loc")
                    && position >= 2
                    && matches!(name_of(nodes[position - 1]), Some("inc" | "post_inc"))
                    && name_of(nodes[position - 2]).is_some_and(|name| {
                        name.starts_with("get_loc")
                            && opt_local_slot(name, nodes[position - 2].bytes()) == Some(counter)
                    });
                if from_increment {
                    increments.push(nodes[position - 1].pc());
                } else {
                    foreign_write = true;
                }
            }
        }
        if !foreign_write && increments.len() == 1 {
            bounded.extend(increments);
        }
    }
    bounded
}

/// How a value may be stored into an interpreter-owned argument or local
/// buffer. Tier 2 spills borrowed SSA aliases into those buffers at every
/// exit without transferring ownership (deoptimization maps carry identity
/// recipes only), so an alias that is a heap reference must never be copied
/// into a *different* slot: the interpreter would inherit an unowned pointer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AliasStore {
    /// Proven scalar, slot-owned (moved by the caller), or the slot's own value.
    Safe,
    /// Unproven representation: check the tag at run time and deoptimize
    /// before the store when the value is reference counted.
    GuardHeap,
    /// Proven or unknowable heap alias: fail closed.
    Reject,
}

fn opt_alias_store(
    specialization: &NumericSpecialization,
    argument_count: usize,
    source: OptProvenance,
    destination: OptProvenance,
) -> AliasStore {
    if source == destination {
        return AliasStore::Safe;
    }
    let representation = |slot: usize| {
        specialization
            .arguments
            .get(slot)
            .copied()
            .unwrap_or(specialization.entry)
    };
    let classify = |representation: EntryRepresentation| match representation {
        EntryRepresentation::Bool
        | EntryRepresentation::Numeric
        | EntryRepresentation::Int32
        | EntryRepresentation::Float64 => AliasStore::Safe,
        EntryRepresentation::Any => AliasStore::GuardHeap,
        EntryRepresentation::HeapRef => AliasStore::Reject,
    };
    match source {
        OptProvenance::ImmediatePrimitive | OptProvenance::OwnedSlot => AliasStore::Safe,
        OptProvenance::Argument(slot) => classify(representation(slot)),
        OptProvenance::Local(slot) => classify(representation(argument_count + slot)),
        OptProvenance::Unknown => AliasStore::Reject,
    }
}

/// Applies [`opt_alias_store`] for the value at stack index `source_index`
/// before it is stored into `destination`; `depth` is the operand-stack
/// depth before the storing instruction pops anything.
#[allow(clippy::too_many_arguments)]
fn emit_opt_alias_store_guard(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    specialization: &NumericSpecialization,
    provenance: &[OptProvenance],
    depth: usize,
    source_index: usize,
    destination: OptProvenance,
    pc: u32,
    guard: Option<u32>,
) -> Result<(), CompileFailure> {
    use cranelift_codegen::ir::{condcodes::IntCC, InstBuilder};
    if env.int32_loop {
        return Ok(());
    }
    match opt_alias_store(
        specialization,
        env.arguments.len(),
        provenance[source_index],
        destination,
    ) {
        AliasStore::Safe => Ok(()),
        AliasStore::Reject => Err(CompileFailure::UnsupportedOpcode),
        AliasStore::GuardHeap => {
            // QuickJS reference-counted tags are the negative ones.
            let pair = opt_use(builder, env.stack[source_index]);
            let not_refcounted =
                builder
                    .ins()
                    .icmp_imm(IntCC::SignedGreaterThanOrEqual, pair.tag, 0);
            let guard = guard.ok_or(CompileFailure::InvalidArtifact)?;
            emit_opt_guard_branch(builder, env, provenance, depth, pc, guard, not_refcounted)?;
            Ok(())
        }
    }
}

/// Compilation fails closed when an opcode would copy or silently discard a
/// value owned by its interpreter stack slot.
fn opt_reject_owned(
    provenance: &[OptProvenance],
    range: core::ops::Range<usize>,
) -> Result<(), CompileFailure> {
    if provenance[range].contains(&OptProvenance::OwnedSlot) {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    Ok(())
}

/// Releases the value an interpreter stack slot owns (the slot must hold
/// the current SSA value, which every owned slot does by construction).
fn emit_opt_free_stack_slot(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    index: usize,
) -> Result<(), CompileFailure> {
    let pair = opt_use(builder, env.stack[index]);
    opt_store(builder, env.stack_base, index, pair);
    opt_set_stack_top(
        builder,
        env.frame,
        env.stack_base,
        index + 1,
        env.pointer_type,
        env.layout,
    );
    emit_opt_free_slot(
        builder,
        env.frame,
        env.sret,
        env.stack_base,
        index + 1,
        env.helper_signatures,
        env.stack_base,
        index,
        opt_flat_stack_slot(env, index)?,
        env.pointer_type,
        env.layout,
    )
}

/// Releases the value a local owns before it is redefined.
fn emit_opt_free_local_slot(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    index: usize,
) -> Result<(), CompileFailure> {
    if env.int32_loop {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    let pair = opt_use(builder, env.locals[index]);
    opt_store(builder, env.var_buf, index, pair);
    emit_opt_free_slot(
        builder,
        env.frame,
        env.sret,
        env.stack_base,
        0,
        env.helper_signatures,
        env.var_buf,
        index,
        opt_flat_local_slot(env, index)?,
        env.pointer_type,
        env.layout,
    )
}

/// Invokes a helper that writes an owned value into the pushed stack slot
/// (GET_GLOBAL, GET_PROPERTY with the receiver kept, or DUP). Every live stack slot
/// is turned into a real interpreter owner first, exactly as the CALL bridge
/// does, so an exception unwinds the frame correctly and the helper sees a
/// consistent stack; `helper_arguments` follow the output slot.
fn emit_opt_owned_helper_push(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &mut [OptProvenance],
    depth: usize,
    pc: u32,
    helper_id: usize,
    helper_arguments: &[u32],
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;
    if env.int32_loop {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    if depth >= env.stack.len() {
        return Err(CompileFailure::ResourceLimit);
    }
    let undefined = OptPair {
        payload: builder.ins().iconst(types::I64, 0),
        tag: builder
            .ins()
            .iconst(types::I64, i64::from(qjs::JS_TAG_UNDEFINED)),
    };
    opt_define(builder, env.stack[depth], undefined);
    for (index, vars) in env.arguments.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.arg_buf, index, value);
    }
    for (index, vars) in env.locals.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.var_buf, index, value);
    }
    for (index, vars) in env.stack.iter().take(depth + 1).enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.stack_base, index, value);
    }
    let bytecode = builder.ins().load(
        env.pointer_type,
        MemFlags::new(),
        env.frame,
        env.layout.bytecode_start,
    );
    let current_pc = builder.ins().iadd_imm(bytecode, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), current_pc, env.frame, env.layout.pc);
    opt_own_stack_for_helper(
        builder,
        env.frame,
        env.sret,
        env.stack_base,
        depth,
        env.arguments.len() + env.locals.len(),
        provenance,
        env.helper_signatures,
        env.pointer_type,
        env.layout,
    )?;
    for slot in provenance.iter_mut().take(depth) {
        if matches!(slot, OptProvenance::Argument(_) | OptProvenance::Local(_)) {
            *slot = OptProvenance::OwnedSlot;
        }
    }
    opt_set_stack_top(
        builder,
        env.frame,
        env.stack_base,
        depth + 1,
        env.pointer_type,
        env.layout,
    );
    let mut arguments = Vec::with_capacity(helper_arguments.len() + 2);
    arguments.push(0);
    arguments.push(opt_flat_stack_slot(env, depth)?);
    arguments.extend_from_slice(helper_arguments);
    emit_opt_helper(
        builder,
        env.frame,
        env.sret,
        env.stack_base,
        depth,
        env.helper_signatures,
        helper_id,
        &arguments,
        env.pointer_type,
        env.layout,
    )?;
    let result = opt_load(builder, env.stack_base, depth);
    opt_define(builder, env.stack[depth], result);
    provenance[depth] = OptProvenance::OwnedSlot;
    Ok(depth + 1)
}

/// DUP of an interpreter-owned stack slot. Outside stress GC the second
/// reference is taken inline (`ref_count++` for heap values) and published in
/// its own owning slot. When no borrowed argument/local alias is live below
/// the operand, the helper path would not change any provenance, so stress-GC
/// frames branch to the exact DUP helper (with its collection points) and both
/// paths join with identical ownership. Otherwise the helper is used always.
fn emit_opt_owned_dup(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &mut [OptProvenance],
    source_index: usize,
    depth: usize,
    pc: u32,
    source_slot: u32,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::{types, InstBuilder};
    let has_borrowed_alias = provenance[..depth]
        .iter()
        .any(|slot| matches!(slot, OptProvenance::Argument(_) | OptProvenance::Local(_)));
    if env.int32_loop || has_borrowed_alias || depth >= env.stack.len() {
        return emit_opt_owned_helper_push(
            builder,
            env,
            provenance,
            depth,
            pc,
            rquickjs_core::qjs::JSJitHelperId_JS_JIT_HELPER_DUP as usize,
            &[source_slot],
        );
    }
    let inline = builder.create_block();
    let stress = builder.create_block();
    let joined = builder.create_block();
    builder.set_cold_block(stress);
    let no_stress = super::refcount::emit_no_stress(builder, env.frame, env.layout.flags);
    builder.ins().brif(no_stress, inline, &[], stress, &[]);

    builder.switch_to_block(stress);
    let mut helper_provenance = provenance.to_vec();
    let helper_depth = emit_opt_owned_helper_push(
        builder,
        env,
        &mut helper_provenance,
        depth,
        pc,
        rquickjs_core::qjs::JSJitHelperId_JS_JIT_HELPER_DUP as usize,
        &[source_slot],
    )?;
    builder.ins().jump(joined, &[]);

    builder.switch_to_block(inline);
    let value = opt_use(builder, env.stack[source_index]);
    if builder.func.dfg.value_type(value.payload) == types::I64 {
        let heap = builder.create_block();
        let copied = builder.create_block();
        let refcounted = super::refcount::emit_has_ref_count(builder, value.tag);
        builder.ins().brif(refcounted, heap, &[], copied, &[]);
        builder.switch_to_block(heap);
        super::refcount::emit_increment(builder, value.payload);
        builder.ins().jump(copied, &[]);
        builder.switch_to_block(copied);
    }
    opt_define(builder, env.stack[depth], value);
    opt_store(builder, env.stack_base, depth, value);
    opt_set_stack_top(
        builder,
        env.frame,
        env.stack_base,
        depth + 1,
        env.pointer_type,
        env.layout,
    );
    builder.ins().jump(joined, &[]);
    builder.switch_to_block(joined);

    provenance[depth] = OptProvenance::OwnedSlot;
    if helper_depth != depth + 1 || helper_provenance != provenance {
        return Err(CompileFailure::InvalidArtifact);
    }
    Ok(depth + 1)
}

/// Executes GetField through the audited owning helper when IC feedback says
/// the result is a heap object. The helper keeps the receiver, so release it
/// after the owned result has been published, then move that result into the
/// consumed receiver slot. Both helper exception edges expose only genuine
/// owners to the interpreter.
fn emit_opt_owned_property_replace(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &mut [OptProvenance],
    depth: usize,
    pc: u32,
    atom: u32,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::{types, InstBuilder};
    use rquickjs_core::qjs;
    let receiver = depth
        .checked_sub(1)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let receiver_slot = opt_flat_stack_slot(env, receiver)?;
    let result_depth = emit_opt_owned_helper_push(
        builder,
        env,
        provenance,
        depth,
        pc,
        qjs::JSJitHelperId_JS_JIT_HELPER_GET_PROPERTY as usize,
        &[receiver_slot, atom],
    )?;
    debug_assert_eq!(result_depth, depth + 1);
    emit_opt_free_slot(
        builder,
        env.frame,
        env.sret,
        env.stack_base,
        result_depth,
        env.helper_signatures,
        env.stack_base,
        receiver,
        receiver_slot,
        env.pointer_type,
        env.layout,
    )?;
    let result = opt_load(builder, env.stack_base, depth);
    let undefined = OptPair {
        payload: builder.ins().iconst(types::I64, 0),
        tag: builder
            .ins()
            .iconst(types::I64, i64::from(qjs::JS_TAG_UNDEFINED)),
    };
    opt_store(builder, env.stack_base, receiver, result);
    opt_store(builder, env.stack_base, depth, undefined);
    opt_define(builder, env.stack[receiver], result);
    opt_define(builder, env.stack[depth], undefined);
    provenance[receiver] = OptProvenance::OwnedSlot;
    provenance[depth] = OptProvenance::Unknown;
    opt_set_stack_top(
        builder,
        env.frame,
        env.stack_base,
        depth,
        env.pointer_type,
        env.layout,
    );
    Ok(depth)
}

/// Executes PutField through the audited generic SET_PROPERTY helper for a
/// megamorphic site. Every live stack value first becomes a real interpreter
/// owner (as for the CALL bridge), so a throwing or reentrant setter sees an
/// exact frame. The helper consumes the value slot; the receiver is released
/// afterwards, leaving the stack below the operands unchanged.
fn emit_opt_owned_property_store(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    env: &OptEnv<'_>,
    provenance: &mut [OptProvenance],
    depth: usize,
    pc: u32,
    atom: u32,
) -> Result<usize, CompileFailure> {
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    use rquickjs_core::qjs;
    if env.int32_loop {
        return Err(CompileFailure::UnsupportedOpcode);
    }
    let object_index = depth
        .checked_sub(2)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let value_index = depth - 1;
    if depth > env.stack.len() || depth > provenance.len() {
        return Err(CompileFailure::InvalidArtifact);
    }
    let object_slot = opt_flat_stack_slot(env, object_index)?;
    let value_slot = opt_flat_stack_slot(env, value_index)?;
    for (index, vars) in env.arguments.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.arg_buf, index, value);
    }
    for (index, vars) in env.locals.iter().enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.var_buf, index, value);
    }
    for (index, vars) in env.stack.iter().take(depth).enumerate() {
        let value = opt_use(builder, *vars);
        opt_store(builder, env.stack_base, index, value);
    }
    let bytecode = builder.ins().load(
        env.pointer_type,
        MemFlags::new(),
        env.frame,
        env.layout.bytecode_start,
    );
    let current_pc = builder.ins().iadd_imm(bytecode, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), current_pc, env.frame, env.layout.pc);
    opt_own_stack_for_exit(
        builder,
        env.frame,
        env.sret,
        env.stack_base,
        depth,
        env.arguments.len() + env.locals.len(),
        provenance,
        env.helper_signatures,
        env.pointer_type,
        env.layout,
    )?;
    for slot in provenance.iter_mut().take(depth) {
        if matches!(slot, OptProvenance::Argument(_) | OptProvenance::Local(_)) {
            *slot = OptProvenance::OwnedSlot;
        }
    }
    opt_set_stack_top(
        builder,
        env.frame,
        env.stack_base,
        depth,
        env.pointer_type,
        env.layout,
    );
    emit_opt_helper(
        builder,
        env.frame,
        env.sret,
        env.stack_base,
        depth,
        env.helper_signatures,
        qjs::JSJitHelperId_JS_JIT_HELPER_SET_PROPERTY as usize,
        &[0, object_slot, atom, value_slot],
        env.pointer_type,
        env.layout,
    )?;
    // SET_PROPERTY left UNDEFINED in the consumed value slot; only the
    // receiver owner remains above the surviving stack. Release it with the
    // inline refcount path (the FREE helper keeps the last reference and
    // stress-GC frames), exactly as the generic GET_PROPERTY replace does.
    emit_opt_free_slot(
        builder,
        env.frame,
        env.sret,
        env.stack_base,
        depth,
        env.helper_signatures,
        env.stack_base,
        object_index,
        object_slot,
        env.pointer_type,
        env.layout,
    )?;
    let undefined = OptPair {
        payload: builder.ins().iconst(types::I64, 0),
        tag: builder
            .ins()
            .iconst(types::I64, i64::from(qjs::JS_TAG_UNDEFINED)),
    };
    for index in [object_index, value_index] {
        opt_store(builder, env.stack_base, index, undefined);
        opt_define(builder, env.stack[index], undefined);
        provenance[index] = OptProvenance::Unknown;
    }
    opt_set_stack_top(
        builder,
        env.frame,
        env.stack_base,
        object_index,
        env.pointer_type,
        env.layout,
    );
    Ok(object_index)
}

#[allow(clippy::too_many_arguments)]
fn emit_opt_helper(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    exception_depth: usize,
    signatures: &[cranelift_codegen::ir::SigRef],
    helper_id: usize,
    arguments: &[u32],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
) -> Result<(), CompileFailure> {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    let signature = *signatures
        .get(helper_id)
        .ok_or(CompileFailure::InvalidArtifact)?;
    let api = builder
        .ins()
        .load(pointer_type, MemFlags::new(), frame, layout.runtime_api);
    let helper = builder.ins().load(
        pointer_type,
        MemFlags::new(),
        api,
        layout.helper_offsets[helper_id],
    );
    let mut params = Vec::with_capacity(arguments.len() + 1);
    params.push(frame);
    params.extend(
        arguments
            .iter()
            .map(|value| builder.ins().iconst(types::I32, i64::from(*value))),
    );
    let call = super::emit_external_call(
        builder,
        signature,
        helper,
        &params,
        pointer_type,
        Some(frame),
        None,
    );
    let status = builder.inst_results(call)[0];
    let succeeded = builder.ins().icmp_imm(IntCC::Equal, status, 0);
    let continuation = builder.create_block();
    let exception = builder.create_block();
    builder
        .ins()
        .brif(succeeded, continuation, &[], exception, &[]);
    builder.switch_to_block(exception);
    opt_set_stack_top(
        builder,
        frame,
        stack_base,
        exception_depth,
        pointer_type,
        layout,
    );
    emit_opt_exit(
        builder,
        sret,
        rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_EXCEPTION,
        None,
        pointer_type,
        0,
    );
    builder.switch_to_block(continuation);
    Ok(())
}

/// `JS_JitHelperFree` of the owner stored at `base[index]` (flat helper slot
/// `flat_slot`). Primitives and shared heap references are released inline
/// with the helper's exact post-state (an undefined slot); the last heap
/// reference and stress-GC frames keep the helper call.
#[allow(clippy::too_many_arguments)]
fn emit_opt_free_slot(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    exception_depth: usize,
    signatures: &[cranelift_codegen::ir::SigRef],
    base: cranelift_codegen::ir::Value,
    index: usize,
    flat_slot: u32,
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
) -> Result<(), CompileFailure> {
    use cranelift_codegen::ir::{types, InstBuilder};
    let value = opt_load(builder, base, index);
    let heap = builder.create_block();
    let primitive = builder.create_block();
    let clear = builder.create_block();
    let helper = builder.create_block();
    let done = builder.create_block();
    builder.set_cold_block(helper);
    let refcounted = super::refcount::emit_has_ref_count(builder, value.tag);
    builder.ins().brif(refcounted, heap, &[], primitive, &[]);
    builder.switch_to_block(primitive);
    let no_stress = super::refcount::emit_no_stress(builder, frame, layout.flags);
    builder.ins().brif(no_stress, clear, &[], helper, &[]);
    builder.switch_to_block(heap);
    super::refcount::emit_release_refcounted(
        builder,
        frame,
        layout.flags,
        value.payload,
        clear,
        helper,
    );
    builder.switch_to_block(clear);
    let undefined = OptPair {
        payload: builder.ins().iconst(types::I64, 0),
        tag: builder
            .ins()
            .iconst(types::I64, i64::from(rquickjs_core::qjs::JS_TAG_UNDEFINED)),
    };
    opt_store(builder, base, index, undefined);
    builder.ins().jump(done, &[]);
    builder.switch_to_block(helper);
    emit_opt_helper(
        builder,
        frame,
        sret,
        stack_base,
        exception_depth,
        signatures,
        rquickjs_core::qjs::JSJitHelperId_JS_JIT_HELPER_FREE as usize,
        &[0, flat_slot],
        pointer_type,
        layout,
    )?;
    builder.ins().jump(done, &[]);
    builder.switch_to_block(done);
    Ok(())
}

fn opt_set_stack_top(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    stack_base: cranelift_codegen::ir::Value,
    depth: usize,
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
) {
    use cranelift_codegen::ir::{InstBuilder, MemFlags};
    let top = builder.ins().iadd_imm(
        stack_base,
        i64::try_from(depth * 16).expect("verified frame"),
    );
    builder
        .ins()
        .store(MemFlags::new(), top, frame, layout.stack_top);
    let _ = pointer_type;
}
fn emit_opt_exit(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    sret: cranelift_codegen::ir::Value,
    kind: u32,
    resume: Option<cranelift_codegen::ir::Value>,
    pointer_type: cranelift_codegen::ir::Type,
    guard: u32,
) {
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    let zero = builder.ins().iconst(pointer_type, 0);
    let map_identity = if kind == rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT {
        guard.saturating_add(1)
    } else {
        0
    };
    let kind = builder.ins().iconst(types::I32, i64::from(kind));
    let map = builder.ins().iconst(types::I32, i64::from(map_identity));
    builder.ins().store(MemFlags::new(), kind, sret, 0);
    builder.ins().store(MemFlags::new(), map, sret, 4);
    builder
        .ins()
        .store(MemFlags::new(), resume.unwrap_or(zero), sret, 8);
    builder.ins().store(MemFlags::new(), zero, sret, 16);
    builder.ins().return_(&[]);
}
#[allow(clippy::too_many_arguments)]
fn emit_opt_numeric_guard(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    arguments: &[OptVars],
    locals: &[OptVars],
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
    guard: u32,
    pc: u32,
    pass: cranelift_codegen::ir::Block,
    side_path: Option<crate::runtime::SidePathProfile>,
    representation: EntryRepresentation,
    argument_representations: &[EntryRepresentation],
    owned_locals: &[bool],
) {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::{types, InstBuilder, MemFlags};
    let mut numeric = builder.ins().iconst(types::I8, 1);
    let mut alternate_numeric = builder.ins().iconst(types::I8, 1);
    let mut alternate_seen = builder.ins().iconst(types::I8, 0);
    for (index, vars) in arguments.iter().chain(locals).enumerate() {
        let pair = opt_use(builder, *vars);
        let int = builder.ins().icmp_imm(
            IntCC::Equal,
            pair.tag,
            i64::from(rquickjs_core::qjs::JS_TAG_INT),
        );
        let float = builder.ins().icmp_imm(
            IntCC::Equal,
            pair.tag,
            i64::from(rquickjs_core::qjs::JS_TAG_FLOAT64),
        );
        let required = if index >= arguments.len()
            && owned_locals
                .get(index - arguments.len())
                .copied()
                .unwrap_or(false)
        {
            // Helper-derived locals are full owned JSValues, not numeric
            // loop-state. Their consumers retain their own representation
            // checks; rejecting them at every loop header would make a
            // safely rooted object local deopt even on a zero-trip loop.
            EntryRepresentation::Any
        } else {
            argument_representations
                .get(index)
                .copied()
                .unwrap_or(representation)
        };
        let valid = match required {
            EntryRepresentation::Any => builder.ins().iconst(types::I8, 1),
            EntryRepresentation::Numeric if index >= arguments.len() => {
                // A `let` declared inside the loop body is still
                // uninitialized or undefined at the header. Every numeric
                // consumer guards its operand tags itself, so such locals
                // need no proof here.
                let numeric = builder.ins().bor(int, float);
                let undefined = builder.ins().icmp_imm(
                    IntCC::Equal,
                    pair.tag,
                    i64::from(rquickjs_core::qjs::JS_TAG_UNDEFINED),
                );
                let uninitialized = builder.ins().icmp_imm(
                    IntCC::Equal,
                    pair.tag,
                    i64::from(rquickjs_core::qjs::JS_TAG_UNINITIALIZED),
                );
                let unset = builder.ins().bor(undefined, uninitialized);
                builder.ins().bor(numeric, unset)
            }
            EntryRepresentation::Numeric => builder.ins().bor(int, float),
            EntryRepresentation::Bool => builder.ins().icmp_imm(
                IntCC::Equal,
                pair.tag,
                i64::from(rquickjs_core::qjs::JS_TAG_BOOL),
            ),
            EntryRepresentation::Int32 => int,
            EntryRepresentation::Float64 => float,
            EntryRepresentation::HeapRef => builder.ins().icmp_imm(
                IntCC::Equal,
                pair.tag,
                i64::from(rquickjs_core::qjs::JS_TAG_OBJECT),
            ),
        };
        numeric = builder.ins().band(numeric, valid);
        let either_numeric = builder.ins().bor(int, float);
        alternate_numeric = builder.ins().band(alternate_numeric, either_numeric);
        alternate_seen = builder.ins().bor(alternate_seen, float);
    }
    let deopt = builder.create_block();
    builder.set_cold_block(deopt);
    if side_path.is_some_and(|profile| profile.observed() == crate::runtime::ObservedType::Float64)
    {
        let side_check = builder.create_block();
        let side_block = builder.create_block();
        builder.ins().brif(numeric, pass, &[], side_check, &[]);
        builder.switch_to_block(side_check);
        let matches_profile = builder.ins().band(alternate_numeric, alternate_seen);
        builder
            .ins()
            .brif(matches_profile, side_block, &[], deopt, &[]);
        builder.switch_to_block(side_block);
        let flags = builder
            .ins()
            .load(types::I32, MemFlags::new(), frame, layout.flags);
        let flags = builder.ins().bor_imm(
            flags,
            i64::from(rquickjs_core::qjs::JS_JIT_FRAME_SIDE_PATH_HIT),
        );
        builder
            .ins()
            .store(MemFlags::new(), flags, frame, layout.flags);
        builder.ins().jump(pass, &[]);
    } else {
        builder.ins().brif(numeric, pass, &[], deopt, &[]);
    }
    builder.switch_to_block(deopt);
    // Entry and loop-header guards resume with an empty operand stack (the
    // lowering rejects a loop poll at a non-empty depth). An earlier helper
    // call may have published a deeper stack top (for example the owned
    // `get_field` that initialized a receiver local before the loop); the
    // resume shape is validated against it, so republish the empty top.
    // Reload the base here: keeping the entry SSA value live into this cold
    // exit would extend it across the whole loop body.
    let stack_base = builder
        .ins()
        .load(pointer_type, MemFlags::new(), frame, layout.stack_base);
    opt_set_stack_top(builder, frame, stack_base, 0, pointer_type, layout);
    let start = builder
        .ins()
        .load(pointer_type, MemFlags::new(), frame, layout.bytecode_start);
    let resume = builder.ins().iadd_imm(start, i64::from(pc));
    builder
        .ins()
        .store(MemFlags::new(), resume, frame, layout.pc);
    emit_opt_exit(
        builder,
        sret,
        rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_DEOPT,
        Some(resume),
        pointer_type,
        guard,
    );
}

fn emit_opt_poll(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    signature: cranelift_codegen::ir::SigRef,
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
    pc: u32,
) {
    use cranelift_codegen::ir::{condcodes::IntCC, InstBuilder, MemFlags};
    let flags = MemFlags::new();
    let api = builder
        .ins()
        .load(pointer_type, flags, frame, layout.runtime_api);
    let poll = builder.ins().load(
        pointer_type,
        flags,
        api,
        layout.helper_offsets[rquickjs_core::qjs::JSJitHelperId_JS_JIT_HELPER_POLL as usize],
    );
    let call = super::emit_external_call(
        builder,
        signature,
        poll,
        &[frame],
        pointer_type,
        Some(frame),
        None,
    );
    let interrupted = builder.inst_results(call)[0];
    let interrupted = builder.ins().icmp_imm(IntCC::NotEqual, interrupted, 0);
    let interrupt = builder.create_block();
    let continuation = builder.create_block();
    builder
        .ins()
        .brif(interrupted, interrupt, &[], continuation, &[]);
    builder.switch_to_block(interrupt);
    let start = builder
        .ins()
        .load(pointer_type, flags, frame, layout.bytecode_start);
    let resume = builder.ins().iadd_imm(start, i64::from(pc));
    emit_opt_exit(
        builder,
        sret,
        rquickjs_core::qjs::JSJitExitKind_JS_JIT_EXIT_INTERRUPT,
        Some(resume),
        pointer_type,
        0,
    );
    builder.switch_to_block(continuation);
}

/// Calls the runtime poll once every 64 executions of this point. The frame
/// is already synchronized here (callers use it only where the regular poll
/// would run), so only the call frequency changes. The poll block keeps its
/// layout position: moving calls changes which call return publishes the
/// artifact's helper stack map.
#[allow(clippy::too_many_arguments)] // Poll ABI parameters mirror the generated helper signature.
fn emit_opt_countdown_poll(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    signature: cranelift_codegen::ir::SigRef,
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
    pc: u32,
    budget: cranelift_frontend::Variable,
) {
    use cranelift_codegen::ir::{condcodes::IntCC, types, InstBuilder};
    let remaining = builder.use_var(budget);
    let remaining = builder.ins().iadd_imm(remaining, -1);
    builder.def_var(budget, remaining);
    let due = builder.ins().icmp_imm(IntCC::Equal, remaining, 0);
    let poll = builder.create_block();
    let continuation = builder.create_block();
    builder.ins().brif(due, poll, &[], continuation, &[]);
    builder.switch_to_block(poll);
    let reset = builder.ins().iconst(types::I32, 64);
    builder.def_var(budget, reset);
    emit_opt_poll(builder, frame, sret, signature, pointer_type, layout, pc);
    builder.ins().jump(continuation, &[]);
    builder.switch_to_block(continuation);
}

#[allow(clippy::too_many_arguments)] // Poll ABI parameters mirror the generated helper signature.
fn emit_opt_amortized_poll(
    builder: &mut cranelift_frontend::FunctionBuilder<'_>,
    frame: cranelift_codegen::ir::Value,
    sret: cranelift_codegen::ir::Value,
    signature: cranelift_codegen::ir::SigRef,
    pointer_type: cranelift_codegen::ir::Type,
    layout: super::helpers::FrameLayout,
    pc: u32,
    budget: cranelift_frontend::Variable,
    cold_poll: bool,
    before_poll: impl FnOnce(&mut cranelift_frontend::FunctionBuilder<'_>),
    after_poll: impl FnOnce(&mut cranelift_frontend::FunctionBuilder<'_>) -> Result<(), CompileFailure>,
) -> Result<(), CompileFailure> {
    use cranelift_codegen::ir::{condcodes::IntCC, types, InstBuilder};
    let remaining = builder.use_var(budget);
    let remaining = builder.ins().iadd_imm(remaining, -1);
    builder.def_var(budget, remaining);
    let due = builder.ins().icmp_imm(IntCC::Equal, remaining, 0);
    let poll = builder.create_block();
    // Preserve the established raw-i32 loop layout. The new mixed scalar
    // path outlines both polling and its frame revalidation together.
    if cold_poll {
        builder.set_cold_block(poll);
    }
    let continuation = builder.create_block();
    builder.ins().brif(due, poll, &[], continuation, &[]);
    builder.seal_block(poll);
    builder.switch_to_block(poll);
    before_poll(builder);
    emit_opt_poll(builder, frame, sret, signature, pointer_type, layout, pc);
    after_poll(builder)?;
    let reset = builder.ins().iconst(types::I32, 64);
    builder.def_var(budget, reset);
    builder.ins().jump(continuation, &[]);
    // Both edges into the fresh join are now declared. Sealing it lets
    // values the poll path did not redefine (such as literal Int32 tags of
    // proven induction variables) stay visible to the loop body.
    builder.seal_block(continuation);
    builder.switch_to_block(continuation);
    Ok(())
}

impl Tier2Compiler {
    pub fn host(feedback_epoch: u64) -> Self {
        use cranelift_codegen::settings::{self, Configurable};
        let mut settings = settings::builder();
        settings
            .set("opt_level", "speed")
            .expect("Cranelift opt_level setting");
        // See BaselineCompiler::host: the IR verifier is a development aid
        // whose superlinear passes dominate large-function compile time.
        settings
            .set("enable_verifier", "false")
            .expect("Cranelift enable_verifier setting");
        let isa = cranelift_native::builder()
            .expect("host architecture is supported by Cranelift")
            .finish(settings::Flags::new(settings))
            .expect("host ISA settings are valid");
        Self {
            isa,
            feedback_epoch,
        }
    }

    #[cfg(feature = "test-support")]
    pub fn lower_for_test(
        &self,
        function: &VerifiedFunction,
        epoch: u64,
    ) -> Result<String, CompileFailure> {
        let ir = OptimizedIr::translate(function, epoch)?;
        lower_optimized_machine(
            &self.isa,
            &ir,
            None,
            None,
            &NumericSpecialization::default(),
        )
        .map(|code| code.clif().to_owned())
    }

    /// Compiles and publishes the feedback-free Tier 2 machine code for a
    /// verified function so tests can execute it on a synthetic frame and
    /// observe exact results and exits.
    #[cfg(all(feature = "test-support", not(target_family = "wasm")))]
    pub fn publish_for_test(
        &self,
        function: &VerifiedFunction,
        epoch: u64,
    ) -> Result<super::baseline::PublishedBaselineCode, CompileFailure> {
        let ir = OptimizedIr::translate(function, epoch)?;
        lower_optimized_machine(
            &self.isa,
            &ir,
            None,
            None,
            &NumericSpecialization::default(),
        )?
        .publish()
        .map_err(|_| CompileFailure::InvalidArtifact)
    }

    #[cfg(feature = "test-support")]
    pub fn lower_with_feedback_for_test(
        &self,
        function: &VerifiedFunction,
        key: crate::runtime::FunctionKey,
        feedback: &crate::runtime::FeedbackSnapshot,
    ) -> Result<String, CompileFailure> {
        let specialization = NumericSpecialization::from_feedback(function, key, feedback);
        let ir = specialization.translate(function, feedback.epoch())?;
        lower_optimized_machine(&self.isa, &ir, None, None, &specialization)
            .map(|code| code.clif().to_owned())
    }

    #[cfg(feature = "test-support")]
    pub fn machine_with_feedback_for_test(
        &self,
        function: &VerifiedFunction,
        key: crate::runtime::FunctionKey,
        feedback: &crate::runtime::FeedbackSnapshot,
    ) -> Result<String, CompileFailure> {
        let specialization = NumericSpecialization::from_feedback(function, key, feedback);
        let ir = specialization.translate(function, feedback.epoch())?;
        lower_optimized_machine(&self.isa, &ir, None, None, &specialization)
            .map(|code| code.machine_disassembly().to_owned())
    }

    #[cfg(feature = "test-support")]
    pub fn lower_direct_call_with_feedback_for_test(
        &self,
        function: &VerifiedFunction,
        key: crate::runtime::FunctionKey,
        feedback: &crate::runtime::FeedbackSnapshot,
    ) -> Result<String, CompileFailure> {
        let signature = feedback
            .bounded_specialization(key)
            .ok_or(CompileFailure::InvalidArtifact)?;
        lower_direct_call_machine(&self.isa, function, &signature, Some(feedback), None)
            .map(|code| code.clif().to_owned())
    }

    #[cfg(feature = "test-support")]
    pub fn lower_with_inline_callee_for_test(
        &self,
        function: &VerifiedFunction,
        key: crate::runtime::FunctionKey,
        feedback: &crate::runtime::FeedbackSnapshot,
        call_pc: u32,
        callee: crate::ir::InlineCallee,
    ) -> Result<(OptimizedIr, String), CompileFailure> {
        let mut specialization = NumericSpecialization::from_feedback(function, key, feedback);
        specialization.inline_callees.insert(call_pc, callee);
        let ir = specialization.translate(function, feedback.epoch())?;
        let code = lower_optimized_machine(&self.isa, &ir, None, None, &specialization)?;
        Ok((ir, code.clif().to_owned()))
    }

    #[cfg(feature = "test-support")]
    pub fn lower_with_frame_inline_callee_for_test(
        &self,
        function: &VerifiedFunction,
        key: crate::runtime::FunctionKey,
        feedback: &crate::runtime::FeedbackSnapshot,
        call_pc: u32,
        callee: crate::ir::FrameInlineCallee,
    ) -> Result<(OptimizedIr, String), CompileFailure> {
        let mut specialization = NumericSpecialization::from_feedback(function, key, feedback);
        specialization.frame_inline_callees.insert(call_pc, callee);
        let ir = specialization.translate(function, feedback.epoch())?;
        let code = lower_optimized_machine(&self.isa, &ir, None, None, &specialization)?;
        Ok((ir, code.clif().to_owned()))
    }

    #[cfg(feature = "test-support")]
    pub fn lower_with_direct_target_for_test(
        &self,
        function: &VerifiedFunction,
        key: crate::runtime::FunctionKey,
        feedback: &crate::runtime::FeedbackSnapshot,
        call_pc: u32,
        entry: usize,
    ) -> Result<String, CompileFailure> {
        let mut specialization = NumericSpecialization::from_feedback(function, key, feedback);
        let ir = specialization.translate(function, feedback.epoch())?;
        let call = feedback
            .call_specialization_at(key, call_pc)
            .ok_or(CompileFailure::InvalidArtifact)?;
        specialization
            .direct_calls
            .insert(call_pc, DirectCallSite { call, entry });
        lower_optimized_machine(&self.isa, &ir, None, None, &specialization)
            .map(|code| code.clif().to_owned())
    }

    #[cfg(all(feature = "test-support", not(target_family = "wasm")))]
    pub fn execute_direct_i32_for_test(
        &self,
        function: &VerifiedFunction,
        key: crate::runtime::FunctionKey,
        feedback: &crate::runtime::FeedbackSnapshot,
        arguments: &[i32],
    ) -> Result<(i32, i32), CompileFailure> {
        let signature = feedback
            .bounded_specialization(key)
            .ok_or(CompileFailure::InvalidArtifact)?;
        if signature
            .arguments()
            .iter()
            .any(|representation| *representation != crate::runtime::FeedbackRepresentation::Int32)
            || arguments.len() != signature.arity()
            || arguments.len() > 2
        {
            return Err(CompileFailure::InvalidArtifact);
        }
        let published =
            lower_direct_call_machine(&self.isa, function, &signature, Some(feedback), None)?
                .publish()
                .map_err(|_| CompileFailure::InvalidArtifact)?;
        // The match above proves the exact arity and representation of the
        // scalar-only ABI before converting the executable entry.
        let mut output = 0i32;
        let status = unsafe {
            match arguments {
                [a] => core::mem::transmute::<*const u8, extern "C" fn(*mut i32, i32) -> i32>(
                    published.as_ptr(),
                )(&mut output, *a),
                [a, b] => {
                    core::mem::transmute::<*const u8, extern "C" fn(*mut i32, i32, i32) -> i32>(
                        published.as_ptr(),
                    )(&mut output, *a, *b)
                }
                _ => return Err(CompileFailure::InvalidArtifact),
            }
        };
        Ok((status, output))
    }

    #[cfg(feature = "test-support")]
    pub fn lower_side_path_for_test(
        &self,
        function: &VerifiedFunction,
        epoch: u64,
        profile: crate::runtime::SidePathProfile,
    ) -> Result<String, CompileFailure> {
        let ir = OptimizedIr::translate(function, epoch)?;
        lower_optimized_machine(
            &self.isa,
            &ir,
            None,
            Some(profile),
            &NumericSpecialization::default(),
        )
        .map(|code| code.clif().to_owned())
    }

    pub fn plan(
        function: &VerifiedFunction,
        feedback_epoch: u64,
    ) -> Result<OptimizedArtifactMetadata, CompileFailure> {
        let ir = OptimizedIr::translate(function, feedback_epoch)?;
        Ok(Self::metadata_from_ir(&ir))
    }

    fn metadata_from_ir(ir: &OptimizedIr) -> OptimizedArtifactMetadata {
        let sites = ir
            .guard_maps()
            .iter()
            .map(|site| (site.shape(), site.map().clone()))
            .collect();
        let metrics = ir.metrics();
        OptimizedArtifactMetadata::new(
            ir.feedback_epoch(),
            sites,
            metrics.boxes_elided,
            metrics.cse_eliminated,
            metrics.dead_nodes_eliminated,
        )
        .with_inlined_calls(
            ir.scalar_graph()
                .inlined_calls()
                .saturating_add(ir.scalar_graph().frame_inlined_calls()),
        )
    }
}

pub struct TieredCompiler {
    baseline: BaselineCompiler,
    optimizing: Tier2Compiler,
}

impl TieredCompiler {
    pub fn host() -> Self {
        Self {
            baseline: BaselineCompiler::host(),
            optimizing: Tier2Compiler::host(0),
        }
    }
    pub fn target_identity(&self) -> crate::compiler::baseline::TargetIdentity {
        self.baseline.target_identity()
    }
}

impl Compiler for TieredCompiler {
    fn compile(
        &self,
        request: CompileRequest,
    ) -> Result<crate::code_cache::CompiledArtifact, CompileFailure> {
        match request.tier() {
            Tier::Baseline => Compiler::compile(&self.baseline, request),
            Tier::Optimizing => self.optimizing.compile(request),
        }
    }
    fn compile_controlled(
        &self,
        request: CompileRequest,
        control: &CompileControl,
    ) -> Result<crate::code_cache::CompiledArtifact, CompileFailure> {
        match request.tier() {
            Tier::Baseline => self.baseline.compile_controlled(request, control),
            Tier::Optimizing => self.optimizing.compile_controlled(request, control),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tier2CompileStage {
    Admission,
    FromFeedback,
    RetainInlineCallees,
    InitialTranslate,
    FrameTranslateRetry,
    IrBudget,
    SidePathValidation,
    DirectDependencies,
    InitialLower,
    InlineLowerRetryTranslate,
    InlineLowerRetryBudget,
    InlineLowerRetry,
    DirectLower,
    ArtifactBind,
    DirectMetadata,
    GenericCallBaseUnderflow,
    GenericCallPrefixUnknown,
    GenericCallOutputCapacity,
    GenericCallSemanticMissing,
    GenericCallFrameStateMissing,
    GenericCallShapeMismatch,
    GenericCallArityMismatch,
    GenericCallGuardMissing,
    GenericCallEmit,
    PropertyFeedbackMissing,
    Complete,
}

#[cfg(feature = "test-support")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tier2CompileDisposition {
    pub stage: Tier2CompileStage,
    pub failure: Option<CompileFailure>,
}

#[cfg(feature = "test-support")]
std::thread_local! {
    static TIER2_DIAGNOSTIC_RUNTIME: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Diagnostics follow the request across worker threads. Restore the previous
/// scope on every exit, including unwinding and nested compilation.
#[cfg(feature = "test-support")]
struct Tier2DiagnosticScope(u64);

#[cfg(feature = "test-support")]
impl Tier2DiagnosticScope {
    fn enter(runtime: u64) -> Self {
        Self(TIER2_DIAGNOSTIC_RUNTIME.replace(runtime))
    }
}

#[cfg(feature = "test-support")]
impl Drop for Tier2DiagnosticScope {
    fn drop(&mut self) {
        TIER2_DIAGNOSTIC_RUNTIME.set(self.0);
    }
}

#[cfg(all(test, feature = "test-support"))]
mod diagnostic_scope_tests {
    use super::{Tier2DiagnosticScope, TIER2_DIAGNOSTIC_RUNTIME};

    #[test]
    fn nested_diagnostic_scopes_restore_each_previous_runtime() {
        let initial = TIER2_DIAGNOSTIC_RUNTIME.get();
        {
            let _outer = Tier2DiagnosticScope::enter(17);
            assert_eq!(TIER2_DIAGNOSTIC_RUNTIME.get(), 17);
            {
                let _inner = Tier2DiagnosticScope::enter(29);
                assert_eq!(TIER2_DIAGNOSTIC_RUNTIME.get(), 29);
            }
            assert_eq!(TIER2_DIAGNOSTIC_RUNTIME.get(), 17);
        }
        assert_eq!(TIER2_DIAGNOSTIC_RUNTIME.get(), initial);
    }

    #[test]
    fn panicking_diagnostic_scope_restores_the_enclosing_runtime() {
        let initial = TIER2_DIAGNOSTIC_RUNTIME.get();
        {
            let _outer = Tier2DiagnosticScope::enter(41);
            let result = std::panic::catch_unwind(|| {
                let _inner = Tier2DiagnosticScope::enter(53);
                assert_eq!(TIER2_DIAGNOSTIC_RUNTIME.get(), 53);
                panic!("simulated compiler panic");
            });
            assert!(result.is_err());
            assert_eq!(TIER2_DIAGNOSTIC_RUNTIME.get(), 41);
        }
        assert_eq!(TIER2_DIAGNOSTIC_RUNTIME.get(), initial);
    }
}

#[cfg(feature = "test-support")]
type Tier2CompileDispositions =
    std::collections::HashMap<(u64, crate::runtime::FunctionKey), Tier2CompileDisposition>;

#[cfg(feature = "test-support")]
fn tier2_compile_dispositions() -> &'static std::sync::Mutex<Tier2CompileDispositions> {
    static DISPOSITIONS: std::sync::OnceLock<std::sync::Mutex<Tier2CompileDispositions>> =
        std::sync::OnceLock::new();
    DISPOSITIONS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(feature = "test-support")]
type Tier2DeoptSites =
    std::collections::HashMap<(u64, crate::runtime::FunctionKey, u32), (u32, u8)>;

#[cfg(feature = "test-support")]
fn tier2_deopt_sites() -> &'static std::sync::Mutex<Tier2DeoptSites> {
    static SITES: std::sync::OnceLock<std::sync::Mutex<Tier2DeoptSites>> =
        std::sync::OnceLock::new();
    SITES.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn test_tier2_deopt_sites(
    runtime: u64,
) -> Vec<((crate::runtime::FunctionKey, u32), (u32, u8))> {
    let mut sites = tier2_deopt_sites()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .filter_map(|(&(owner, key, guard), &pc)| (owner == runtime).then_some(((key, guard), pc)))
        .collect::<Vec<_>>();
    sites.sort_unstable_by_key(|((key, guard), _)| (key.id, key.generation, *guard));
    sites
}

#[cfg(feature = "test-support")]
fn record_tier2_deopt_sites(
    key: crate::runtime::FunctionKey,
    metadata: &OptimizedArtifactMetadata,
    function: &VerifiedFunction,
) {
    let runtime = TIER2_DIAGNOSTIC_RUNTIME.get();
    let mut sites = tier2_deopt_sites()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    sites.retain(|(owner, function, _), _| *owner != runtime || *function != key);
    sites.extend(metadata.deopt_sites().iter().map(|(_, map)| {
        let opcode = function
            .instructions()
            .iter()
            .find(|instruction| instruction.pc() == map.resume_pc())
            .and_then(|instruction| instruction.bytes().first().copied())
            .unwrap_or(u8::MAX);
        ((runtime, key, map.guard()), (map.resume_pc(), opcode))
    }));
}

#[cfg(feature = "test-support")]
fn record_tier2_stage(
    key: crate::runtime::FunctionKey,
    stage: Tier2CompileStage,
    failure: Option<CompileFailure>,
) {
    tier2_compile_dispositions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(
            (TIER2_DIAGNOSTIC_RUNTIME.get(), key),
            Tier2CompileDisposition { stage, failure },
        );
}

#[cfg(feature = "test-support")]
#[doc(hidden)]
pub fn test_tier2_compile_dispositions(
    runtime: u64,
) -> Vec<(crate::runtime::FunctionKey, Tier2CompileDisposition)> {
    let mut dispositions = tier2_compile_dispositions()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
        .filter_map(|(&(owner, key), &disposition)| {
            (owner == runtime).then_some((key, disposition))
        })
        .collect::<Vec<_>>();
    dispositions.sort_unstable_by_key(|(key, _)| (key.id, key.generation));
    dispositions
}

#[cfg(feature = "test-support")]
fn tier2_stage<T>(
    key: crate::runtime::FunctionKey,
    stage: Tier2CompileStage,
    result: Result<T, CompileFailure>,
) -> Result<T, CompileFailure> {
    let preserve_inner = result.is_err()
        && tier2_compile_dispositions()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(&(TIER2_DIAGNOSTIC_RUNTIME.get(), key))
            .is_some_and(|disposition| {
                matches!(
                    disposition.stage,
                    Tier2CompileStage::GenericCallBaseUnderflow
                        | Tier2CompileStage::GenericCallPrefixUnknown
                        | Tier2CompileStage::GenericCallOutputCapacity
                        | Tier2CompileStage::GenericCallSemanticMissing
                        | Tier2CompileStage::GenericCallFrameStateMissing
                        | Tier2CompileStage::GenericCallShapeMismatch
                        | Tier2CompileStage::GenericCallArityMismatch
                        | Tier2CompileStage::GenericCallGuardMissing
                        | Tier2CompileStage::GenericCallEmit
                        | Tier2CompileStage::PropertyFeedbackMissing
                ) && disposition.failure.is_some()
            });
    if !preserve_inner {
        record_tier2_stage(key, stage, result.as_ref().err().copied());
    }
    result
}

#[cfg(not(feature = "test-support"))]
#[inline]
fn tier2_stage<T>(
    _key: crate::runtime::FunctionKey,
    _stage: Tier2CompileStage,
    result: Result<T, CompileFailure>,
) -> Result<T, CompileFailure> {
    result
}

/// Routes monomorphic calls to callees that published a P3a native entry,
/// and compiles this artifact's own native entry when the function is a pure
/// self-recursive Int32 function, routing its self-call sites to it. Other
/// sites keep their existing lowering.
///
/// Every linked callee is added to `callee_dependencies`: the site guards raw
/// object and bytecode addresses, so retiring the callee must invalidate this
/// artifact before a different function can reuse those addresses (ABA).
fn prepare_native_calls(
    isa: &cranelift_codegen::isa::OwnedTargetIsa,
    request: &CompileRequest,
    specialization: &mut NumericSpecialization,
    control: Option<&CompileControl>,
    direct_dependencies: &mut Vec<super::baseline::PublishedBaselineCode>,
    callee_dependencies: &mut Vec<crate::runtime::FunctionKey>,
) -> Option<(
    super::native_call::NativeCallPlan,
    super::baseline::RelocatableCode,
)> {
    if request.side_path_profile().is_some() {
        return None;
    }
    let key = request.key();
    for instruction in request.snapshot().instructions() {
        if let Some(target) = request.native_call_target(instruction.pc()) {
            direct_dependencies.push(target.publication());
            callee_dependencies.push(target.link().callee());
            specialization.native_calls.insert(
                instruction.pc(),
                NativeCallSite {
                    atom: target.plan().atom(),
                    arguments: target.plan().arguments().into(),
                    result: target.plan().result(),
                    callee_identity: target.link().callee_identity(),
                    callee_bytecode_identity: target.link().callee_bytecode_identity(),
                    entry: Some(target.entry() as usize),
                },
            );
        }
    }
    let signature = request.feedback().bounded_specialization(key)?;
    let (plan, code) =
        super::native_call::lower(isa, request.snapshot(), &signature, control).ok()?;
    for instruction in request.snapshot().instructions() {
        if !is_call_site(instruction.opcode().name()) {
            continue;
        }
        let Some(link) = request.feedback().call_link_at(key, instruction.pc()) else {
            continue;
        };
        if link.callee() != key {
            continue;
        }
        specialization.native_calls.insert(
            instruction.pc(),
            NativeCallSite {
                atom: plan.atom(),
                arguments: plan.arguments().into(),
                result: plan.result(),
                callee_identity: link.callee_identity(),
                callee_bytecode_identity: link.callee_bytecode_identity(),
                entry: None,
            },
        );
    }
    Some((plan, code))
}

/// Call opcodes whose call-site feedback the runtime records at the
/// instruction pc; tail calls are lowered as the equivalent call plus return.
fn is_call_site(name: &str) -> bool {
    name.starts_with("call") || name.starts_with("tail_call")
}

fn has_stable_compiled_call(request: &CompileRequest) -> bool {
    let mut found = false;
    for instruction in request.snapshot().instructions() {
        if !is_call_site(instruction.opcode().name()) {
            continue;
        }
        found = true;
        if request
            .feedback()
            .call_specialization_at(request.key(), instruction.pc())
            .is_none()
        {
            return false;
        }
    }
    found
}

impl Compiler for Tier2Compiler {
    fn compile(
        &self,
        request: CompileRequest,
    ) -> Result<crate::code_cache::CompiledArtifact, CompileFailure> {
        #[cfg(feature = "test-support")]
        let _diagnostic_scope = Tier2DiagnosticScope::enter(request.artifact_key().runtime_id);
        let key = request.key();
        if request.snapshot().has_exception_regions() {
            #[cfg(feature = "test-support")]
            record_tier2_stage(
                key,
                Tier2CompileStage::Admission,
                Some(CompileFailure::UnsupportedOpcode),
            );
            return Err(CompileFailure::UnsupportedOpcode);
        }
        if request.tier() != Tier::Optimizing {
            #[cfg(feature = "test-support")]
            record_tier2_stage(
                key,
                Tier2CompileStage::Admission,
                Some(CompileFailure::InvalidArtifact),
            );
            return Err(CompileFailure::InvalidArtifact);
        }
        if request.side_path_profile().is_none()
            && !request.feedback().has_stable_value_for(request.key())
            && request
                .feedback()
                .bounded_specialization(request.key())
                .is_none()
            && !has_stable_compiled_call(&request)
            && request.frame_inline_targets().is_empty()
        {
            #[cfg(feature = "test-support")]
            record_tier2_stage(
                key,
                Tier2CompileStage::Admission,
                Some(CompileFailure::InvalidArtifact),
            );
            return Err(CompileFailure::InvalidArtifact);
        }
        let epoch = request.feedback_epoch().max(self.feedback_epoch);
        let mut specialization = NumericSpecialization::from_feedback(
            request.snapshot(),
            request.key(),
            request.feedback(),
        );
        #[cfg(feature = "test-support")]
        record_tier2_stage(key, Tier2CompileStage::FromFeedback, None);
        specialization.retain_inline_callees(&request);
        #[cfg(feature = "test-support")]
        record_tier2_stage(key, Tier2CompileStage::RetainInlineCallees, None);
        let frame_inline_dependencies = specialization.frame_inline_dependencies();
        let mut ir = match specialization.translate(request.snapshot(), epoch) {
            Err(CompileFailure::InvalidArtifact)
                if !specialization.frame_inline_callees.is_empty() =>
            {
                // Frame-backed inlining is optional. IR construction can reject
                // a candidate before machine lowering (for example at a loop
                // ownership join), so retry the unchanged caller without only
                // that optional candidate instead of blacklisting the function.
                specialization.frame_inline_callees.clear();
                tier2_stage(
                    key,
                    Tier2CompileStage::FrameTranslateRetry,
                    specialization.translate(request.snapshot(), epoch),
                )?
            }
            result => tier2_stage(key, Tier2CompileStage::InitialTranslate, result)?,
        };
        specialization.inline_callees.clear();
        specialization.frame_inline_callees.clear();
        let mut metadata = Self::metadata_from_ir(&ir);
        #[cfg(feature = "test-support")]
        record_tier2_deopt_sites(key, &metadata, request.snapshot());
        let profile = request.side_path_profile();
        if let Some(profile) = profile {
            tier2_stage(
                key,
                Tier2CompileStage::SidePathValidation,
                validate_side_path_profile(&request, profile),
            )?;
        }
        let mut direct_dependencies = Vec::new();
        for instruction in request.snapshot().instructions() {
            if let Some(target) = request.direct_call_target(instruction.pc()) {
                direct_dependencies.push(target.publication());
                specialization.direct_calls.insert(
                    instruction.pc(),
                    DirectCallSite {
                        call: target.call().clone(),
                        entry: target.entry() as usize,
                    },
                );
            }
        }
        #[cfg(feature = "test-support")]
        record_tier2_stage(key, Tier2CompileStage::DirectDependencies, None);
        let mut native_callee_dependencies = Vec::new();
        let native_entry = prepare_native_calls(
            &self.isa,
            &request,
            &mut specialization,
            None,
            &mut direct_dependencies,
            &mut native_callee_dependencies,
        );
        let code = match lower_optimized_machine(&self.isa, &ir, None, profile, &specialization) {
            Err(CompileFailure::InvalidArtifact)
                if ir.scalar_graph().frame_inlined_calls() != 0 =>
            {
                // A retained frame-inline tree is an optional optimization.
                // Confirm that it alone caused rejection by rebuilding without
                // candidates; an ordinary InvalidArtifact still propagates.
                drop(ir);
                drop(metadata);
                ir = tier2_stage(
                    key,
                    Tier2CompileStage::InlineLowerRetryTranslate,
                    specialization.translate(request.snapshot(), epoch),
                )?;
                metadata = Self::metadata_from_ir(&ir);
                tier2_stage(
                    key,
                    Tier2CompileStage::InlineLowerRetry,
                    lower_optimized_machine(&self.isa, &ir, None, profile, &specialization),
                )?
            }
            result => tier2_stage(key, Tier2CompileStage::InitialLower, result)?,
        };
        let direct_signature = (profile.is_none())
            .then(|| request.feedback().bounded_specialization(request.key()))
            .flatten();
        let direct_code = direct_signature.as_ref().and_then(|signature| {
            tier2_stage(
                key,
                Tier2CompileStage::DirectLower,
                lower_direct_call_machine(
                    &self.isa,
                    request.snapshot(),
                    signature,
                    Some(request.feedback()),
                    None,
                ),
            )
            .ok()
        });
        let mut dependencies = vec![crate::code_cache::ArtifactDependency::new(request.key())];
        dependencies.extend(
            specialization
                .calls
                .values()
                .map(|call| crate::code_cache::ArtifactDependency::new(call.callee())),
        );
        dependencies.extend(
            native_callee_dependencies
                .into_iter()
                .map(crate::code_cache::ArtifactDependency::new),
        );
        if ir.scalar_graph().frame_inlined_calls() != 0 {
            dependencies.extend(
                frame_inline_dependencies
                    .into_iter()
                    .map(crate::code_cache::ArtifactDependency::new),
            );
        }
        let mut artifact = artifact_from_relocatable(request, code)
            .with_dependencies(dependencies)
            .with_optimized_metadata(profile.map_or(metadata.clone(), |profile| {
                metadata.with_side_path_profile(profile)
            }));
        #[cfg(feature = "test-support")]
        record_tier2_stage(key, Tier2CompileStage::ArtifactBind, None);
        if let (Some(signature), Some(direct_code)) = (direct_signature, direct_code) {
            let optimized = tier2_stage(
                key,
                Tier2CompileStage::DirectMetadata,
                artifact
                    .optimized_metadata()
                    .cloned()
                    .ok_or(CompileFailure::InvalidArtifact),
            )?
            .with_direct_call_signature(signature);
            artifact = artifact
                .with_optimized_metadata(optimized)
                .with_direct_call_relocatable(direct_code);
        }
        if let Some((plan, native_code)) = native_entry {
            let optimized = tier2_stage(
                key,
                Tier2CompileStage::DirectMetadata,
                artifact
                    .optimized_metadata()
                    .cloned()
                    .ok_or(CompileFailure::InvalidArtifact),
            )?
            .with_native_call_plan(plan);
            artifact = artifact
                .with_optimized_metadata(optimized)
                .with_native_call_relocatable(native_code);
        }
        #[cfg(feature = "test-support")]
        record_tier2_stage(key, Tier2CompileStage::Complete, None);
        Ok(artifact.with_direct_call_dependencies(direct_dependencies))
    }

    fn compile_controlled(
        &self,
        request: CompileRequest,
        control: &CompileControl,
    ) -> Result<crate::code_cache::CompiledArtifact, CompileFailure> {
        #[cfg(feature = "test-support")]
        let _diagnostic_scope = Tier2DiagnosticScope::enter(request.artifact_key().runtime_id);
        let key = request.key();
        tier2_stage(key, Tier2CompileStage::Admission, control.check())?;
        if request.snapshot().has_exception_regions() {
            #[cfg(feature = "test-support")]
            record_tier2_stage(
                key,
                Tier2CompileStage::Admission,
                Some(CompileFailure::UnsupportedOpcode),
            );
            return Err(CompileFailure::UnsupportedOpcode);
        }
        if request.tier() != Tier::Optimizing {
            #[cfg(feature = "test-support")]
            record_tier2_stage(
                key,
                Tier2CompileStage::Admission,
                Some(CompileFailure::InvalidArtifact),
            );
            return Err(CompileFailure::InvalidArtifact);
        }
        if request.side_path_profile().is_none()
            && !request.feedback().has_stable_value_for(request.key())
            && request
                .feedback()
                .bounded_specialization(request.key())
                .is_none()
            && !has_stable_compiled_call(&request)
            && request.frame_inline_targets().is_empty()
        {
            #[cfg(feature = "test-support")]
            record_tier2_stage(
                key,
                Tier2CompileStage::Admission,
                Some(CompileFailure::InvalidArtifact),
            );
            return Err(CompileFailure::InvalidArtifact);
        }
        let epoch = request.feedback_epoch().max(self.feedback_epoch);
        let mut specialization = NumericSpecialization::from_feedback(
            request.snapshot(),
            request.key(),
            request.feedback(),
        );
        #[cfg(feature = "test-support")]
        record_tier2_stage(key, Tier2CompileStage::FromFeedback, None);
        specialization.retain_inline_callees(&request);
        #[cfg(feature = "test-support")]
        record_tier2_stage(key, Tier2CompileStage::RetainInlineCallees, None);
        let frame_inline_dependencies = specialization.frame_inline_dependencies();
        let mut ir = match specialization.translate(request.snapshot(), epoch) {
            Err(CompileFailure::InvalidArtifact)
                if !specialization.frame_inline_callees.is_empty() =>
            {
                tier2_stage(key, Tier2CompileStage::FrameTranslateRetry, control.check())?;
                specialization.frame_inline_callees.clear();
                tier2_stage(
                    key,
                    Tier2CompileStage::FrameTranslateRetry,
                    specialization.translate(request.snapshot(), epoch),
                )?
            }
            result => tier2_stage(key, Tier2CompileStage::InitialTranslate, result)?,
        };
        specialization.inline_callees.clear();
        specialization.frame_inline_callees.clear();
        let mut metadata = Self::metadata_from_ir(&ir);
        #[cfg(feature = "test-support")]
        record_tier2_deopt_sites(key, &metadata, request.snapshot());
        let check_budget = |ir: &OptimizedIr, metadata: &OptimizedArtifactMetadata| {
            control.check_ir_bytes(
                ir.scalar_graph()
                    .allocated_bytes()
                    .saturating_add(ir.scalar_graph().values().len().saturating_mul(
                        core::mem::size_of::<u8>()
                            + core::mem::size_of::<Option<crate::ir::ScalarNumericMode>>(),
                    ))
                    .saturating_add(
                        metadata
                            .deopt_sites()
                            .len()
                            .saturating_mul(core::mem::size_of::<DeoptMap>()),
                    ),
            )
        };
        match check_budget(&ir, &metadata) {
            Err(CompileFailure::ResourceLimit)
                if ir.scalar_graph().inlined_calls() != 0
                    || ir.scalar_graph().frame_inlined_calls() != 0 =>
            {
                // Inlining is optional: retry within the same cancellation and
                // deadline contract after releasing its graph storage.
                drop(ir);
                drop(metadata);
                tier2_stage(key, Tier2CompileStage::IrBudget, control.check())?;
                ir = specialization.translate(request.snapshot(), epoch)?;
                metadata = Self::metadata_from_ir(&ir);
                tier2_stage(
                    key,
                    Tier2CompileStage::IrBudget,
                    check_budget(&ir, &metadata),
                )?;
            }
            result => tier2_stage(key, Tier2CompileStage::IrBudget, result)?,
        }
        let profile = request.side_path_profile();
        if let Some(profile) = profile {
            tier2_stage(
                key,
                Tier2CompileStage::SidePathValidation,
                validate_side_path_profile(&request, profile),
            )?;
        }
        let mut direct_dependencies = Vec::new();
        for instruction in request.snapshot().instructions() {
            if let Some(target) = request.direct_call_target(instruction.pc()) {
                direct_dependencies.push(target.publication());
                specialization.direct_calls.insert(
                    instruction.pc(),
                    DirectCallSite {
                        call: target.call().clone(),
                        entry: target.entry() as usize,
                    },
                );
            }
        }
        #[cfg(feature = "test-support")]
        record_tier2_stage(key, Tier2CompileStage::DirectDependencies, None);
        let mut native_callee_dependencies = Vec::new();
        let native_entry = prepare_native_calls(
            &self.isa,
            &request,
            &mut specialization,
            Some(control),
            &mut direct_dependencies,
            &mut native_callee_dependencies,
        );
        let code = match lower_optimized_machine(
            &self.isa,
            &ir,
            Some(control),
            profile,
            &specialization,
        ) {
            Err(failure)
                if (matches!(failure, CompileFailure::ResourceLimit)
                    && (ir.scalar_graph().inlined_calls() != 0
                        || ir.scalar_graph().frame_inlined_calls() != 0))
                    || (matches!(failure, CompileFailure::InvalidArtifact)
                        && ir.scalar_graph().frame_inlined_calls() != 0) =>
            {
                drop(ir);
                drop(metadata);
                tier2_stage(
                    key,
                    Tier2CompileStage::InlineLowerRetryTranslate,
                    control.check(),
                )?;
                ir = tier2_stage(
                    key,
                    Tier2CompileStage::InlineLowerRetryTranslate,
                    specialization.translate(request.snapshot(), epoch),
                )?;
                metadata = Self::metadata_from_ir(&ir);
                tier2_stage(
                    key,
                    Tier2CompileStage::InlineLowerRetryBudget,
                    check_budget(&ir, &metadata),
                )?;
                tier2_stage(
                    key,
                    Tier2CompileStage::InlineLowerRetry,
                    lower_optimized_machine(
                        &self.isa,
                        &ir,
                        Some(control),
                        profile,
                        &specialization,
                    ),
                )?
            }
            result => tier2_stage(key, Tier2CompileStage::InitialLower, result)?,
        };
        let direct_signature = (profile.is_none())
            .then(|| request.feedback().bounded_specialization(request.key()))
            .flatten();
        let direct_code = direct_signature.as_ref().and_then(|signature| {
            tier2_stage(
                key,
                Tier2CompileStage::DirectLower,
                lower_direct_call_machine(
                    &self.isa,
                    request.snapshot(),
                    signature,
                    Some(request.feedback()),
                    Some(control),
                ),
            )
            .ok()
        });
        tier2_stage(key, Tier2CompileStage::DirectLower, control.check())?;
        let mut dependencies = vec![crate::code_cache::ArtifactDependency::new(request.key())];
        dependencies.extend(
            specialization
                .calls
                .values()
                .map(|call| crate::code_cache::ArtifactDependency::new(call.callee())),
        );
        dependencies.extend(
            native_callee_dependencies
                .into_iter()
                .map(crate::code_cache::ArtifactDependency::new),
        );
        if ir.scalar_graph().frame_inlined_calls() != 0 {
            dependencies.extend(
                frame_inline_dependencies
                    .into_iter()
                    .map(crate::code_cache::ArtifactDependency::new),
            );
        }
        let mut artifact = artifact_from_relocatable(request, code)
            .with_dependencies(dependencies)
            .with_optimized_metadata(profile.map_or(metadata.clone(), |profile| {
                metadata.with_side_path_profile(profile)
            }));
        #[cfg(feature = "test-support")]
        record_tier2_stage(key, Tier2CompileStage::ArtifactBind, None);
        if let (Some(signature), Some(direct_code)) = (direct_signature, direct_code) {
            let optimized = tier2_stage(
                key,
                Tier2CompileStage::DirectMetadata,
                artifact
                    .optimized_metadata()
                    .cloned()
                    .ok_or(CompileFailure::InvalidArtifact),
            )?
            .with_direct_call_signature(signature);
            artifact = artifact
                .with_optimized_metadata(optimized)
                .with_direct_call_relocatable(direct_code);
        }
        if let Some((plan, native_code)) = native_entry {
            let optimized = tier2_stage(
                key,
                Tier2CompileStage::DirectMetadata,
                artifact
                    .optimized_metadata()
                    .cloned()
                    .ok_or(CompileFailure::InvalidArtifact),
            )?
            .with_native_call_plan(plan);
            artifact = artifact
                .with_optimized_metadata(optimized)
                .with_native_call_relocatable(native_code);
        }
        #[cfg(feature = "test-support")]
        record_tier2_stage(key, Tier2CompileStage::Complete, None);
        Ok(artifact.with_direct_call_dependencies(direct_dependencies))
    }
}

fn validate_side_path_profile(
    request: &CompileRequest,
    profile: crate::runtime::SidePathProfile,
) -> Result<(), CompileFailure> {
    if profile.function() != request.key()
        || profile.feedback_epoch() != request.feedback_epoch()
        || !request.feedback().contains_stable_observation(
            request.key(),
            profile.pc(),
            profile.observed(),
        )
        || !matches!(
            profile.observed(),
            crate::runtime::ObservedType::Int32 | crate::runtime::ObservedType::Float64
        )
    {
        return Err(CompileFailure::InvalidArtifact);
    }
    Ok(())
}
use crate::{
    bytecode::VerifiedFunction,
    code_cache::OptimizedArtifactMetadata,
    ir::{DeoptMap, OptimizedIr},
    runtime::{CompileRequest, Tier},
};

use super::{
    baseline::{artifact_from_relocatable, BaselineCompiler},
    CompileControl, CompileFailure, Compiler,
};
