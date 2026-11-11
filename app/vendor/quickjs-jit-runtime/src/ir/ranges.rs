//! Bounded integer proofs for deleting array bounds comparisons.
//!
//! Like JSC's DFGIntegerRangeOptimizationPhase, facts relate SSA values on
//! particular branch edges, joins require every incoming path, and arithmetic
//! contracts must exclude wrapping. This intentionally implements a smaller
//! domain: nonnegative integers and strict upper bounds. Exhaustion loses facts.
//!
//! Primary reference:
//! <https://github.com/WebKit/WebKit/blob/main/Source/JavaScriptCore/dfg/DFGIntegerRangeOptimizationPhase.cpp>

use std::collections::{BTreeMap, BTreeSet};

use super::{
    FrameSlot, OptimizedBlock, OptimizedEffect, OptimizedNode, OptimizedNodeKind, ScalarBinaryOp,
    ScalarCompareOp, ScalarGraph, ScalarNumericMode, ScalarValue, ScalarValueId,
};

pub struct IntegerRangeAnalysis<'a> {
    graph: &'a ScalarGraph,
    blocks: &'a [OptimizedBlock],
    nodes: &'a [OptimizedNode],
    predecessors: BTreeMap<u32, Vec<usize>>,
    block_indices: BTreeMap<u32, usize>,
    node_blocks: Vec<Option<(usize, usize)>>,
    aliases: Vec<Option<ScalarValueId>>,
    nonnegative: Vec<bool>,
    preserved_nodes: Vec<bool>,
    remaining_work: usize,
    complete: bool,
}

impl<'a> IntegerRangeAnalysis<'a> {
    /// Conservative reservation for the persistent analysis plus one query's
    /// simultaneously live scratch. Every inserted edge/value/state consumes
    /// work before insertion; this allowance includes spare Vec capacity and
    /// sparsely occupied BTree nodes. Saturation makes oversized requests fail
    /// the compiler's resource reservation rather than wrap.
    pub(crate) fn bytes_upper_bound(work_limit: usize) -> usize {
        core::mem::size_of::<Self>().saturating_add(work_limit.saturating_mul(512))
    }

    pub fn analyze(
        graph: &'a ScalarGraph,
        blocks: &'a [OptimizedBlock],
        nodes: &'a [OptimizedNode],
        preserve_poll_bindings: bool,
        work_limit: usize,
    ) -> Self {
        Self::analyze_with_guarded_heap(
            graph,
            blocks,
            nodes,
            preserve_poll_bindings,
            |_| false,
            work_limit,
        )
    }

