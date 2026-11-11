//! Bounded construction of pure SSA and tagged-frame inline regions.
//!
//! Pure admission follows statically selected paths with pre-call recovery.
//! Frame-backed admission keeps effects and exact callee/root continuations;
//! its bounded consumer handles acyclic branches and locals in a tagged frame.

use super::{BaselineIr, OptimizedFrameShape, ScalarBinaryOp};
use crate::{
    bytecode::VerifiedFunction,
    code_cache::ArtifactKey,
    runtime::{CallLinkStatus, CallSpecializationKey, FeedbackRepresentation},
};

#[derive(Clone, Debug)]
pub struct InlineCallee {
    pub artifact: ArtifactKey,
    pub call: CallSpecializationKey,
    pub body: VerifiedFunction,
}

/// A retained target independent of scalar direct-call entry availability.
#[derive(Clone, Debug)]
pub struct FrameInlineCallee {
    pub artifact: ArtifactKey,
    pub target: CallLinkStatus,
    pub body: VerifiedFunction,
    /// Monomorphic callees rooted at CALL bytecode offsets in `body`.
    pub children: std::collections::BTreeMap<u32, FrameInlineCallee>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InlineCallKind {
    Call,
    Method,
}

impl InlineCallKind {
    pub const fn abi_kind(self) -> u32 {
        match self {
            Self::Call => rquickjs_core::qjs::JS_JIT_INLINE_CALL,
            Self::Method => rquickjs_core::qjs::JS_JIT_INLINE_CALL_METHOD,
        }
    }
}

/// Root recovery after the runtime has completed the pending CALL. Values in
/// this identity map already own their slots; caller SSA must not overwrite them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InlineContinuation {
    pub call_node: u32,
    pub call_pc: u32,
    pub resume_pc: u32,
    pub guard: u32,
    pub shape: OptimizedFrameShape,
}

/// Actual bytecode boundaries, excluding the semantic IR's inserted polls.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InlineFrameBoundary {
    pub pc: u32,
    pub next_pc: u32,
    pub stack_before: u16,
    pub stack_after: u16,
}

/// General tagged execution in a runtime-owned, stable shadow frame. Unlike
/// ScalarInlineRegion, failures after Enter resume the callee's exact state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScalarFrameInlineRegion {
    pub artifact: ArtifactKey,
    pub object_identity: u64,
    pub bytecode_identity: u64,
    pub call_kind: InlineCallKind,
    pub body: BaselineIr,
    pub boundaries: Box<[InlineFrameBoundary]>,
    /// Recursively expanded native callees keyed by this frame's CALL PC.
    pub children: std::collections::BTreeMap<u32, Box<ScalarFrameInlineRegion>>,
    pub continuation: InlineContinuation,
    storage_bound: usize,
}

impl ScalarFrameInlineRegion {
    pub(super) fn allocated_bytes(&self) -> usize {
        // Includes conservative Vec spare capacity and the continuation map
        // retained by both OptimizedIr and compiled artifact metadata.
        self.children
            .values()
            .fold(self.storage_bound, |total, child| {
                total.saturating_add(child.allocated_bytes())
            })
    }

    pub fn total_regions(&self) -> usize {
        1usize.saturating_add(
            self.children
                .values()
                .map(|child| child.total_regions())
                .sum::<usize>(),
        )
    }
}

pub(super) const FRAME_INLINE_BUDGET: usize = 1024 * 1024;

impl FrameInlineCallee {
    /// Runs the same bounded admission used by scalar rewriting without
    /// retaining the resulting region. Runtime tiering uses this to
    /// distinguish a target that is still compiling from one whose immutable
    /// body cannot consume a frame-inline call IC.
    pub(crate) fn admission_probe(
        &self,
        kind: InlineCallKind,
        caller_shape: OptimizedFrameShape,
    ) -> bool {
        let Some(resume_pc) = self.target.pc().checked_add(1) else {
            return false;
        };
        let mut budget = FRAME_INLINE_BUDGET;
        self.plan(
            kind,
            InlineContinuation {
                call_node: 0,
                call_pc: self.target.pc(),
                resume_pc,
                guard: 1,
                shape: caller_shape,
            },
            &mut budget,
        )
        .is_some()
    }

