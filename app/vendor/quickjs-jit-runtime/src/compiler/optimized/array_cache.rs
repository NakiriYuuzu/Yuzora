//! Profile-selected, lazily guarded array metadata carried across SSA blocks.
//! A plan authorizes a guard, never a heap fact. Metadata is invalidated before
//! every operation that can reenter, finalize a reference or alter storage.

use crate::ir::{
    FrameSlot, IntegerRangeAnalysis, KnownFacts, LoopAnalysis, OptimizedIr, ScalarHeapEffect,
    ScalarHeapOperation, ScalarValueId,
};
use crate::runtime::{ArrayAccess, ArrayFeedbackSnapshot, ArrayMode};

/// The frame slot whose current value is the guarded receiver.
///
/// Lowering keys cached metadata by the provenance of the stack operand, so a
/// receiver is identified by the one frame slot that the operand was read
/// from. An entry argument uses the SSA identity proof. A local receiver
/// (for example `const ints = buffers.ints`) is admitted only when it is the
/// unique frame slot holding the operand's SSA value at every planned access,
/// and when no node of the enclosing natural loop writes that local.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ArrayReceiver {
    Argument(u16),
    Local(u16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ArrayCandidate {
    pub receiver: ArrayReceiver,
    pub mode: ArrayMode,
    /// Typed length requires the metadata query to guard observable lookup.
    pub needs_length: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ArrayAccessPlan {
    pub candidate: usize,
    pub access: ArrayAccess,
    pub index: Option<ScalarValueId>,
    pub result: Option<ScalarValueId>,
    /// Lowering must check an Int32 key, non-negativity, and unsigned
    /// `index < dense_count` before using cached storage. The plan never turns
    /// an out-of-bounds access into `undefined` by itself.
    pub requires_index_guard: bool,
    /// This access is dominated by a packed-array metadata guard in the
    /// natural-loop preheader, and integer range analysis proved its key is
    /// below that guard's exact logical length. Lowering may omit only the
    /// per-access count comparison; tag, hole/value and address checks remain.
    pub bounds_covered_by_hoist: bool,
    pub loop_header: u32,
}

/// Metadata that may be materialized exactly once on the loop-entry edge.
///
/// Packed arrays require `logical_length == dense_count` in this guard. That
/// equality makes the SSA `bound` used by range analysis an exact storage
/// bound and rejects sparse/extended arrays before entering native loop code.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ArrayLoopHoist {
    pub candidate: usize,
    pub preheader: u32,
    pub header: u32,
    pub bound: ScalarValueId,
    pub length_node: u32,
}

/// Typed-array storage metadata (mode, fixed attached backing, count and data
/// pointer, but not the observable `length` lookup) guarded once on the
/// loop-entry edge. Accesses keep their per-access index guards; the hoist only
/// removes the repeated leaf query for a loop-invariant receiver, like JSC's
/// hoisted CheckStructure/GetButterfly for typed-array views.
///
/// Because the preheader runs even when no access does (a zero-trip loop),
/// lowering emits this query without a side exit: a miss publishes an empty
/// view (`count == 0`), so each retained index guard exits at the access that
/// actually runs, exactly like the unhoisted query, and poll revalidation is
/// skipped while the view is empty.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ArrayStorageHoist {
    pub candidate: usize,
    pub preheader: u32,
    pub header: u32,
}

#[derive(Debug, Default)]
pub(super) struct ArrayPlan {
    candidates: Vec<ArrayCandidate>,
    accesses: Vec<Option<ArrayAccessPlan>>,
    hoists: Vec<ArrayLoopHoist>,
    storage_hoists: Vec<ArrayStorageHoist>,
    /// Loop headers whose hoisted tuples a poll node must revalidate.
    poll_revalidations: Vec<(u32, Vec<u32>)>,
    invalidations: Vec<bool>,
}

impl ArrayPlan {
    const MAX_CANDIDATES: usize = 16;
    /// Metadata tuples lowering may keep live at once. Every loop header seeds
    /// at most this many hoisted sources.
    pub(super) const MAX_LIVE_SOURCES: usize = 4;