    /// `guarded_heap(node)` is a compiler capability, not feedback: the node
    /// must execute only a guarded non-reentrant primitive heap fast path, or
    /// exit native code before its effect. Its successful path preserves frame
    /// slots. Poll aliases require the analogous register-preservation contract.
    pub fn analyze_with_guarded_heap(
        graph: &'a ScalarGraph,
        blocks: &'a [OptimizedBlock],
        nodes: &'a [OptimizedNode],
        preserve_poll_bindings: bool,
        guarded_heap: impl Fn(u32) -> bool,
        work_limit: usize,
    ) -> Self {
        let mut result = Self {
            graph,
            blocks,
            nodes,
            predecessors: BTreeMap::new(),
            block_indices: BTreeMap::new(),
            node_blocks: Vec::new(),
            aliases: Vec::new(),
            nonnegative: Vec::new(),
            preserved_nodes: Vec::new(),
            remaining_work: work_limit,
            complete: false,
        };
        let allocation = graph
            .values()
            .len()
            .saturating_add(nodes.len())
            .saturating_add(blocks.len());
        if !spend(&mut result.remaining_work, allocation) {
            return result;
        }
        result.node_blocks.resize(nodes.len(), None);
        result.preserved_nodes = nodes
            .iter()
            .map(|node| {
                preserve_poll_bindings
                    && matches!(node.kind(), OptimizedNodeKind::GuardNumeric { .. })
                    || guarded_heap(node.id())
            })
            .collect();
        for (block_index, block) in blocks.iter().enumerate() {
            if result
                .block_indices
                .insert(block.start_pc(), block_index)
                .is_some()
            {
                return result;
            }
            for (position, &node) in block.nodes().iter().enumerate() {
                if !spend(&mut result.remaining_work, 1) {
                    return result;
                }
                let Some(entry) = result.node_blocks.get_mut(node as usize) else {
                    return result;
                };
                if entry.replace((block_index, position)).is_some() {
                    return result;
                }
            }
            for &successor in block.successors() {
                if !spend(&mut result.remaining_work, 1) {
                    return result;
                }
                result
                    .predecessors
                    .entry(successor)
                    .or_default()
                    .push(block_index);
            }
        }
        result.aliases = graph
            .values()
            .iter()
            .map(|value| {
                let ScalarValue::FrameRead { node, slot } = value else {
                    return None;
                };
                if !result
                    .preserved_nodes
                    .get(*node as usize)
                    .copied()
                    .unwrap_or(false)
                {
                    return None;
                }
                let state = graph.frame_state_for_node(*node)?;
                match slot {
                    FrameSlot::Argument(index) => state.arguments.get(usize::from(*index)),
                    FrameSlot::Local(index) => state.locals.get(usize::from(*index)),
                    FrameSlot::Stack(index) => state.stack.get(usize::from(*index)),
                }
                .copied()
            })
            .collect();
        let invalidated_headers = blocks
            .iter()
            .filter(|block| {
                block.nodes().iter().any(|&node| {
                    matches!(
                        nodes[node as usize].kind(),
                        OptimizedNodeKind::GuardNumeric { mid_loop: true, .. }
                    ) && !result.preserved_nodes[node as usize]
                })
            })
            .map(OptimizedBlock::start_pc)
            .collect::<BTreeSet<_>>();

        // 0 is unseeded; 1 is a nonnegative seed; 2 is an invalid/negative
        // incoming value. Union propagates unknown edges through entire SCCs.
        // A seedless cycle never acquires the bit required by the consumer.
        let mut signs = vec![0u8; graph.values().len()];
        loop {
            let mut changed = false;
            for (id, value) in graph.values().iter().enumerate() {
                let cost = match value {
                    ScalarValue::Phi { inputs, .. } => inputs.len().max(1),
                    _ => 1,
                };
                if !spend(&mut result.remaining_work, cost) {
                    return result;
                }
                let sign = |id: ScalarValueId| signs.get(id.index()).copied().unwrap_or(2);
                let next = match value {
                    ScalarValue::Int32(value) => {
                        if *value >= 0 {
                            1
                        } else {
                            2
                        }
                    }
                    ScalarValue::Phi {
                        block_pc, inputs, ..
                    } if !inputs.is_empty() && !invalidated_headers.contains(block_pc) => {
                        inputs.iter().fold(0, |set, input| set | sign(input.value))
                    }
                    ScalarValue::Update {
                        mode: ScalarNumericMode::Int32,
                        input,
                        delta,
                    } if *delta >= 0 => sign(*input),
                    ScalarValue::Binary {
                        mode: ScalarNumericMode::Int32,
                        op: ScalarBinaryOp::Add,
                        lhs,
                        rhs,
                    } => {
                        let (lhs, rhs) = (sign(*lhs), sign(*rhs));
                        if lhs == 0 || rhs == 0 {
                            0
                        } else {
                            lhs | rhs
                        }
                    }
                    ScalarValue::FrameRead { .. } => result.aliases[id].map_or(2, sign),
                    _ => 2,
                } | signs[id];
                changed |= next != signs[id];
                signs[id] = next;
            }
            if !changed {
                // A separate seedless SCC may feed a seeded Phi. It must be
                // unknown on that incoming edge, rather than silently absent.
                if signs.contains(&0) {
                    for sign in &mut signs {
                        if *sign == 0 {
                            *sign = 2;
                        }
                    }
                } else {
                    break;
                }
            }
        }
        result.nonnegative = signs.into_iter().map(|sign| sign == 1).collect();
        result.complete = true;
        result
    }

    pub const fn is_complete(&self) -> bool {
        self.complete
    }