    pub(super) fn plan(
        &self,
        kind: InlineCallKind,
        continuation: InlineContinuation,
        budget: &mut usize,
    ) -> Option<Box<ScalarFrameInlineRegion>> {
        self.plan_nested(kind, continuation, budget, 1)
    }

    fn plan_nested(
        &self,
        kind: InlineCallKind,
        continuation: InlineContinuation,
        budget: &mut usize,
        depth: usize,
    ) -> Option<Box<ScalarFrameInlineRegion>> {
        // Charge failed admission attempts too, before scanning or allocating.
        *budget = budget.checked_sub(256)?;
        let snapshot = self.body.snapshot();
        let instructions = self.body.instructions();
        let slots = usize::from(snapshot.arg_count())
            .checked_add(usize::from(snapshot.local_count()))?
            .checked_add(usize::from(snapshot.stack_size()))?;
        if depth > 4
            || self.target.callee().id != snapshot.function_id()
            || self.target.callee().generation != snapshot.generation()
            || self.target.pc() != continuation.call_pc
            || self.artifact.function_id != snapshot.function_id()
            || self.artifact.generation != snapshot.generation()
            || self.artifact.source_revision != snapshot.source_revision()
            || self.artifact.opcode_fingerprint != snapshot.opcode_fingerprint()
            || self.target.callee_identity() == 0
            || self.target.callee_bytecode_identity() == 0
            || instructions.len() > 128
            || slots > 64
            || snapshot.retained_bytes() > 16 * 1024
            || snapshot.closure_count() != 0
            || !snapshot.exception_map().is_empty()
            || self.body.has_exception_regions()
            || self.body.control_flow_graph().blocks().len() > 16
            || self.body.control_flow_graph().blocks().iter().any(|block| {
                block
                    .successors()
                    .iter()
                    .any(|successor| *successor <= block.start_pc())
            })
            || instructions
                .iter()
                .any(|instruction| instruction.opcode().name().starts_with("tail_call"))
            // A callee that instantiates closures may alias its shadow-frame
            // locals through attached var refs; keep it out of frame inlining.
            || instructions.iter().any(|instruction| {
                matches!(
                    instruction.opcode().name(),
                    "fclosure" | "fclosure8" | "close_loc"
                )
            })
        {
            return None;
        }
        // Baseline translation reserves at most pops+12 helper/marker/poll
        // states per bytecode, each containing all frame slots plus scratch.
        // 64 bytes per slot/state unit covers nested boxes and Vec growth.
        let state_count = instructions.iter().try_fold(0usize, |sum, instruction| {
            sum.checked_add(super::optimized::effective_pop(instruction).checked_add(12)?)
        })?;
        let storage_bound = state_count
            .checked_mul(slots.checked_add(4)?)?
            .checked_mul(64)?
            .checked_add(continuation.shape.slot_count().checked_mul(128)?)?
            .checked_add(core::mem::size_of::<ScalarFrameInlineRegion>())?;
        *budget = budget.checked_sub(storage_bound)?;
        let body = BaselineIr::translate(&self.body).ok()?;
        // The initial backend supports tagged-frame operations below and can
        // resume later unsupported instructions exactly. Do not call an
        // Enter/Resume-at-zero wrapper an expanded native callee.
        let first = body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .find(|instruction| {
                !matches!(
                    instruction.op,
                    super::IrOp::Poll { .. } | super::IrOp::OsrLabel { .. } | super::IrOp::Nop
                )
            })?;
        if !matches!(
            first.op,
            super::IrOp::Push(_)
                | super::IrOp::GetArgument(_)
                | super::IrOp::GetLocal(_)
                | super::IrOp::GetLocalChecked(_)
                | super::IrOp::SetLocalUninitialized(_)
                | super::IrOp::Return
                | super::IrOp::ReturnUndefined
        ) {
            return None;
        }
        let cfg = self.body.control_flow_graph();
        let mut boundaries = Vec::with_capacity(instructions.len());
        for (block, lowered) in cfg.blocks().iter().zip(&body.blocks) {
            if block.start_pc() != lowered.start_pc {
                return None;
            }
            let mut depth = usize::from(lowered.stack_depth);
            for instruction in &instructions[block.instruction_range()] {
                let next_depth = depth
                    .checked_sub(super::optimized::effective_pop(instruction))?
                    .checked_add(usize::from(instruction.opcode().n_push()))?;
                boundaries.push(InlineFrameBoundary {
                    pc: instruction.pc(),
                    next_pc: instruction
                        .pc()
                        .checked_add(u32::try_from(instruction.size()).ok()?)?,
                    stack_before: u16::try_from(depth).ok()?,
                    stack_after: u16::try_from(next_depth).ok()?,
                });
                depth = next_depth;
            }
        }
        let mut children = std::collections::BTreeMap::new();
        for instruction in instructions {
            let Some(child) = self.children.get(&instruction.pc()) else {
                continue;
            };
            let child_kind = match instruction.opcode().name() {
                "call" | "call0" | "call1" | "call2" | "call3" => InlineCallKind::Call,
                "call_method" => InlineCallKind::Method,
                _ => continue,
            };
            let child_continuation = InlineContinuation {
                // Nested frames recover through the runtime-owned frame chain;
                // the root post-effect map remains the final native exit.
                call_node: continuation.call_node,
                call_pc: instruction.pc(),
                resume_pc: instruction
                    .pc()
                    .checked_add(u32::try_from(instruction.size()).ok()?)?,
                guard: continuation.guard,
                shape: continuation.shape,
            };
            if let Some(region) = child.plan_nested(
                child_kind,
                child_continuation,
                budget,
                depth.checked_add(1)?,
            ) {
                children.insert(instruction.pc(), region);
            }
        }
        Some(Box::new(ScalarFrameInlineRegion {
            artifact: self.artifact,
            object_identity: self.target.callee_identity(),
            bytecode_identity: self.target.callee_bytecode_identity(),
            call_kind: kind,
            body,
            boundaries: boundaries.into(),
            children,
            continuation,
            storage_bound,
        }))
    }
}