    #[allow(clippy::too_many_arguments)]
    pub(super) fn analyze(
        ir: &OptimizedIr,
        facts: &KnownFacts,
        loops: &LoopAnalysis,
        feedback_len: usize,
        feedback_at: impl Fn(u32) -> Option<ArrayFeedbackSnapshot>,
        typed_length_guard: bool,
        non_reentrant: impl Fn(u32) -> bool,
        mut work: usize,
    ) -> Self {
        if !loops.is_complete() {
            return Self::default();
        }
        let graph = ir.scalar_graph();
        let nodes = ir.nodes();
        // Charge all output-sized allocations before making them. Partial
        // analysis is never published when the caller's finite budget runs out.
        let Some(initial_cost) = nodes
            .len()
            .checked_mul(8)
            .and_then(|cost| {
                feedback_len
                    .checked_mul(4)
                    .and_then(|feedback| cost.checked_add(feedback))
            })
            .and_then(|cost| cost.checked_add(64))
        else {
            return Self::default();
        };
        if spend(&mut work, initial_cost).is_none() {
            return Self::default();
        }

        #[derive(Clone, Copy)]
        struct Proposed {
            node: u32,
            receiver: ArrayReceiver,
            mode: ArrayMode,
            access: ArrayAccess,
            index: Option<ScalarValueId>,
            result: Option<ScalarValueId>,
            loop_header: u32,
        }

        let mut proposed = Vec::with_capacity(nodes.len().min(work));
        for node in nodes {
            if spend(&mut work, 1).is_none() {
                return Self::default();
            }
            let Some(operation) = graph.heap_operation(node.id()).copied() else {
                continue;
            };
            let (base, access, index, result) = match operation {
                ScalarHeapOperation::GetElement {
                    base,
                    index,
                    result,
                    ..
                } => (base, ArrayAccess::Load, Some(index), Some(result)),
                ScalarHeapOperation::PutElement { base, index, .. } => {
                    (base, ArrayAccess::Store, Some(index), None)
                }
                ScalarHeapOperation::GetProperty {
                    base, atom, result, ..
                } if atom == rquickjs_core::qjs::JS_ATOM_length => {
                    (base, ArrayAccess::Length, None, Some(result))
                }
                _ => continue,
            };
            let Some(site) = feedback_at(node.pc()) else {
                continue;
            };
            if site.access() != access || !site.can_specialize() {
                continue;
            }
            let mut modes = site.modes();
            let Some(mode) = modes.next() else { continue };
            if modes.next().is_some() || mode == ArrayMode::Generic {
                continue;
            }
            // Only the typed store lowering has the leaf contract: an exact
            // fixed, attached, nonshared writable view, in-bounds Int32 key,
            // and numeric value whose conversion cannot run script. Packed
            // stores may replace an owner or grow storage and stay generic.
            if access == ArrayAccess::Store
                && !matches!(mode, ArrayMode::Int32 | ArrayMode::Float64)
            {
                continue;
            }
            if access == ArrayAccess::Length && mode != ArrayMode::Packed && !typed_length_guard {
                continue;
            }
            let receiver = if let Some(argument) = facts.entry_argument(base) {
                ArrayReceiver::Argument(argument)
            } else if let Some(local) = unique_local_receiver(graph, facts, operation, base) {
                ArrayReceiver::Local(local)
            } else {
                continue;
            };
            let Some(block) = loops.block_for_node(node.id()) else {
                continue;
            };
            let containing_loop = loops
                .loops()
                .iter()
                .filter(|natural_loop| natural_loop.contains_block(block))
                .min_by_key(|natural_loop| natural_loop.members().len());
            // A canonical traversal often snapshots `a.length` immediately
            // before the loop. Associate that dominating read with the loop;
            // element accesses themselves must still be inside it.
            // A typed snapshot read in an earlier block is not the same SSA
            // value as the count returned by the later leaf query, so it must
            // not authorize bounds-check elimination. Packed Array length is
            // the engine-owned, non-overridable logical length and its guard
            // additionally proves logical_length == dense_count.
            let dominating_loop = (access == ArrayAccess::Length && mode == ArrayMode::Packed)
                .then(|| {
                    loops
                        .loops()
                        .iter()
                        .filter(|natural_loop| {
                            natural_loop
                                .preheader()
                                .is_some_and(|preheader| loops.dominates(block, preheader))
                        })
                        .min_by_key(|natural_loop| natural_loop.members().len())
                })
                .flatten();
            let loop_header = if let Some(natural_loop) = containing_loop.or(dominating_loop) {
                natural_loop.header()
            } else if access != ArrayAccess::Length {
                // A standalone load still has a concrete guarded lowering and
                // must be represented in the final capability set. It cannot
                // hoist metadata or eliminate its index guard.
                block
            } else {
                continue;
            };
            proposed.push(Proposed {
                node: node.id(),
                receiver,
                mode,
                access,
                index,
                result,
                loop_header,
            });
        }

        // Mixed modes for one entry identity cannot share one metadata tuple.
        // Drop that identity as a unit rather than publishing a partial plan.
        let Some(conflict_work) = proposed.len().checked_mul(proposed.len()) else {
            return Self::default();
        };
        if spend(&mut work, conflict_work).is_none() {
            return Self::default();
        }
        let mut conflicting_receivers = Vec::new();
        for left in &proposed {
            if proposed
                .iter()
                .any(|right| right.receiver == left.receiver && right.mode != left.mode)
                && !conflicting_receivers.contains(&left.receiver)
            {
                conflicting_receivers.push(left.receiver);
            }
        }
        proposed.retain(|access| !conflicting_receivers.contains(&access.receiver));

        // A reentrant operation in the natural loop can replace storage, detach
        // a buffer, or run arbitrary script. Feedback never authorizes carrying
        // metadata through it. A lowering may explicitly certify a node only
        // when its emitted guarded path is non-reentrant.
        let Some(effect_work) = proposed.len().checked_mul(nodes.len()) else {
            return Self::default();
        };
        if spend(&mut work, effect_work).is_none() {
            return Self::default();
        }
        let mut proposed_nodes = vec![false; nodes.len()];
        for access in &proposed {
            proposed_nodes[access.node as usize] = true;
        }
        proposed.retain(|candidate| {
            let Some(block) = loops.block_for_node(candidate.node) else {
                return false;
            };
            let Some(natural_loop) = loops
                .loops()
                .iter()
                .find(|natural_loop| natural_loop.header() == candidate.loop_header)
            else {
                return candidate.access != ArrayAccess::Length && candidate.loop_header == block;
            };
            if candidate.access != ArrayAccess::Length && !natural_loop.contains_block(block) {
                return false;
            }
            if candidate.access == ArrayAccess::Length
                && !natural_loop.contains_block(block)
                && !natural_loop
                    .preheader()
                    .is_some_and(|preheader| loops.dominates(block, preheader))
            {
                return false;
            }
            for node in nodes {
                if !loops
                    .block_for_node(node.id())
                    .is_some_and(|owner| natural_loop.contains_block(owner))
                {
                    continue;
                }
                // A local receiver is loop-invariant only when no loop node can
                // rebind it. The metadata cache is keyed by that frame slot.
                if let ArrayReceiver::Local(local) = candidate.receiver {
                    if graph.effect_for_node(node.id())
                        == ScalarHeapEffect::FrameWrite(FrameSlot::Local(local))
                    {
                        return false;
                    }
                }
                // Lowering revalidates hoisted tuples only at loop polls
                // (every such poll of an enclosing loop, see
                // `revalidating_headers`). Any other safepoint could observe
                // an interrupt without that revalidation.
                if graph.effect_for_node(node.id()) == ScalarHeapEffect::Safepoint
                    && !matches!(
                        node.kind(),
                        crate::ir::OptimizedNodeKind::GuardNumeric { mid_loop: true, .. }
                    )
                {
                    return false;
                }
                if graph.effect_for_node(node.id()) != ScalarHeapEffect::Reentrant
                    || non_reentrant(node.id())
                    || proposed_nodes[node.id() as usize]
                {
                    continue;
                }
                return false;
            }
            true
        });

        let mut candidates: Vec<ArrayCandidate> = Vec::new();
        let mut accesses: Vec<Option<ArrayAccessPlan>> = vec![None; nodes.len()];
        for proposed in proposed {
            let candidate = if let Some(index) = candidates.iter().position(|candidate| {
                candidate.receiver == proposed.receiver && candidate.mode == proposed.mode
            }) {
                index
            } else {
                if candidates.len() == Self::MAX_CANDIDATES {
                    return Self::default();
                }
                candidates.push(ArrayCandidate {
                    receiver: proposed.receiver,
                    mode: proposed.mode,
                    needs_length: false,
                });
                candidates.len() - 1
            };
            if proposed.access == ArrayAccess::Length {
                candidates[candidate].needs_length = true;
            }
            accesses[proposed.node as usize] = Some(ArrayAccessPlan {
                candidate,
                access: proposed.access,
                index: proposed.index,
                result: proposed.result,
                requires_index_guard: proposed.access != ArrayAccess::Length,
                bounds_covered_by_hoist: false,
                loop_header: proposed.loop_header,
            });
        }

        // JSC's integer-range phase only removes a check when the compared SSA
        // bound is the same value guarded at the storage boundary. For packed
        // arrays we establish that identity by guarding logical_length ==
        // dense_count. For typed arrays the versioned leaf API additionally
        // guards the exact intrinsic getter and fixed backing before returning
        // the same live count; `typed_length_guard` is the capability bit that
        // makes their length access eligible above.
        let ranges = IntegerRangeAnalysis::analyze_with_guarded_heap(
            graph,
            ir.blocks(),
            nodes,
            true,
            |id| accesses.get(id as usize).is_some_and(Option::is_some) || non_reentrant(id),
            work,
        );
        let mut hoists = Vec::new();
        for candidate in 0..candidates.len() {
            for natural_loop in loops.loops() {
                let Some(preheader) = natural_loop.preheader() else {
                    continue;
                };
                let mut lengths = accesses.iter().enumerate().filter_map(|(node, access)| {
                    let access = access.as_ref()?;
                    (access.candidate == candidate
                        && access.loop_header == natural_loop.header()
                        && access.access == ArrayAccess::Length)
                        .then_some((node as u32, access.result?))
                });
                let Some((length_node, bound)) = lengths.next() else {
                    continue;
                };
                // More than one observable length read does not identify one
                // exact bound for every load. Keep all checks in that case.
                if lengths.next().is_some() {
                    continue;
                }
                // Check the live-tuple cap before marking any access as
                // covered: a capped loop keeps every per-access index guard.
                if hoists
                    .iter()
                    .filter(|hoist: &&ArrayLoopHoist| hoist.header == natural_loop.header())
                    .count()
                    >= Self::MAX_LIVE_SOURCES
                {
                    continue;
                }
                let mut covered_any = false;
                for (node, access) in accesses.iter_mut().enumerate() {
                    let Some(access) = access.as_mut() else {
                        continue;
                    };
                    if access.candidate != candidate
                        || access.loop_header != natural_loop.header()
                        || access.access != ArrayAccess::Load
                    {
                        continue;
                    }
                    let Some(index) = access.index else { continue };
                    if ranges.proves_in_bounds_at(index, bound, node as u32) {
                        access.requires_index_guard = false;
                        access.bounds_covered_by_hoist = true;
                        covered_any = true;
                    }
                }
                if covered_any {
                    hoists.push(ArrayLoopHoist {
                        candidate,
                        preheader,
                        header: natural_loop.header(),
                        bound,
                        length_node,
                    });
                }
            }
        }
        // A loop-invariant typed receiver without a hoisted length (for
        // example the destination of `floats[i] = ...`) still needs only one
        // storage query per loop entry. The admission above already rejected
        // loops containing a write to the receiver's frame slot or any
        // uncertified reentrant node; polls revalidate the tuple.
        let mut storage_hoists = Vec::new();
        for (candidate, typed) in candidates.iter().enumerate() {
            if !matches!(typed.mode, ArrayMode::Int32 | ArrayMode::Float64) {
                continue;
            }
            for natural_loop in loops.loops() {
                let Some(preheader) = natural_loop.preheader() else {
                    continue;
                };
                let header = natural_loop.header();
                if hoists
                    .iter()
                    .any(|hoist| hoist.candidate == candidate && hoist.header == header)
                {
                    continue;
                }
                // Guarding the receiver on the loop-entry edge is profitable
                // only when every completed iteration performs one of its
                // accesses anyway. A conditionally used receiver (for example
                // `if (out) out[i] = v` with `out === null`) would otherwise
                // deopt on every loop entry although the loop never touches it.
                let latches: Vec<u32> = ir
                    .blocks()
                    .iter()
                    .filter(|block| {
                        natural_loop.contains_block(block.start_pc())
                            && block.successors().contains(&header)
                    })
                    .map(|block| block.start_pc())
                    .collect();
                let used = !latches.is_empty()
                    && accesses.iter().enumerate().any(|(node, access)| {
                        access.as_ref().is_some_and(|access| {
                            access.candidate == candidate
                                && access.loop_header == header
                                && access.access != ArrayAccess::Length
                                && loops.block_for_node(node as u32).is_some_and(|block| {
                                    latches.iter().all(|&latch| loops.dominates(block, latch))
                                })
                        })
                    });
                let live = hoists.iter().filter(|hoist| hoist.header == header).count()
                    + storage_hoists
                        .iter()
                        .filter(|hoist: &&ArrayStorageHoist| hoist.header == header)
                        .count();
                if used && live < Self::MAX_LIVE_SOURCES {
                    storage_hoists.push(ArrayStorageHoist {
                        candidate,
                        preheader,
                        header,
                    });
                }
            }
        }
        // Every loop poll that can run the interrupt handler must revalidate
        // the hoisted tuples of each loop enclosing it, not only those of its
        // own header: an inner loop's poll can detach an outer receiver's
        // buffer, and the outer header re-seeds its preheader tuple on every
        // backedge while its own amortized countdown rarely expires.
        let mut poll_revalidations = Vec::new();
        if !hoists.is_empty() || !storage_hoists.is_empty() {
            for node in nodes {
                if graph.effect_for_node(node.id()) != ScalarHeapEffect::Safepoint {
                    continue;
                }
                let Some(block) = loops.block_for_node(node.id()) else {
                    continue;
                };
                let mut headers = Vec::new();
                for natural_loop in loops.loops() {
                    let header = natural_loop.header();
                    if natural_loop.contains_block(block)
                        && (hoists.iter().any(|hoist| hoist.header == header)
                            || storage_hoists.iter().any(|hoist| hoist.header == header))
                        && !headers.contains(&header)
                    {
                        headers.push(header);
                    }
                }
                if !headers.is_empty() {
                    poll_revalidations.push((node.id(), headers));
                }
            }
        }
        let mut invalidations = Vec::with_capacity(nodes.len());
        for node in nodes {
            let planned = accesses
                .get(node.id() as usize)
                .is_some_and(Option::is_some);
            let invalidates = match graph.effect_for_node(node.id()) {
                ScalarHeapEffect::Pure => false,
                // Planned operations have a lowering contract: loads retain
                // metadata only after all mode/index/storage guards succeed;
                // packed logical length never replaces the dense count.
                ScalarHeapEffect::Reentrant if planned => false,
                // Consumers may certify another emitted guarded fast path
                // (for example checked Int32 arithmetic). The slow/miss edge
                // deopts before returning to the cached path.
                ScalarHeapEffect::Reentrant if non_reentrant(node.id()) => false,
                // Local identity is part of the metadata-cache key. Even a
                // primitive/unowned move can rebind that local to a different
                // entry array, so every frame write is a provenance barrier.
                ScalarHeapEffect::FrameWrite(_) => true,
                // Even a non-reentrant store can grow/reallocate dense storage,
                // so writes and every other unknown boundary invalidate.
                _ => true,
            };
            invalidations.push(invalidates);
        }
        Self {
            candidates,
            accesses,
            hoists,
            storage_hoists,
            poll_revalidations,
            invalidations,
        }
    }