    /// Proves `0 <= index < bound` immediately before `node`. The consumer
    /// MUST retain integer representation checks, checked arithmetic overflow
    /// exits, and all array class/storage/length validity checks. `bound` must
    /// denote the exact guarded length; a stale heap load is not interchangeable
    /// with the current length. This API never proves heap stability.
    pub fn proves_in_bounds_at(
        &self,
        index: ScalarValueId,
        bound: ScalarValueId,
        node: u32,
    ) -> bool {
        if !self.complete
            || !self
                .nonnegative
                .get(index.index())
                .copied()
                .unwrap_or(false)
        {
            return false;
        }
        let Some((block, position)) = self.node_blocks.get(node as usize).copied().flatten() else {
            return false;
        };
        let mut work = self.remaining_work;
        let mut pending = vec![(block, position, index, bound)];
        let mut visited = BTreeSet::new();
        while let Some((block_index, position, index, bound)) = pending.pop() {
            if !spend(&mut work, 1) || !visited.insert((block_index, position, index, bound)) {
                return false;
            }
            let Some(index) = self.canonical(index, &mut work) else {
                return false;
            };
            let Some(bound) = self.canonical(bound, &mut work) else {
                return false;
            };
            if let (Some(ScalarValue::Int32(i)), Some(ScalarValue::Int32(n))) = (
                self.graph.values().get(index.index()),
                self.graph.values().get(bound.index()),
            ) {
                if i < n {
                    continue;
                }
            }
            let block = &self.blocks[block_index];
            for &node in &block.nodes()[..position] {
                if !spend(&mut work, 1) {
                    return false;
                }
                let node = &self.nodes[node as usize];
                if self.is_barrier(node) {
                    return false;
                }
            }
            // The implicit function-entry edge cannot be justified by a backedge.
            if block.start_pc() == 0 {
                return false;
            }
            let Some(predecessors) = self
                .predecessors
                .get(&block.start_pc())
                .filter(|p| !p.is_empty())
            else {
                return false;
            };
            for &predecessor in predecessors {
                if !spend(&mut work, 1) {
                    return false;
                }
                let predecessor_block = &self.blocks[predecessor];
                let Some(index) = self.on_incoming_edge(
                    index,
                    block.start_pc(),
                    predecessor_block.start_pc(),
                    &mut work,
                ) else {
                    return false;
                };
                let Some(bound) = self.on_incoming_edge(
                    bound,
                    block.start_pc(),
                    predecessor_block.start_pc(),
                    &mut work,
                ) else {
                    return false;
                };
                if !self.edge_proves(predecessor_block, block.start_pc(), index, bound, &mut work) {
                    pending.push((predecessor, predecessor_block.nodes().len(), index, bound));
                }
            }
        }
        true
    }

    fn on_incoming_edge(
        &self,
        value: ScalarValueId,
        block: u32,
        predecessor: u32,
        work: &mut usize,
    ) -> Option<ScalarValueId> {
        match self.graph.values().get(value.index())? {
            ScalarValue::Phi {
                block_pc, inputs, ..
            } if *block_pc == block => {
                if !spend(work, inputs.len()) {
                    return None;
                }
                let mut incoming = inputs
                    .iter()
                    .filter(|input| input.predecessor == Some(predecessor));
                let value = incoming.next()?.value;
                if incoming.next().is_some() {
                    None
                } else {
                    Some(value)
                }
            }
            _ => Some(value),
        }
    }

    fn is_barrier(&self, node: &OptimizedNode) -> bool {
        (matches!(
            node.effect(),
            OptimizedEffect::Poll | OptimizedEffect::Reentrant
        ) || matches!(
            node.kind(),
            OptimizedNodeKind::GuardNumeric { mid_loop: true, .. }
        )) && !self.preserved_nodes[node.id() as usize]
    }

    fn strip_alias(&self, mut value: ScalarValueId, work: &mut usize) -> Option<ScalarValueId> {
        loop {
            if !spend(work, 1) {
                return None;
            }
            match self.graph.values().get(value.index())? {
                ScalarValue::Phi { inputs, .. } if inputs.len() == 1 => value = inputs[0].value,
                ScalarValue::FrameRead { .. } if self.aliases[value.index()].is_some() => {
                    value = self.aliases[value.index()]?
                }
                _ => return Some(value),
            }
        }
    }

    fn canonical(&self, value: ScalarValueId, work: &mut usize) -> Option<ScalarValueId> {
        let original = self.strip_alias(value, work)?;
        let mut pending = vec![original];
        let mut visited = BTreeSet::new();
        let mut reverse = BTreeMap::<ScalarValueId, Vec<ScalarValueId>>::new();
        let mut seed = None;
        while let Some(value) = pending.pop() {
            let value = self.strip_alias(value, work)?;
            if !visited.insert(value) {
                continue;
            }
            if let ScalarValue::Phi { inputs, .. } = self.graph.values().get(value.index())? {
                if inputs.is_empty() {
                    return Some(original);
                }
                if !spend(work, inputs.len()) {
                    return None;
                }
                for input in inputs {
                    let input = self.strip_alias(input.value, work)?;
                    reverse.entry(input).or_default().push(value);
                    pending.push(input);
                }
            } else if seed.is_some_and(|seed| seed != value) {
                return Some(original);
            } else {
                seed = Some(value);
            }
        }
        let Some(seed) = seed else {
            return Some(original);
        };
        let mut reached = BTreeSet::new();
        pending.push(seed);
        while let Some(value) = pending.pop() {
            if !spend(work, 1) {
                return None;
            }
            if !reached.insert(value) {
                continue;
            }
            if let Some(parents) = reverse.get(&value) {
                if !spend(work, parents.len()) {
                    return None;
                }
                pending.extend(parents);
            }
        }
        Some(if reached == visited { seed } else { original })
    }