#[derive(Clone, Debug)]
pub(super) enum InlineValue {
    Argument(usize),
    Int32(i32),
    Bool(bool),
    Binary {
        op: ScalarBinaryOp,
        lhs: usize,
        rhs: usize,
        pc: u32,
        stack: Box<[usize]>,
    },
}

#[derive(Debug)]
pub(super) struct InlinePlan {
    pub values: Vec<InlineValue>,
    pub result: usize,
}

impl InlineCallee {
    pub(super) fn plan(&self, argument_bool: impl Fn(usize) -> Option<bool>) -> Option<InlinePlan> {
        let snapshot = self.body.snapshot();
        if self.call.callee().id != snapshot.function_id()
            || self.call.callee().generation != snapshot.generation()
            || self.artifact.function_id != snapshot.function_id()
            || self.artifact.generation != snapshot.generation()
            || self.artifact.source_revision != snapshot.source_revision()
            || self.artifact.opcode_fingerprint != snapshot.opcode_fingerprint()
            || self.call.callee_identity() == 0
            || self.call.callee_bytecode_identity() == 0
            || self.call.arguments().len() != usize::from(snapshot.arg_count())
            || self.call.result() != FeedbackRepresentation::Int32
            || self.call.arguments().iter().any(|r| {
                !matches!(
                    r,
                    FeedbackRepresentation::Int32 | FeedbackRepresentation::Bool
                )
            })
            || snapshot.local_count() != 0
            || snapshot.closure_count() != 0
            || !snapshot.exception_map().is_empty()
            || self.body.has_exception_regions()
            || self.body.instructions().len() > 128
        {
            return None;
        }
        let mut values = (0..self.call.arguments().len())
            .map(InlineValue::Argument)
            .collect::<Vec<_>>();
        let mut stack = Vec::new();
        let instructions = self.body.instructions();
        let mut cursor = 0;
        for _ in 0..128 {
            let instruction = instructions.get(cursor)?;
            let name = instruction.opcode().name();
            let mut push = |value| {
                let id = values.len();
                values.push(value);
                stack.push(id);
            };
            if let Some(value) = super::scalar::integer_constant(name, instruction.bytes()) {
                push(InlineValue::Int32(value));
            } else if matches!(name, "push_true" | "push_false") {
                push(InlineValue::Bool(name == "push_true"));
            } else if name.starts_with("get_arg") {
                let index = usize::from(super::optimized::indexed_node_operand(
                    name,
                    instruction.bytes(),
                )?);
                if index >= self.call.arguments().len() {
                    return None;
                }
                stack.push(index);
            } else {
                match name {
                    "add" | "sub" => {
                        let state = stack.clone().into_boxed_slice();
                        let rhs = stack.pop()?;
                        let lhs = stack.pop()?;
                        let numeric = |id: usize| match values.get(id) {
                            Some(InlineValue::Argument(index)) => {
                                self.call.arguments()[*index] == FeedbackRepresentation::Int32
                            }
                            Some(InlineValue::Int32(_) | InlineValue::Binary { .. }) => true,
                            _ => false,
                        };
                        if !numeric(lhs) || !numeric(rhs) {
                            return None;
                        }
                        let id = values.len();
                        values.push(InlineValue::Binary {
                            op: if name == "add" {
                                ScalarBinaryOp::Add
                            } else {
                                ScalarBinaryOp::Sub
                            },
                            lhs,
                            rhs,
                            pc: instruction.pc(),
                            stack: state,
                        });
                        stack.push(id);
                    }
                    "if_false" | "if_false8" | "if_true" | "if_true8" => {
                        let condition = stack.pop()?;
                        let truth = match values.get(condition)? {
                            InlineValue::Bool(value) => *value,
                            InlineValue::Int32(value) => *value != 0,
                            InlineValue::Argument(index) => argument_bool(*index)?,
                            _ => return None,
                        };
                        if truth == name.starts_with("if_true") {
                            let target = u32::try_from(instruction.branch_target()?).ok()?;
                            if target <= instruction.pc() {
                                return None;
                            }
                            cursor = instructions
                                .binary_search_by_key(&target, |i| i.pc())
                                .ok()?;
                            continue;
                        }
                    }
                    "goto" | "goto8" | "goto16" => {
                        let target = u32::try_from(instruction.branch_target()?).ok()?;
                        if target <= instruction.pc() {
                            return None;
                        }
                        cursor = instructions
                            .binary_search_by_key(&target, |i| i.pc())
                            .ok()?;
                        continue;
                    }
                    "drop" => {
                        stack.pop()?;
                    }
                    "nop" => {}
                    "return" => {
                        let result = stack.pop()?;
                        if !stack.is_empty() {
                            return None;
                        }
                        match values.get(result)? {
                            InlineValue::Bool(_) => return None,
                            InlineValue::Argument(index)
                                if self.call.arguments()[*index]
                                    != FeedbackRepresentation::Int32 =>
                            {
                                return None
                            }
                            _ => return Some(InlinePlan { values, result }),
                        }
                    }
                    _ => return None,
                }
            }
            if stack.len() > 16 {
                return None;
            }
            cursor += 1;
        }
        None
    }
}