    pub(super) fn candidates(&self) -> &[ArrayCandidate] {
        &self.candidates
    }
    pub(super) fn is_empty(&self) -> bool {
        self.candidates.is_empty()
    }
    pub(super) fn access(&self, node: u32) -> Option<&ArrayAccessPlan> {
        self.accesses.get(node as usize)?.as_ref()
    }

    pub(super) fn guarded_leaf(&self, node: u32) -> bool {
        self.access(node).is_some_and(|access| match access.access {
            ArrayAccess::Load | ArrayAccess::Store => true,
            ArrayAccess::Length => self.hoists.iter().any(|hoist| hoist.length_node == node),
        })
    }
    pub(super) fn hoists(&self) -> &[ArrayLoopHoist] {
        &self.hoists
    }
    pub(super) fn storage_hoists(&self) -> &[ArrayStorageHoist] {
        &self.storage_hoists
    }
    /// Headers of every loop containing poll `node` that owns hoisted
    /// metadata. The poll's observable (runtime-calling) path must revalidate
    /// all of their tuples before native code continues.
    pub(super) fn revalidating_headers(&self, node: u32) -> &[u32] {
        self.poll_revalidations
            .iter()
            .find(|(poll, _)| *poll == node)
            .map_or(&[], |(_, headers)| headers.as_slice())
    }
    /// Hoisted tuples (length and storage-only) seeded at `header`.
    pub(super) fn hoisted_sources_at(&self, header: u32) -> usize {
        self.hoists
            .iter()
            .filter(|hoist| hoist.header == header)
            .count()
            + self
                .storage_hoists
                .iter()
                .filter(|hoist| hoist.header == header)
                .count()
    }
    pub(super) fn invalidates_before(&self, node: u32) -> bool {
        self.invalidations
            .get(node as usize)
            .copied()
            .unwrap_or(true)
    }