    fn edge_proves(
        &self,
        block: &OptimizedBlock,
        successor: u32,
        index: ScalarValueId,
        bound: ScalarValueId,
        work: &mut usize,
    ) -> bool {
        let mut proof = || -> Option<bool> {
            if block.successors().len() != 2 {
                return Some(false);
            }
            for &node in block.nodes() {
                if !spend(work, 1) || self.is_barrier(&self.nodes[node as usize]) {
                    return Some(false);
                }
            }
            let branch = self.nodes.get(*block.nodes().last()? as usize)?;
            let OptimizedNodeKind::Bytecode { opcode } = branch.kind() else {
                return Some(false);
            };
            let target_is_true = match opcode.as_ref() {
                "if_true" | "if_true8" => true,
                "if_false" | "if_false8" => false,
                _ => return Some(false),
            };
            let truth = (branch.branch_target()? == successor) == target_is_true;
            let condition = *self.graph.frame_state_for_node(branch.id())?.stack.last()?;
            let ScalarValue::Compare { op, lhs, rhs } =
                self.graph.values().get(condition.index())?
            else {
                return Some(false);
            };
            let index = self.canonical(index, work)?;
            let bound = self.canonical(bound, work)?;
            let lhs = self.canonical(*lhs, work)?;
            let rhs = self.canonical(*rhs, work)?;
            Some(match (op, truth) {
                (ScalarCompareOp::LessThan, true) | (ScalarCompareOp::GreaterEqual, false) => {
                    lhs == index && rhs == bound
                }
                (ScalarCompareOp::GreaterThan, true) | (ScalarCompareOp::LessEqual, false) => {
                    rhs == index && lhs == bound
                }
                _ => false,
            })
        };
        proof().unwrap_or(false)
    }
}