#[cfg(all(test, feature = "test-support"))]
mod frame_inline_tests {
    use super::*;
    use crate::{
        bytecode::VerifyLimits,
        code_cache::CompiledArtifact,
        ir::{DeoptPhase, OptimizedEffect, OptimizedIr},
        runtime::{FeedbackTable, FunctionKey, ObservedType, Tier},
        test_support::SnapshotFixture,
    };

    fn inputs(
        caller_source: &str,
        callee_source: &str,
    ) -> (
        VerifiedFunction,
        std::collections::BTreeMap<u32, FrameInlineCallee>,
        crate::runtime::FeedbackSnapshot,
    ) {
        let caller_fixture = SnapshotFixture::compile(caller_source);
        let caller = caller_fixture
            .snapshot()
            .verify(VerifyLimits::default())
            .unwrap();
        let callee_fixture = SnapshotFixture::compile(callee_source);
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
            .filter(|instruction| instruction.opcode().name().contains("call"))
            .map(|instruction| instruction.pc())
            .collect();
        let mut feedback = FeedbackTable::new(64, 2);
        for _ in 0..32 {
            feedback.observe_call(
                caller_key,
                &[
                    ObservedType::Object,
                    ObservedType::Object,
                    ObservedType::Int32,
                ],
            );
            feedback.observe_return(caller_key, 0, ObservedType::Int32);
        }
        for &pc in &pcs {
            for _ in 0..32 {
                feedback.observe_call_signature_with_identity(
                    caller_key,
                    pc,
                    callee,
                    0x12345678,
                    0x22345678,
                    &[ObservedType::Object, ObservedType::Int32],
                    ObservedType::Int32,
                );
            }
        }
        let feedback = feedback.snapshot(410);
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
                    FrameInlineCallee {
                        artifact,
                        target: feedback.call_link_at(caller_key, pc).unwrap(),
                        body: body.clone(),
                        children: Default::default(),
                    },
                )
            })
            .collect();
        (caller, callees, feedback)
    }

    fn translate(caller_source: &str, callee_source: &str) -> OptimizedIr {
        let (caller, callees, _) = inputs(caller_source, callee_source);
        OptimizedIr::translate_with_frame_inline_callees(
            &caller,
            410,
            &Default::default(),
            &Default::default(),
            &callees,
        )
        .unwrap()
    }

    #[test]
    fn frame_inline_continuation_maps_are_post_call_and_keep_effects() {
        let ir = translate(
            "(function(f,o,n){return n+f(o,n)})",
            "(function(o,n){o.x=n;return n+1})",
        );
        let call_node = ir
            .nodes()
            .iter()
            .find(|node| {
                ir.scalar_graph()
                    .call(node.id())
                    .is_some_and(|call| call.frame_state_node == node.id())
            })
            .unwrap();
        assert_eq!(call_node.effect(), OptimizedEffect::Reentrant);
        assert_eq!(
            ir.scalar_graph().inlined_calls(),
            0,
            "effectful calls must not use pre-CALL replay regions"
        );
        let post = ir
            .guard_maps()
            .iter()
            .find(|site| matches!(site.map().phase(), DeoptPhase::AfterEffect(_)))
            .expect("effectful inline recovery needs an exact post-CALL map");
        assert_eq!(
            post.map().resume_pc(),
            call_node.pc() + call_node.bytes().len() as u32
        );
        assert_eq!(
            post.shape().stack(),
            2,
            "preserve the pending addition's prefix plus one call result"
        );
        assert_ne!(Some(post.guard()), call_node.deopt_guard());
        assert!(post
            .map()
            .validate_identity_materialization(post.shape())
            .is_ok());
        assert_eq!(ir.scalar_graph().frame_inlined_calls(), 1);
        assert!(!ir.scalar_graph().permits_amortized_poll(ir.nodes(), &[]));
        let region = ir
            .scalar_graph()
            .call(call_node.id())
            .unwrap()
            .frame_inline
            .as_ref()
            .unwrap();
        assert_eq!(region.continuation.guard, post.guard());
        assert!(region
            .body
            .blocks
            .iter()
            .flat_map(|block| &block.instructions)
            .any(|instruction| matches!(instruction.op, super::super::IrOp::SetProperty(_))));
        for boundary in &region.boundaries {
            assert!(boundary.next_pc > boundary.pc);
        }
    }

    #[test]
    fn no_native_prefix_is_not_admitted_as_frame_inlining() {
        let ir = translate(
            "(function(f,o,n){return n+f(o,n)})",
            "(function(o,n){return globalThis.value+n})",
        );
        assert_eq!(
            ir.scalar_graph().frame_inlined_calls(),
            0,
            "an unsupported first operation would only enter and resume at PC zero"
        );
    }

    #[test]
    fn method_continuation_consumes_receiver_and_preserves_prefix() {
        let ir = translate(
            "(function(o,n){return n+o.f(o,n)})",
            "(function(o,n){o.x=n;return n+1})",
        );
        let regions: Vec<_> = ir
            .scalar_graph()
            .values()
            .iter()
            .filter_map(|value| {
                let super::super::ScalarValue::Call(call) = value else {
                    return None;
                };
                call.frame_inline.as_ref()
            })
            .collect();
        assert_eq!(regions.len(), 1);
        assert_eq!(regions[0].call_kind, InlineCallKind::Method);
        assert_eq!(
            regions[0].call_kind.abi_kind(),
            rquickjs_core::qjs::JS_JIT_INLINE_CALL_METHOD
        );
        assert_eq!(regions[0].continuation.shape.stack(), 2);
    }

    #[test]
    fn branch_and_local_callees_are_frame_inlined() {
        for callee in [
            "(function(o,n){let x=n+1;if(n>0)o.x=x;else o.x=n;return x})",
            "(function(o,n){let x=n+1;let y=x+1;o.x=y;return y})",
        ] {
            let ir = translate("(function(f,o,n){return n+f(o,n)})", callee);
            assert_eq!(
                ir.scalar_graph().frame_inlined_calls(),
                1,
                "general branches and locals must retain an exact frame: {callee}"
            );
            let region = ir
                .scalar_graph()
                .values()
                .iter()
                .find_map(|value| match value {
                    super::super::ScalarValue::Call(call) => call.frame_inline.as_deref(),
                    _ => None,
                })
                .unwrap();
            assert!(region.body.local_count > 0);
            assert!(region.body.blocks.len() > 1 || callee.contains("let y"));
        }
    }

    #[test]
    fn nested_frame_regions_retain_the_inner_call_tree() {
        let (caller, mut callees, root_feedback) = inputs(
            "(function(f,o,n){return 1+f(o,n)})",
            "(function(o,n){let x=o.f(o,n);return x+1})",
        );
        let inner_fixture = SnapshotFixture::compile("(function(o,n){o.x=n;return n+1})");
        let inner_body = inner_fixture
            .snapshot()
            .verify(VerifyLimits::default())
            .unwrap();
        let outer = callees.values_mut().next().unwrap();
        let outer_key = outer.target.callee();
        let inner_key = FunctionKey::new(
            inner_body.snapshot().function_id(),
            inner_body.snapshot().generation(),
        );
        let inner_pc = outer
            .body
            .instructions()
            .iter()
            .find(|instruction| instruction.opcode().name().contains("call"))
            .unwrap()
            .pc();
        let mut feedback = FeedbackTable::new(64, 2);
        for _ in 0..32 {
            feedback.observe_call_signature_with_identity(
                outer_key,
                inner_pc,
                inner_key,
                0x32345678,
                0x42345678,
                &[ObservedType::Object, ObservedType::Int32],
                ObservedType::Int32,
            );
        }
        let feedback = feedback.snapshot(411);
        let mut artifact = CompiledArtifact::fake(Tier::Baseline).key();
        artifact.function_id = inner_key.id;
        artifact.generation = inner_key.generation;
        artifact.source_revision = inner_body.snapshot().source_revision();
        artifact.opcode_fingerprint = inner_body.snapshot().opcode_fingerprint();
        outer.children.insert(
            inner_pc,
            FrameInlineCallee {
                artifact,
                target: feedback.call_link_at(outer_key, inner_pc).unwrap(),
                body: inner_body,
                children: Default::default(),
            },
        );
        let ir = OptimizedIr::translate_with_frame_inline_callees(
            &caller,
            411,
            &Default::default(),
            &Default::default(),
            &callees,
        )
        .unwrap();
        let region = ir
            .scalar_graph()
            .values()
            .iter()
            .find_map(|value| match value {
                super::super::ScalarValue::Call(call) => call.frame_inline.as_deref(),
                _ => None,
            })
            .unwrap();
        assert_eq!(region.children.len(), 1);
        assert_eq!(
            region.children[&inner_pc].artifact.function_id,
            inner_key.id
        );
        assert_eq!(region.total_regions(), 2);
        let (&call_pc, callee) = callees.iter().next().unwrap();
        let caller_key = FunctionKey::new(
            caller.snapshot().function_id(),
            caller.snapshot().generation(),
        );
        let lowered = crate::compiler::optimized::Tier2Compiler::host(411)
            .lower_with_frame_inline_callee_for_test(
                &caller,
                caller_key,
                &root_feedback,
                call_pc,
                callee.clone(),
            );
        assert!(lowered.is_ok(), "nested frame tree must lower: {lowered:?}");
    }

    #[test]
    fn branch_and_local_frame_regions_lower_to_machine_ir() {
        let (caller, callees, feedback) = inputs(
            "(function(f,o,n){return n+f(o,n)})",
            "(function(o,n){let x=n+1;if(n>0)o.x=x;else o.x=n;return x})",
        );
        let (&call_pc, callee) = callees.iter().next().unwrap();
        let key = FunctionKey::new(
            caller.snapshot().function_id(),
            caller.snapshot().generation(),
        );
        let result = crate::compiler::optimized::Tier2Compiler::host(410)
            .lower_with_frame_inline_callee_for_test(
                &caller,
                key,
                &feedback,
                call_pc,
                callee.clone(),
            );
        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn tail_calls_loops_and_exception_callees_keep_ordinary_calls() {
        for (caller, callee) in [
            (
                "(function(f,o,n){return f(o,n)})",
                "(function(o,n){o.x=n;return n+1})",
            ),
            (
                "(function(f,o,n){return n+f(o,n)})",
                "(function(o,n){for(;;)o.x=n})",
            ),
            (
                "(function(f,o,n){return n+f(o,n)})",
                "(function(o,n){try{o.x=n;return n+1}catch(e){return 0}})",
            ),
        ] {
            let ir = translate(caller, callee);
            assert_eq!(ir.scalar_graph().frame_inlined_calls(), 0, "{callee}");
            assert!(!ir
                .guard_maps()
                .iter()
                .any(|site| matches!(site.map().phase(), DeoptPhase::AfterEffect(_))));
        }
    }

    #[test]
    fn each_call_owns_a_distinct_post_effect_continuation() {
        let ir = translate(
            "(function(f,o,n){let a=f(o,n);return a+f(o,n)})",
            "(function(o,n){o.x=n;return n+1})",
        );
        let post: Vec<_> = ir
            .guard_maps()
            .iter()
            .filter(|site| matches!(site.map().phase(), DeoptPhase::AfterEffect(_)))
            .collect();
        assert_eq!(post.len(), 2);
        assert_ne!(post[0].guard(), post[1].guard());
        assert_ne!(post[0].map().resume_pc(), post[1].map().resume_pc());
        assert_eq!(post[0].shape().stack(), 1);
        assert_eq!(post[1].shape().stack(), 2);
    }

    #[test]
    fn frame_body_budget_keeps_remaining_calls_and_charges_retained_storage() {
        let mut source = String::from("(function(f,o,n){");
        for _ in 0..64 {
            source.push_str("f(o,n);");
        }
        source.push_str("return n})");
        let (caller, callees, _) = inputs(&source, "(function(o,n){o.x=n;return n+1})");
        let plain = OptimizedIr::translate(&caller, 410).unwrap();
        let ir = OptimizedIr::translate_with_frame_inline_callees(
            &caller,
            410,
            &Default::default(),
            &Default::default(),
            &callees,
        )
        .unwrap();
        let admitted = ir.scalar_graph().frame_inlined_calls();
        assert!(admitted > 0 && admitted < 64, "{admitted}");
        let extra = ir.scalar_graph().allocated_bytes() - plain.scalar_graph().allocated_bytes();
        assert!(extra > 0 && extra <= FRAME_INLINE_BUDGET, "{extra}");
        assert_eq!(
            ir.guard_maps()
                .iter()
                .filter(|site| matches!(site.map().phase(), DeoptPhase::AfterEffect(_)))
                .count() as u64,
            admitted
        );
    }

    #[test]
    fn stale_source_and_oversized_frames_fail_before_inline_allocation() {
        let (caller, mut callees, _) = inputs(
            "(function(f,o,n){return n+f(o,n)})",
            "(function(o,n){o.x=n;return n+1})",
        );
        for candidate in callees.values_mut() {
            candidate.artifact.source_revision ^= 1;
        }
        let ir = OptimizedIr::translate_with_frame_inline_callees(
            &caller,
            410,
            &Default::default(),
            &Default::default(),
            &callees,
        )
        .unwrap();
        assert_eq!(ir.scalar_graph().frame_inlined_calls(), 0);
        let mut callee = String::from("(function(o,n");
        for index in 0..64 {
            callee.push_str(&format!(",a{index}"));
        }
        callee.push_str("){o.x=n;return n+1})");
        let ir = translate("(function(f,o,n){return n+f(o,n)})", &callee);
        assert_eq!(ir.scalar_graph().frame_inlined_calls(), 0);
    }
}
