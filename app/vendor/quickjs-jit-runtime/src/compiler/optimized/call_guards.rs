//! Speculative call-target guards in a dominating entry block.
//!
//! This consumes Maglev-style SSA identities and JSC-style dominance proofs.
//! Initial admission uses the existing effect-free scalar-region proof. The
//! target remains rooted in an unchanged argument; an actual observable poll
//! revalidates the guard before returning to the loop.

use super::*;

// Hoisting is optional. Each analysis phase has this hard work ceiling, even
// without a CompileControl (for example in diagnostic lowering). Emission has
// a separate bound because each target is copied at entry and every poll.
const MAX_ANALYSIS_WORK: usize = 1_048_576;
const MAX_TARGET_CHECKS: usize = 512;
const MAX_EMISSION_UNITS: usize = 65_536;
const BYTES_PER_EMISSION_UNIT: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CallTargetFact {
    argument: u16,
    object: u64,
    bytecode: u64,
}

#[derive(Default)]
pub(super) struct HoistedCallGuards {
    targets: std::collections::BTreeMap<u16, CallTargetFact>,
    calls: std::collections::BTreeMap<u32, u16>,
}

pub(super) struct GuardState {
    variables: std::collections::BTreeMap<
        u16,
        (cranelift_frontend::Variable, cranelift_frontend::Variable),
    >,
}

impl HoistedCallGuards {
    pub(super) fn scratch_bytes(ir: &OptimizedIr) -> usize {
        if ir.scalar_graph().inlined_calls() == 0 {
            return 0;
        }
        let work = Self::analysis_work(ir);
        let (emission, arguments) = Self::emission_shape(ir).map_or(
            (MAX_EMISSION_UNITS, 0),
            |(sites, slots, arguments)| {
                let targets = usize::try_from(ir.scalar_graph().inlined_calls())
                    .unwrap_or(usize::MAX)
                    .min(arguments);
                // Fewer distinct admitted targets may fit even when this upper
                // bound does not. Such a plan can never exceed the emission cap.
                (
                    Self::emission_units(targets, sites, slots).unwrap_or(MAX_EMISSION_UNITS),
                    arguments,
                )
            },
        );
        crate::ir::LoopAnalysis::bytes_upper_bound(ir, work)
            .saturating_add(
                ir.scalar_graph()
                    .values()
                    .len()
                    .saturating_mul(crate::ir::KnownFacts::bytes_per_value()),
            )
            .saturating_add(ir.nodes().len().saturating_mul(256))
            .saturating_add(arguments.saturating_mul(64))
            .saturating_add(emission.saturating_mul(BYTES_PER_EMISSION_UNIT))
    }

    pub(super) fn analyze(ir: &OptimizedIr, scalar_region: bool) -> Self {
        if !scalar_region || ir.scalar_graph().inlined_calls() == 0 {
            return Self::default();
        }
        Self::try_analyze(ir, Self::analysis_work(ir)).unwrap_or_default()
    }

    fn analysis_work(ir: &OptimizedIr) -> usize {
        ir.scalar_graph()
            .values()
            .len()
            .saturating_add(ir.nodes().len())
            .saturating_mul(128)
            .min(MAX_ANALYSIS_WORK)
    }

    fn emission_shape(ir: &OptimizedIr) -> Option<(usize, usize, usize)> {
        let shape = ir.guard_maps().first()?.shape();
        let sites = ir
            .nodes()
            .iter()
            .filter(|node| {
                matches!(
                    node.kind(),
                    crate::ir::OptimizedNodeKind::GuardNumeric { .. }
                )
            })
            .count();
        Some((sites, shape.slot_count(), usize::from(shape.arguments())))
    }

    fn emission_units(targets: usize, sites: usize, slots: usize) -> Option<usize> {
        if targets == 0 {
            return Some(0);
        }
        if targets.checked_mul(sites)? > MAX_TARGET_CHECKS {
            return None;
        }
        // Charge guard blocks/instructions, full-frame recovery, and SSA
        // variable propagation through each guard's predecessor blocks.
        let per_target = slots.checked_mul(4)?.checked_add(32)?;
        let per_site = targets
            .checked_mul(per_target)?
            .checked_add(slots.checked_mul(8)?)?
            .checked_add(64)?;
        let units = sites.checked_mul(per_site)?;
        (units <= MAX_EMISSION_UNITS).then_some(units)
    }