fn spend(work: &mut usize, cost: usize) -> bool {
    match work.checked_sub(cost) {
        Some(remaining) => {
            *work = remaining;
            true
        }
        None => {
            *work = 0;
            false
        }
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::{
        bytecode::VerifyLimits,
        ir::{OptimizedIr, OptimizedNodeKind, ScalarNumericMode},
        test_support::SnapshotFixture,
    };

    fn graph(source: &str) -> OptimizedIr {
        let fixture = SnapshotFixture::compile(source);
        let verified = fixture.snapshot().verify(VerifyLimits::default()).unwrap();
        let modes = verified
            .instructions()
            .iter()
            .map(|instruction| (instruction.pc(), ScalarNumericMode::Int32))
            .collect();
        OptimizedIr::translate_with_numeric_modes(&verified, 1, &modes).unwrap()
    }

    fn access(ir: &OptimizedIr) -> (ScalarValueId, ScalarValueId, u32) {
        let node = ir.nodes().iter().find(|node| matches!(node.kind(), OptimizedNodeKind::Bytecode { opcode } if opcode.as_ref() == "mul")).unwrap();
        let state = ir.scalar_graph().frame_state_for_node(node.id()).unwrap();
        (
            state.stack[state.stack.len() - 2],
            state.arguments[1],
            node.id(),
        )
    }

    #[test]
    fn ascending_counted_loop_proves_both_bounds_from_seed_and_branch() {
        let ir = graph("(function(a,n){let s=0;for(let i=0;i<n;i++)s+=i*n;return s})");
        let (index, bound, node) = access(&ir);
        assert!(IntegerRangeAnalysis::analyze(
            ir.scalar_graph(),
            ir.blocks(),
            ir.nodes(),
            true,
            100_000
        )
        .proves_in_bounds_at(index, bound, node));
    }

    #[test]
    fn negative_seed_wrong_branch_mutable_limit_and_wrapping_update_keep_checks() {
        for source in [
            "(function(a,n){let s=0;for(let i=-1;i<n;i++)s+=i*n;return s})",
            "(function(a,n){let s=0;for(let i=0;i>=n;i++)s+=i*n;return s})",
            "(function(a,n){let s=0;for(let i=0;i<n;i++){n=0;s+=i*n}return s})",
            "(function(a,n){let s=0;for(let i=0;i<n;i=(i+1)|0)s+=i*n;return s})",
        ] {
            let ir = graph(source);
            let (index, bound, node) = access(&ir);
            assert!(
                !IntegerRangeAnalysis::analyze(
                    ir.scalar_graph(),
                    ir.blocks(),
                    ir.nodes(),
                    true,
                    100_000
                )
                .proves_in_bounds_at(index, bound, node),
                "{source}"
            );
        }
    }

    #[test]
    fn polls_and_work_exhaustion_fail_closed() {
        let ir = graph("(function(a,n){let s=0;for(let i=0;i<n;i++)s+=i*n;return s})");
        let (index, bound, node) = access(&ir);
        for (preserve, work) in [(false, 100_000), (true, 1), (true, 0)] {
            assert!(
                !IntegerRangeAnalysis::analyze(
                    ir.scalar_graph(),
                    ir.blocks(),
                    ir.nodes(),
                    preserve,
                    work
                )
                .proves_in_bounds_at(index, bound, node),
                "preserve={preserve}, work={work}"
            );
        }
    }

    #[test]
    fn every_incoming_path_and_current_index_must_satisfy_branch() {
        for source in [
            "(function(a,n){let s=0;for(let i=a;i<n;i++)s+=i*n;return s})",
            "(function(a,n){let s=0;for(let i=0;i<n;i++){i=-1;s+=i*n}return s})",
            "(function(a,n){let i=0;if(i<n || a)return i*n;return 0})",
            "(function(a,n){let s=0;for(let i=0;i<n;i++){a();s+=i*n}return s})",
        ] {
            let ir = graph(source);
            let (index, bound, node) = access(&ir);
            assert!(
                !IntegerRangeAnalysis::analyze(
                    ir.scalar_graph(),
                    ir.blocks(),
                    ir.nodes(),
                    true,
                    100_000
                )
                .proves_in_bounds_at(index, bound, node),
                "{source}"
            );
        }
    }

    #[test]
    fn guarded_array_fast_path_preserves_induction_but_generic_access_does_not() {
        let ir = graph("(function(a,n){let s=0;for(let i=0;i<n;i++)s+=a[i];return s})");
        let graph = ir.scalar_graph();
        let access = ir.nodes().iter().find(|node| matches!(node.kind(), OptimizedNodeKind::Bytecode { opcode } if opcode.as_ref() == "get_array_el")).unwrap();
        let state = graph.frame_state_for_node(access.id()).unwrap();
        let index = *state.stack.last().unwrap();
        let bound = state.arguments[1];
        assert!(
            !IntegerRangeAnalysis::analyze(graph, ir.blocks(), ir.nodes(), true, 100_000)
                .proves_in_bounds_at(index, bound, access.id())
        );
        assert!(IntegerRangeAnalysis::analyze_with_guarded_heap(
            graph,
            ir.blocks(),
            ir.nodes(),
            true,
            |node| node == access.id(),
            100_000
        )
        .proves_in_bounds_at(index, bound, access.id()));
    }

    #[test]
    fn guarded_array_length_and_access_keep_exact_bound_identity() {
        let ir =
            graph("(function(a){let n=a.length;let s=0;for(let i=0;i<n;i++)s+=a[i];return s})");
        let graph = ir.scalar_graph();
        let access = ir.nodes().iter().find(|node| matches!(node.kind(), OptimizedNodeKind::Bytecode { opcode } if opcode.as_ref() == "get_array_el")).unwrap();
        let state = graph.frame_state_for_node(access.id()).unwrap();
        let index = *state.stack.last().unwrap();
        let bound = state.locals[0];
        let ranges = IntegerRangeAnalysis::analyze_with_guarded_heap(
            graph,
            ir.blocks(),
            ir.nodes(),
            true,
            |id| matches!(ir.nodes()[id as usize].kind(), OptimizedNodeKind::Bytecode { opcode } if matches!(opcode.as_ref(), "get_array_el" | "get_length")),
            100_000,
        );
        assert!(ranges.proves_in_bounds_at(index, bound, access.id()));
    }
}