    /// Conservative retained-plus-temporary allocation bound for compilation
    /// control. Work accounting independently bounds all quadratic scans.
    pub(super) fn bytes_upper_bound(ir: &OptimizedIr) -> usize {
        ir.nodes()
            .len()
            .saturating_mul(160)
            .saturating_add(Self::MAX_CANDIDATES.saturating_mul(64))
    }
}

/// The single local that holds `base` in the operation's pre-effect frame
/// state. Any other frame slot holding the same SSA value makes the stack
/// operand's lowering provenance ambiguous, so the site is left unplanned.
/// Lowering independently requires the operand provenance to be this local.
fn unique_local_receiver(
    graph: &crate::ir::ScalarGraph,
    facts: &KnownFacts,
    operation: ScalarHeapOperation,
    base: ScalarValueId,
) -> Option<u16> {
    let state = graph.frame_state_for_node(operation.frame_state_node())?;
    if state
        .arguments
        .iter()
        .any(|&value| facts.same_value(value, base))
    {
        return None;
    }
    let mut holders = state
        .locals
        .iter()
        .enumerate()
        .filter(|(_, &value)| facts.same_value(value, base));
    let (local, _) = holders.next()?;
    if holders.next().is_some() {
        return None;
    }
    u16::try_from(local).ok()
}