    fn try_analyze(ir: &OptimizedIr, mut work: usize) -> Option<Self> {
        let mut plan = Self::default();
        // Pay for the emission-site scan before performing it. Every later
        // traversal and tree lookup is charged before work or allocation.
        work = work.checked_sub(ir.nodes().len())?;
        let (sites, slots, arguments) = Self::emission_shape(ir)?;
        let graph = ir.scalar_graph();
        if graph.values().len() > work {
            return None;
        }
        let loops = crate::ir::LoopAnalysis::analyze(ir, work);
        if !loops.is_complete() {
            return None;
        }
        let facts = crate::ir::KnownFacts::analyze(graph, ir.nodes(), true, work);
        let tree_cost =
            (usize::BITS - ir.nodes().len().max(arguments).leading_zeros()) as usize + 1;
        let mut written_arguments = std::collections::BTreeSet::new();
        for node in ir.nodes() {
            work = work.checked_sub(1)?;
            if !matches!(node.kind(), crate::ir::OptimizedNodeKind::Bytecode { .. }) {
                continue;
            }
            for (slot, _) in graph.frame_definitions_for_node(node.id()) {
                work = work.checked_sub(tree_cost)?;
                if let crate::ir::FrameSlot::Argument(index) = slot {
                    written_arguments.insert(*index);
                }
            }
        }
        for node in ir.nodes() {
            work = work.checked_sub(1)?;
            if !loops.contains(node.id()) {
                continue;
            }
            let Some(call) = graph.call(node.id()) else {
                continue;
            };
            // Result aliases can expose the same ScalarCall through another
            // node. Only its semantic call boundary needs guard coverage.
            if call.frame_state_node != node.id() {
                continue;
            }
            let Some(region) = call.inline.as_ref() else {
                continue;
            };
            let Some(argument) = facts.entry_argument(call.target) else {
                continue;
            };
            work = work.checked_sub(tree_cost.checked_mul(3)?)?;
            if written_arguments.contains(&argument) {
                continue;
            }
            let target = CallTargetFact {
                argument,
                object: region.object_identity,
                bytecode: region.bytecode_identity,
            };
            // A later conflicting site retains its own guard. The earlier
            // target can remain hoisted without an impossible conjunction.
            match plan.targets.entry(argument) {
                std::collections::btree_map::Entry::Occupied(prior) => {
                    if *prior.get() != target {
                        continue;
                    }
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(target);
                }
            }
            // Exhaustion discards the entire plan, including earlier sites.
            Self::emission_units(plan.targets.len(), sites, slots)?;
            plan.calls.insert(node.id(), argument);
        }
        Some(plan)
    }

    pub(super) fn contains(&self, node: u32) -> bool {
        self.calls.contains_key(&node)
    }
    pub(super) fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    pub(super) fn create_state(
        &self,
        builder: &mut cranelift_frontend::FunctionBuilder<'_>,
        next_var: &mut u32,
    ) -> GuardState {
        use cranelift_codegen::ir::{types, InstBuilder};
        let mut variables = std::collections::BTreeMap::new();
        for &argument in self.targets.keys() {
            let valid = cranelift_frontend::Variable::from_u32(*next_var);
            let used = cranelift_frontend::Variable::from_u32(next_var.saturating_add(1));
            *next_var = next_var.saturating_add(2);
            builder.declare_var(valid, types::I8);
            builder.declare_var(used, types::I8);
            let zero = builder.ins().iconst(types::I8, 0);
            builder.def_var(valid, zero);
            builder.def_var(used, zero);
            variables.insert(argument, (valid, used));
        }
        GuardState { variables }
    }

    /// Probe at function entry without deoptimizing. This keeps the identity
    /// loads out of the loop while preserving zero-trip and skipped-call
    /// semantics: a mismatch matters only if the call is actually reached.
    pub(super) fn probe_entry(
        &self,
        state: &GuardState,
        builder: &mut cranelift_frontend::FunctionBuilder<'_>,
        env: &OptEnv<'_>,
    ) -> Result<(), CompileFailure> {
        use cranelift_codegen::ir::{types, InstBuilder};
        for target in self.targets.values() {
            let &(valid, _) = state
                .variables
                .get(&target.argument)
                .ok_or(CompileFailure::InvalidArtifact)?;
            let matched = builder.create_block();
            let missed = builder.create_block();
            let done = builder.create_block();
            let pair = opt_use(
                builder,
                *env.arguments
                    .get(usize::from(target.argument))
                    .ok_or(CompileFailure::InvalidArtifact)?,
            );
            super::super::emit_guarded_direct_callee_identity(
                builder,
                pair.tag,
                pair.payload,
                super::super::DirectCalleeIdentity {
                    object: target.object,
                    bytecode: target.bytecode,
                },
                env.pointer_type,
                matched,
                missed,
            );
            builder.switch_to_block(matched);
            let one = builder.ins().iconst(types::I8, 1);
            builder.def_var(valid, one);
            builder.ins().jump(done, &[]);
            builder.switch_to_block(missed);
            let zero = builder.ins().iconst(types::I8, 0);
            builder.def_var(valid, zero);
            builder.ins().jump(done, &[]);
            builder.switch_to_block(done);
        }
        Ok(())
    }

    pub(super) fn admit_call(
        &self,
        state: &GuardState,
        builder: &mut cranelift_frontend::FunctionBuilder<'_>,
        env: &OptEnv<'_>,
        node: &crate::ir::OptimizedNode,
        provenance: &[OptProvenance],
        depth: usize,
    ) -> Result<(), CompileFailure> {
        use cranelift_codegen::ir::{condcodes::IntCC, types, InstBuilder};
        if !self.contains(node.id()) {
            return Ok(());
        }
        let argument = *self
            .calls
            .get(&node.id())
            .ok_or(CompileFailure::InvalidArtifact)?;
        let &(valid, used) = state
            .variables
            .get(&argument)
            .ok_or(CompileFailure::InvalidArtifact)?;
        let pass = builder.create_block();
        let deopt = builder.create_block();
        builder.set_cold_block(deopt);
        let valid_value = builder.use_var(valid);
        let okay = builder.ins().icmp_imm(IntCC::NotEqual, valid_value, 0);
        builder.ins().brif(okay, pass, &[], deopt, &[]);
        builder.switch_to_block(deopt);
        emit_opt_deopt(
            builder,
            env,
            provenance,
            depth,
            node.pc(),
            node.deopt_guard().ok_or(CompileFailure::InvalidArtifact)?,
        )?;
        builder.switch_to_block(pass);
        let one = builder.ins().iconst(types::I8, 1);
        builder.def_var(used, one);
        Ok(())
    }

    /// Both entry and loop-header guards have an empty operand stack and an
    /// existing exact deopt map. Reuse those maps, never a moved call's PC.
    pub(super) fn emit(
        &self,
        state: &GuardState,
        builder: &mut cranelift_frontend::FunctionBuilder<'_>,
        env: &OptEnv<'_>,
        pc: u32,
        guard: u32,
    ) -> Result<(), CompileFailure> {
        use cranelift_codegen::ir::{types, InstBuilder};
        if self.is_empty() {
            return Ok(());
        }
        let deopt = builder.create_block();
        builder.set_cold_block(deopt);
        for target in self.targets.values() {
            // The poll itself is observable, even before this invocation's
            // first guarded call. Always refresh validity; only a target that
            // was already used needs an eager deopt here. Otherwise admit_call
            // performs the lazy deopt if execution later reaches the call.
            let &(valid, used) = state
                .variables
                .get(&target.argument)
                .ok_or(CompileFailure::InvalidArtifact)?;
            let matched = builder.create_block();
            let missed = builder.create_block();
            let skip = builder.create_block();
            let variables = env
                .arguments
                .get(usize::from(target.argument))
                .ok_or(CompileFailure::InvalidArtifact)?;
            let pair = opt_use(builder, *variables);
            super::super::emit_guarded_direct_callee_identity(
                builder,
                pair.tag,
                pair.payload,
                super::super::DirectCalleeIdentity {
                    object: target.object,
                    bytecode: target.bytecode,
                },
                env.pointer_type,
                matched,
                missed,
            );
            builder.switch_to_block(matched);
            let one = builder.ins().iconst(types::I8, 1);
            builder.def_var(valid, one);
            builder.ins().jump(skip, &[]);
            builder.switch_to_block(missed);
            let zero = builder.ins().iconst(types::I8, 0);
            builder.def_var(valid, zero);
            let was_used = builder.use_var(used);
            builder.ins().brif(was_used, deopt, &[], skip, &[]);
            builder.switch_to_block(skip);
        }
        let done = builder.create_block();
        builder.ins().jump(done, &[]);
        builder.switch_to_block(deopt);
        emit_opt_deopt(builder, env, &[], 0, pc, guard)?;
        builder.switch_to_block(done);
        Ok(())
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::{
        bytecode::VerifyLimits,
        code_cache::CompiledArtifact,
        ir::InlineCallee,
        runtime::{FeedbackTable, FunctionKey, ObservedType, Tier},
        test_support::SnapshotFixture,
    };

    fn inline_ir(source: &str) -> OptimizedIr {
        inline_ir_with_conflicting_site(source, None)
    }

    fn inline_ir_with_conflicting_site(source: &str, conflicting: Option<usize>) -> OptimizedIr {
        let caller_fixture = SnapshotFixture::compile(source);
        let caller = caller_fixture
            .snapshot()
            .verify(VerifyLimits::default())
            .unwrap();
        let callee_fixture = SnapshotFixture::compile("(function(value){return value+1})");
        let body = callee_fixture
            .snapshot()
            .verify(VerifyLimits::default())
            .unwrap();
        let caller_key = FunctionKey::new(
            caller.snapshot().function_id(),
            caller.snapshot().generation(),
        );
        let callee = FunctionKey::new(body.snapshot().function_id(), body.snapshot().generation());
        let pcs: Vec<_> = caller
            .instructions()
            .iter()
            .filter(|instruction| instruction.opcode().name() == "call1")
            .map(|instruction| instruction.pc())
            .collect();
        assert!(!pcs.is_empty());
        let mut feedback = FeedbackTable::new(64, 2);
        for (index, &pc) in pcs.iter().enumerate() {
            let identity = if Some(index) == conflicting {
                0x12345679
            } else {
                0x12345678
            };
            for _ in 0..32 {
                feedback.observe_call_signature_with_identity(
                    caller_key,
                    pc,
                    callee,
                    identity,
                    0x22345678,
                    &[ObservedType::Int32],
                    ObservedType::Int32,
                );
            }
        }
        let feedback = feedback.snapshot(302);
        let mut artifact = CompiledArtifact::fake(Tier::Baseline).key();
        artifact.function_id = callee.id;
        artifact.generation = callee.generation;
        artifact.source_revision = body.snapshot().source_revision();
        artifact.opcode_fingerprint = body.snapshot().opcode_fingerprint();
        let callees = pcs
            .iter()
            .map(|&pc| {
                (
                    pc,
                    InlineCallee {
                        artifact,
                        call: feedback.call_specialization_at(caller_key, pc).unwrap(),
                        body: body.clone(),
                    },
                )
            })
            .collect();
        let ir =
            OptimizedIr::translate_with_inline_callees(&caller, 302, &Default::default(), &callees)
                .unwrap();
        assert_eq!(ir.scalar_graph().inlined_calls(), pcs.len() as u64);
        ir
    }

    #[test]
    fn excessive_poll_expansion_retains_all_site_guards() {
        let mut source = String::from("(function(a,b,c,d,e,f,g,h){let value=0,i=0;");
        for _ in 0..64 {
            source.push_str("for(i=0;i<4;i++){");
            for target in ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h'] {
                source.push_str(&format!("value={target}(value);"));
            }
            source.push('}');
        }
        source.push_str("return value;})");
        let ir = inline_ir(&source);
        let work = HoistedCallGuards::analysis_work(&ir) - ir.nodes().len();
        assert!(crate::ir::LoopAnalysis::analyze(&ir, work).is_complete());
        let facts = crate::ir::KnownFacts::analyze(ir.scalar_graph(), ir.nodes(), true, work);
        assert!(ir
            .nodes()
            .iter()
            .filter_map(|node| ir.scalar_graph().call(node.id()))
            .all(|call| facts.entry_argument(call.target).is_some()));
        assert!(
            HoistedCallGuards::analyze(&ir, true).is_empty(),
            "eight targets at 64 poll locations must retain site guards"
        );
    }