fn spend(work: &mut usize, cost: usize) -> Option<()> {
    *work = work.checked_sub(cost)?;
    Some(())
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::{
        bytecode::VerifyLimits,
        ir::{OptimizedNodeKind, ScalarHeapOperation, ScalarNumericMode},
        runtime::{ArrayHazards, FeedbackSnapshot, FeedbackTable, FunctionKey},
        test_support::SnapshotFixture,
    };

    fn fixture(source: &str, mode: ArrayMode) -> (OptimizedIr, FeedbackSnapshot) {
        let compiled = SnapshotFixture::compile(source);
        let verified = compiled.snapshot().verify(VerifyLimits::default()).unwrap();
        let numeric_modes = verified
            .instructions()
            .iter()
            .filter(|instruction| {
                matches!(
                    instruction.opcode().name(),
                    "add"
                        | "sub"
                        | "mul"
                        | "div"
                        | "or"
                        | "lt"
                        | "lte"
                        | "gt"
                        | "gte"
                        | "inc"
                        | "dec"
                        | "post_inc"
                        | "post_dec"
                        | "inc_loc"
                        | "dec_loc"
                )
            })
            .map(|instruction| (instruction.pc(), ScalarNumericMode::Int32))
            .collect();
        let ir = OptimizedIr::translate_with_numeric_modes(&verified, 900, &numeric_modes).unwrap();
        let mut feedback = FeedbackTable::new(100, 3);
        for node in ir.nodes() {
            let access = match ir.scalar_graph().heap_operation(node.id()) {
                Some(ScalarHeapOperation::GetElement { .. }) => ArrayAccess::Load,
                Some(ScalarHeapOperation::PutElement { .. }) => ArrayAccess::Store,
                Some(ScalarHeapOperation::GetProperty { atom, .. })
                    if *atom == rquickjs_core::qjs::JS_ATOM_length =>
                {
                    ArrayAccess::Length
                }
                _ => continue,
            };
            feedback.observe_array(
                FunctionKey::new(900, 1),
                node.pc(),
                access,
                mode,
                ArrayHazards::NONE,
            );
        }
        (ir, feedback.snapshot(1))
    }

    fn plan(
        ir: &OptimizedIr,
        feedback: &FeedbackSnapshot,
        typed_length: bool,
        work: usize,
    ) -> ArrayPlan {
        let facts = KnownFacts::analyze_with_preserved_frame_reads(
            ir.scalar_graph(),
            ir.nodes(),
            |id| {
                ir.scalar_graph().heap_operation(id).is_some()
                    || matches!(ir.nodes()[id as usize].kind(),
                        OptimizedNodeKind::Bytecode { opcode } if opcode.as_ref() == "to_propkey")
                    || matches!(
                        ir.nodes()[id as usize].kind(),
                        OptimizedNodeKind::GuardNumeric { .. }
                    )
            },
            100_000,
        );
        let loops = LoopAnalysis::analyze(ir, 100_000);
        assert!(loops.is_complete());
        ArrayPlan::analyze(
            ir,
            &facts,
            &loops,
            feedback.arrays().len(),
            |pc| feedback.array_at(FunctionKey::new(900, 1), pc).copied(),
            typed_length,
            |id| {
                matches!(ir.nodes()[id as usize].kind(),
                OptimizedNodeKind::Bytecode { opcode } if opcode.as_ref() == "to_propkey")
            },
            work,
        )
    }

    #[test]
    fn typed_stores_publish_leaf_capabilities_but_packed_stores_do_not() {
        for mode in [ArrayMode::Int32, ArrayMode::Float64, ArrayMode::Packed] {
            let (ir, feedback) = fixture("(function(a,i,v){a[i]=v;return a[i]})", mode);
            let plan = plan(&ir, &feedback, true, 100_000);
            let store = ir
                .nodes()
                .iter()
                .find(|node| {
                    matches!(
                        ir.scalar_graph().heap_operation(node.id()),
                        Some(ScalarHeapOperation::PutElement { .. })
                    )
                })
                .unwrap();
            assert_eq!(plan.guarded_leaf(store.id()), mode != ArrayMode::Packed);
            if mode != ArrayMode::Packed {
                assert!(!plan.invalidates_before(store.id()));
                assert!(plan.access(store.id()).unwrap().requires_index_guard);
                assert!(ir
                    .nodes()
                    .iter()
                    .filter(|node| matches!(
                        ir.scalar_graph().heap_operation(node.id()),
                        Some(ScalarHeapOperation::GetElement { .. })
                    ))
                    .all(|node| plan.guarded_leaf(node.id())));
            }
        }
    }

    #[test]
    fn nested_counted_loop_uses_one_argument_identity_and_separate_length_access() {
        let (ir, feedback) = fixture("(function(a,n){let b=a,s=0;for(let j=0;j<n;j++){for(let i=0;i<b.length;i++){s+=b[i]}}return s})", ArrayMode::Packed);
        let plan = plan(&ir, &feedback, false, 100_000);
        assert_eq!(
            plan.candidates(),
            &[ArrayCandidate {
                receiver: ArrayReceiver::Argument(0),
                mode: ArrayMode::Packed,
                needs_length: true
            }]
        );
        let accesses: Vec<_> = ir
            .nodes()
            .iter()
            .filter_map(|n| plan.access(n.id()))
            .collect();
        assert_eq!(accesses.len(), 2);
        assert!(accesses
            .iter()
            .any(|a| a.access == ArrayAccess::Length && a.index.is_none()));
        assert!(accesses
            .iter()
            .any(|a| a.access == ArrayAccess::Load && a.index.is_some()));
        assert!(accesses
            .iter()
            .filter(|access| access.access == ArrayAccess::Load)
            .all(|access| access.requires_index_guard ^ access.bounds_covered_by_hoist));
        for node in ir.nodes() {
            if matches!(
                node.kind(),
                OptimizedNodeKind::GuardNumeric { mid_loop: true, .. }
            ) {
                assert!(plan.invalidates_before(node.id()));
            }
        }
    }

    #[test]
    fn unknown_reentry_and_changed_entry_argument_do_not_retain_receiver_identity() {
        let source = "(function(a,n,f){for(let i=0;i<n;i++){f.length;a[i]}return 0})";
        let compiled = SnapshotFixture::compile(source);
        let verified = compiled.snapshot().verify(VerifyLimits::default()).unwrap();
        let ir = OptimizedIr::translate(&verified, 900).unwrap();
        let mut feedback = FeedbackTable::new(100, 3);
        for node in ir.nodes() {
            if matches!(
                ir.scalar_graph().heap_operation(node.id()),
                Some(ScalarHeapOperation::GetElement { .. })
            ) {
                feedback.observe_array(
                    FunctionKey::new(900, 1),
                    node.pc(),
                    ArrayAccess::Load,
                    ArrayMode::Int32,
                    ArrayHazards::NONE,
                );
            }
        }
        assert!(plan(&ir, &feedback.snapshot(1), false, 100_000)
            .candidates()
            .is_empty());

        let source = "(function(a,n){for(let i=0;i<n;i++){a=i;a[i]}return 0})";
        let (ir, feedback) = fixture(source, ArrayMode::Int32);
        assert!(plan(&ir, &feedback, false, 100_000).candidates().is_empty());
    }

    #[test]
    fn typed_length_requires_lookup_capability_and_budget_exhaustion_is_empty() {
        let (ir, feedback) = fixture(
            "(function(a){let s=0;for(let i=0;i<a.length;i++)s+=a[i];return s})",
            ArrayMode::Float64,
        );
        let without = plan(&ir, &feedback, false, 100_000);
        assert!(ir.nodes().iter().all(|n| without
            .access(n.id())
            .is_none_or(|a| a.access != ArrayAccess::Length)));
        let with = plan(&ir, &feedback, true, 100_000);
        assert_eq!(with.candidates().len(), 1);
        assert!(with.candidates()[0].needs_length);
        let [hoist] = with.hoists() else {
            panic!("an intrinsic typed-length guard must authorize one loop hoist: {with:#?}");
        };
        assert_ne!(hoist.preheader, hoist.header);
        assert!(ir
            .nodes()
            .iter()
            .filter_map(|node| with.access(node.id()))
            .filter(|access| access.access == ArrayAccess::Load)
            .all(|access| access.bounds_covered_by_hoist && !access.requires_index_guard));
        for work in [0, 1, 16] {
            assert!(plan(&ir, &feedback, true, work).candidates().is_empty());
        }
    }

    #[test]
    fn packed_counted_loop_records_preheader_and_covers_only_proven_load_bounds() {
        let (ir, feedback) = fixture(
            "(function(a){let n=a.length,s=0;for(let i=0;i<n;i++)s=(s+a[i])|0;return s})",
            ArrayMode::Packed,
        );
        let plan = plan(&ir, &feedback, false, 100_000);
        let [hoist] = plan.hoists() else {
            panic!("canonical packed traversal must have one legal metadata hoist: {plan:#?}");
        };
        assert_ne!(hoist.preheader, hoist.header);
        let length = plan.access(hoist.length_node).unwrap();
        assert_eq!(length.access, ArrayAccess::Length);
        assert_eq!(length.result, Some(hoist.bound));
        let covered = ir
            .nodes()
            .iter()
            .filter_map(|node| plan.access(node.id()))
            .filter(|access| access.access == ArrayAccess::Load)
            .collect::<Vec<_>>();
        assert_eq!(covered.len(), 1);
        assert!(covered[0].bounds_covered_by_hoist);
        assert!(!covered[0].requires_index_guard);
    }

    #[test]
    fn wrong_range_shape_never_publishes_a_hoist() {
        for source in [
            "(function(a){let n=a.length,s=0;for(let i=-1;i<n;i++)s+=a[i];return s})",
            "(function(a){let n=a.length,s=0;for(let i=0;i>=n;i++)s+=a[i];return s})",
            "(function(a){let n=a.length,s=0;for(let i=0;i<n;i=(i+1)|0)s+=a[i];return s})",
            "(function(a,f){let n=a.length;f();let s=0;for(let i=0;i<n;i++)s+=a[i];return s})",
        ] {
            let (ir, feedback) = fixture(source, ArrayMode::Packed);
            let plan = plan(&ir, &feedback, false, 100_000);
            assert!(plan.hoists().is_empty(), "{source}");
            assert!(
                ir.nodes().iter().all(|node| {
                    plan.access(node.id()).is_none_or(|access| {
                        access.access != ArrayAccess::Length || !plan.guarded_leaf(node.id())
                    })
                }),
                "an unhoisted observable length became a leaf capability: {source}"
            );
        }
    }

    #[test]
    fn alias_writes_and_unknown_reentry_never_publish_stale_bounds() {
        // These are the two ways a loop-local metadata tuple can become stale:
        // an alias mutates dense storage directly, or arbitrary script does so
        // across a reentrant boundary.  As in JSC's clobberize model, either
        // effect must prevent the preheader guard from being consumed at all.
        for source in [
            "(function(a){let n=a.length,s=0,b=a;for(let i=0;i<n;i++){if(i===0)b.length=1;s=(s+(a[i]|0))|0}return s})",
            "(function(a,f){let n=a.length,s=0;for(let i=0;i<n;i++){if(i===0)f(a);s=(s+(a[i]|0))|0}return s})",
            "(function(a){let n=a.length;a.length=1;let s=0;for(let i=0;i<n;i++)s=(s+(a[i]|0))|0;return s})",
            "(function(a,f){let n=a.length;f(a);let s=0;for(let i=0;i<n;i++)s=(s+(a[i]|0))|0;return s})",
        ] {
            let (ir, feedback) = fixture(source, ArrayMode::Packed);
            let plan = plan(&ir, &feedback, false, 100_000);
            assert!(
                plan.hoists().is_empty(),
                "effectful loop published stale packed metadata: {source}; {plan:#?}"
            );
            assert!(
                ir.nodes()
                    .iter()
                    .filter_map(|node| plan.access(node.id()))
                    .filter(|access| access.access == ArrayAccess::Load)
                    .all(|access| access.requires_index_guard
                        && !access.bounds_covered_by_hoist),
                "effectful loop removed a per-access bounds guard: {source}; {plan:#?}"
            );
        }
    }

    #[test]
    fn loop_invariant_local_typed_receiver_is_a_hoisted_candidate() {
        let (ir, feedback) = fixture(
            "(function(o){const a=o.a;let s=0;for(let i=0;i<a.length;i++)s=(s+a[i])|0;return s})",
            ArrayMode::Int32,
        );
        let plan = plan(&ir, &feedback, true, 100_000);
        let [candidate] = plan.candidates() else {
            panic!("one local receiver candidate expected: {plan:#?}");
        };
        assert!(matches!(candidate.receiver, ArrayReceiver::Local(_)));
        assert!(candidate.needs_length);
        let [hoist] = plan.hoists() else {
            panic!("the local receiver's intrinsic length must be hoisted: {plan:#?}");
        };
        assert!(plan.guarded_leaf(hoist.length_node));
        assert!(plan.storage_hoists().is_empty());
    }

    #[test]
    fn rebound_or_aliased_local_receivers_are_not_candidates() {
        for source in [
            // Rebinding the receiver local inside the loop.
            "(function(o){let a=o.a,b=o.b,s=0;for(let i=0;i<a.length;i++){s=(s+a[i])|0;a=b}return s})",
            // Two locals hold the receiver: lowering provenance is ambiguous.
            "(function(o){const a=o.a;const b=a;let s=0;for(let i=0;i<a.length;i++)s=(s+b[i])|0;return s})",
            // An argument also holds the receiver.
            "(function(o){const a=o;let s=0;for(let i=0;i<a.length;i++)s=(s+a[i])|0;return s})",
        ] {
            let (ir, feedback) = fixture(source, ArrayMode::Int32);
            let plan = plan(&ir, &feedback, true, 100_000);
            assert!(
                plan.candidates()
                    .iter()
                    .all(|candidate| !matches!(candidate.receiver, ArrayReceiver::Local(_))),
                "{source}: {plan:#?}"
            );
        }
    }

    #[test]
    fn inner_loop_polls_revalidate_outer_hoisted_tuples() {
        let (ir, feedback) = fixture(
            "(function(o,m){const f=o.f;let t=0;for(let i=0;i<f.length;i++){f[i]=i;for(let j=0;j<m;j++)t=(t+1)|0}return t})",
            ArrayMode::Float64,
        );
        let plan = plan(&ir, &feedback, true, 100_000);
        // The store-only destination gets one storage hoist on the outer loop.
        assert!(plan.hoists().is_empty());
        let [hoist] = plan.storage_hoists() else {
            panic!("the outer storage hoist is expected: {plan:#?}");
        };
        let polls: Vec<_> = ir
            .nodes()
            .iter()
            .filter(|node| {
                matches!(
                    node.kind(),
                    OptimizedNodeKind::GuardNumeric { mid_loop: true, .. }
                )
            })
            .collect();
        assert_eq!(polls.len(), 2, "outer and inner loop polls expected");
        // Both the outer header poll and the nested inner poll must
        // revalidate the outer receiver's tuple.
        for poll in polls {
            assert!(
                plan.revalidating_headers(poll.id()).contains(&hoist.header),
                "poll {} does not revalidate the outer tuple: {plan:#?}",
                poll.id()
            );
        }
        assert_eq!(plan.hoisted_sources_at(hoist.header), 1);
    }

    #[test]
    fn conditionally_used_typed_receiver_gets_no_storage_hoist() {
        let (ir, feedback) = fixture(
            "(function(o,n){const f=o.f;for(let i=0;i<n;i++){if(f)f[i]=i}return 0})",
            ArrayMode::Float64,
        );
        let plan = plan(&ir, &feedback, true, 100_000);
        assert!(
            ir.nodes().iter().any(|node| plan
                .access(node.id())
                .is_some_and(|access| access.access == ArrayAccess::Store)),
            "the conditional store is still a planned guarded leaf: {plan:#?}"
        );
        assert!(
            plan.storage_hoists().is_empty(),
            "a conditional store must not guard its receiver on loop entry: {plan:#?}"
        );
    }

    #[test]
    fn more_than_max_live_receivers_keep_exact_guards() {
        let (ir, feedback) = fixture(
            "(function(o){const a=o.a,b=o.b,c=o.c,d=o.d,e=o.e,f=o.f;let s=0;for(let i=0;i<a.length;i++){s=s+a[i]+b[i]+c[i]+d[i]+e[i];f[i]=s}return s})",
            ArrayMode::Float64,
        );
        let plan = plan(&ir, &feedback, true, 100_000);
        for natural_loop in LoopAnalysis::analyze(&ir, 100_000).loops() {
            assert!(
                plan.hoisted_sources_at(natural_loop.header()) <= ArrayPlan::MAX_LIVE_SOURCES,
                "{plan:#?}"
            );
        }
        // Accesses of an unhoisted receiver keep their own index guard and
        // are never marked as covered by a hoist.
        let hoisted: Vec<usize> = plan
            .hoists()
            .iter()
            .map(|hoist| hoist.candidate)
            .chain(plan.storage_hoists().iter().map(|hoist| hoist.candidate))
            .collect();
        let mut unhoisted = 0;
        for node in ir.nodes() {
            let Some(access) = plan.access(node.id()) else {
                continue;
            };
            if access.bounds_covered_by_hoist {
                assert!(
                    plan.hoists()
                        .iter()
                        .any(|hoist| hoist.candidate == access.candidate),
                    "{plan:#?}"
                );
            }
            if access.access != ArrayAccess::Length && !hoisted.contains(&access.candidate) {
                assert!(access.requires_index_guard, "{plan:#?}");
                unhoisted += 1;
            }
        }
        assert!(
            unhoisted > 0,
            "six receivers exceed the live-tuple cap: {plan:#?}"
        );
    }

    #[test]
    fn store_only_typed_receiver_gets_one_storage_hoist() {
        let (ir, feedback) = fixture(
            "(function(o,n){const f=o.f;for(let i=0;i<n;i++)f[i]=i;return 0})",
            ArrayMode::Float64,
        );
        let plan = plan(&ir, &feedback, true, 100_000);
        assert!(plan.hoists().is_empty());
        let [hoist] = plan.storage_hoists() else {
            panic!("one storage hoist expected: {plan:#?}");
        };
        assert_ne!(hoist.preheader, hoist.header);
        assert!(matches!(
            plan.candidates()[hoist.candidate].receiver,
            ArrayReceiver::Local(_)
        ));
        // The store keeps its own per-access index guard.
        assert!(ir
            .nodes()
            .iter()
            .filter_map(|node| plan.access(node.id()))
            .all(|access| access.requires_index_guard));
    }

    #[test]
    fn typed_loop_without_length_hoists_metadata_but_keeps_every_bounds_check() {
        for mode in [ArrayMode::Int32, ArrayMode::Float64, ArrayMode::Packed] {
            let (ir, feedback) = fixture(
                "(function(a,n){let s=0;for(let i=0;i<n;i++)s=(s+a[i])|0;return s})",
                mode,
            );
            let plan = plan(&ir, &feedback, true, 100_000);
            let loads = ir
                .nodes()
                .iter()
                .filter_map(|node| plan.access(node.id()))
                .filter(|access| access.access == ArrayAccess::Load)
                .collect::<Vec<_>>();
            assert_eq!(loads.len(), 1, "{mode:?}: {plan:#?}");
            assert!(
                loads[0].requires_index_guard && !loads[0].bounds_covered_by_hoist,
                "an unrelated loop bound can never delete a bounds check: {mode:?}"
            );
            // Element-only loops never get a length hoist.
            assert!(plan.hoists().is_empty(), "{plan:#?}");
            if mode == ArrayMode::Packed {
                // A packed guard would demand logical length == dense count,
                // which an element-only loop never needed.
                assert!(plan.storage_hoists().is_empty(), "{plan:#?}");
            } else {
                let [hoist] = plan.storage_hoists() else {
                    panic!("typed element loop must hoist its storage metadata: {plan:#?}");
                };
                assert!(matches!(
                    plan.candidates()[hoist.candidate].receiver,
                    ArrayReceiver::Argument(0)
                ));
                assert_ne!(hoist.preheader, hoist.header);
            }
        }
    }
}