    #[test]
    fn repeated_sites_share_a_target_without_losing_call_coverage() {
        let ir = inline_ir("(function(target){let value=0;for(let i=0;i<4;i++){value=target(value);value=target(value)}return value})");
        let plan = HoistedCallGuards::analyze(&ir, true);
        assert_eq!(plan.targets.len(), 1);
        for node in ir.nodes().iter().filter(|node| {
            ir.scalar_graph()
                .call(node.id())
                .is_some_and(|call| call.frame_state_node == node.id())
        }) {
            assert!(plan.contains(node.id()));
        }
    }

    #[test]
    fn conflicting_site_keeps_its_identity_check() {
        let ir = inline_ir_with_conflicting_site(
            "(function(target){let value=0;for(let i=0;i<4;i++){value=target(value);value=target(value)}return value})",
            Some(1),
        );
        let calls: Vec<_> = ir
            .nodes()
            .iter()
            .filter(|node| {
                ir.scalar_graph()
                    .call(node.id())
                    .is_some_and(|call| call.frame_state_node == node.id())
            })
            .map(|node| node.id())
            .collect();
        let plan = HoistedCallGuards::analyze(&ir, true);
        assert_eq!(plan.targets.len(), 1);
        assert!(plan.contains(calls[0]));
        assert!(!plan.contains(calls[1]));
    }

    #[test]
    fn planning_exhaustion_discards_partially_selected_sites() {
        let ir = inline_ir("(function(target){let value=0;for(let i=0;i<4;i++){value=target(value);value=target(value)}return value})");
        let mut exhausted = false;
        let mut complete = false;
        for work in 0..=HoistedCallGuards::analysis_work(&ir) {
            let plan = HoistedCallGuards::try_analyze(&ir, work).unwrap_or_default();
            if plan.is_empty() {
                exhausted = true;
                assert!(plan.calls.is_empty());
            } else {
                complete = true;
                assert_eq!(
                    plan.calls.len(),
                    2,
                    "budget exhaustion must not retain a partial call plan"
                );
            }
        }
        assert!(exhausted && complete);
    }
}
