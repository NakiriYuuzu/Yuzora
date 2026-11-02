//! Bounded dominance and natural-loop proofs for speculative optimization.
//!
//! As in JSC's DFGLICMPhase, membership and preheaders come from dominance,
//! never bytecode order. A dedicated preheader is only a control-flow proof:
//! consumers must separately prove recovery, effects and zero-trip safety.

use super::{OptimizedIr, OptimizedNodeKind};

#[derive(Debug, Default)]
pub(crate) struct LoopAnalysis {
    complete: bool,
    pcs: Vec<u32>,
    dominators: Vec<Vec<u64>>,
    owners: Vec<Option<(usize, usize)>>,
    loop_nodes: Vec<bool>,
    loops: Vec<NaturalLoop>,
}

#[derive(Debug)]
pub(crate) struct NaturalLoop {
    header: u32,
    preheader: Option<u32>,
    members: Vec<u32>,
}

impl NaturalLoop {
    pub(crate) fn header(&self) -> u32 {
        self.header
    }
    pub(crate) fn preheader(&self) -> Option<u32> {
        self.preheader
    }
    pub(crate) fn members(&self) -> &[u32] {
        &self.members
    }
    pub(crate) fn contains_block(&self, pc: u32) -> bool {
        self.members.binary_search(&pc).is_ok()
    }
}

impl LoopAnalysis {
    /// Failure is an empty proof, including after partial convergence. In
    /// particular, `!contains(node)` alone does not prove the node is acyclic.
    pub(crate) fn analyze(ir: &OptimizedIr, work: usize) -> Self {
        Self::try_analyze(ir, work).unwrap_or_default()
    }

    /// Covers peak retained and temporary allocations. Every allocation also
    /// consumes work before it occurs, giving a second independent bound.
    pub(crate) fn bytes_upper_bound(ir: &OptimizedIr, work: usize) -> usize {
        let n = ir.blocks().len();
        let nodes = ir.nodes().len();
        let edges = ir.blocks().iter().fold(0usize, |sum, block| {
            sum.saturating_add(block.successors().len())
        });
        let graph_bound = n
            .saturating_mul(n.div_ceil(64))
            .saturating_mul(8)
            .saturating_add(n.saturating_mul(n).saturating_mul(4))
            .saturating_add(n.saturating_mul(256))
            .saturating_add(nodes.saturating_mul(64))
            .saturating_add(edges.saturating_mul(64));
        graph_bound.min(work.saturating_mul(64))
    }

    fn try_analyze(ir: &OptimizedIr, mut work: usize) -> Option<Self> {
        let n = ir.blocks().len();
        spend(&mut work, n.checked_mul(4)?.checked_add(ir.nodes().len())?)?;
        let pcs: Vec<_> = ir.blocks().iter().map(|b| b.start_pc()).collect();
        if pcs.first() != Some(&0) || pcs.windows(2).any(|p| p[0] >= p[1]) {
            return None;
        }
        let mut successors = Vec::with_capacity(n);
        let mut owners = vec![None; ir.nodes().len()];
        let search_cost = (usize::BITS - n.leading_zeros()) as usize + 1;
        for (index, block) in ir.blocks().iter().enumerate() {
            spend(
                &mut work,
                block.successors().len().checked_mul(search_cost)?,
            )?;
            let mut edges = Vec::with_capacity(block.successors().len());
            for pc in block.successors() {
                edges.push(pcs.binary_search(pc).ok()?);
            }
            successors.push(edges);
            for (position, &id) in block.nodes().iter().enumerate() {
                spend(&mut work, 1)?;
                let node = ir.nodes().get(id as usize)?;
                if node.id() != id || owners[id as usize].replace((index, position)).is_some() {
                    return None;
                }
            }
        }
        for (id, owner) in owners.iter().enumerate() {
            spend(&mut work, 1)?;
            // The entry guard belongs to the function prologue, outside the
            // bytecode CFG (even when block zero is itself a loop header).
            if owner.is_none()
                && !(id == 0
                    && ir.nodes()[id].pc() == 0
                    && matches!(
                        ir.nodes()[id].kind(),
                        OptimizedNodeKind::GuardNumeric {
                            mid_loop: false,
                            ..
                        }
                    ))
            {
                return None;
            }
        }
        Self::analyze_graph(pcs, successors, owners, work)
    }

    fn analyze_graph(
        pcs: Vec<u32>,
        successors: Vec<Vec<usize>>,
        owners: Vec<Option<(usize, usize)>>,
        mut work: usize,
    ) -> Option<Self> {
        let n = pcs.len();
        let words = n.div_ceil(64);
        // Dominator storage is quadratic in bits, but its complete allocation
        // is charged up front. Nested loop member storage is charged per loop.
        spend(
            &mut work,
            n.checked_mul(16)?
                .checked_add(n.checked_mul(words)?)?
                .checked_add(owners.len())?,
        )?;
        if pcs.first() != Some(&0) || pcs.windows(2).any(|p| p[0] >= p[1]) || successors.len() != n
        {
            return None;
        }
        let mut predecessors = vec![Vec::new(); n];
        for (from, edges) in successors.iter().enumerate() {
            for &to in edges {
                spend(&mut work, 1)?;
                predecessors.get_mut(to)?.push(from);
            }
        }
        let mut reachable = vec![false; n];
        reachable[0] = true;
        let mut pending = vec![0];
        while let Some(block) = pending.pop() {
            spend(&mut work, 1)?;
            for &next in &successors[block] {
                spend(&mut work, 1)?;
                if !reachable[next] {
                    reachable[next] = true;
                    pending.push(next);
                }
            }
        }
        let mut reachable_bits = vec![0u64; words];
        for (index, &live) in reachable.iter().enumerate() {
            if live {
                reachable_bits[index / 64] |= 1 << (index % 64);
            }
        }
        let mut dominators = vec![vec![0u64; words]; n];
        for (index, row) in dominators.iter_mut().enumerate() {
            if index == 0 {
                row[0] = 1;
            } else if reachable[index] {
                row.copy_from_slice(&reachable_bits);
            }
        }
        loop {
            let mut changed = false;
            for block in 1..n {
                spend(&mut work, 1)?;
                if !reachable[block] {
                    continue;
                }
                spend(&mut work, words)?;
                let mut next = reachable_bits.clone();
                for &pred in &predecessors[block] {
                    spend(&mut work, 1)?;
                    if !reachable[pred] {
                        continue;
                    }
                    spend(&mut work, words)?;
                    for (word, &incoming) in next.iter_mut().zip(&dominators[pred]) {
                        *word &= incoming;
                    }
                }
                next[block / 64] |= 1 << (block % 64);
                spend(&mut work, words)?;
                if next != dominators[block] {
                    dominators[block] = next;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let dominates = |a: usize, b: usize| dominators[b][a / 64] & (1 << (a % 64)) != 0;

        // Removing every dominance backedge leaves a DAG exactly for a
        // reducible flow graph. Fail closed for residual multiple-entry cycles.
        let mut indegree = vec![0usize; n];
        for (from, edges) in successors.iter().enumerate() {
            spend(&mut work, 1)?;
            if !reachable[from] {
                continue;
            }
            for &to in edges {
                spend(&mut work, 1)?;
                if !dominates(to, from) {
                    indegree[to] = indegree[to].checked_add(1)?;
                }
            }
        }
        pending.extend((0..n).filter(|&i| reachable[i] && indegree[i] == 0));
        let mut visited = 0usize;
        while let Some(from) = pending.pop() {
            spend(&mut work, 1)?;
            visited += 1;
            for &to in &successors[from] {
                spend(&mut work, 1)?;
                if dominates(to, from) {
                    continue;
                }
                indegree[to] = indegree[to].checked_sub(1)?;
                if indegree[to] == 0 {
                    pending.push(to);
                }
            }
        }
        if visited != reachable.iter().filter(|&&live| live).count() {
            return None;
        }

        let mut loops = Vec::new();
        let mut loop_blocks = vec![false; n];
        for header in 0..n {
            spend(&mut work, 1)?;
            if !reachable[header] {
                continue;
            }
            let mut latches = Vec::new();
            for &pred in &predecessors[header] {
                spend(&mut work, 1)?;
                if dominates(header, pred) {
                    latches.push(pred);
                }
            }
            if latches.is_empty() {
                continue;
            }
            spend(&mut work, n.checked_mul(2)?)?;
            let mut members = vec![false; n];
            members[header] = true;
            for latch in latches {
                if !members[latch] {
                    members[latch] = true;
                    pending.push(latch);
                }
            }
            while let Some(block) = pending.pop() {
                spend(&mut work, 1)?;
                for &pred in &predecessors[block] {
                    spend(&mut work, 1)?;
                    if !reachable[pred] {
                        continue;
                    }
                    if !dominates(header, pred) {
                        return None;
                    }
                    if !members[pred] {
                        members[pred] = true;
                        pending.push(pred);
                    }
                }
            }
            let mut preheader = None;
            let mut entries = 0usize;
            for &pred in &predecessors[header] {
                spend(&mut work, 1)?;
                if reachable[pred] && !members[pred] {
                    entries += 1;
                    preheader = Some(pred);
                }
            }
            let preheader = preheader
                .filter(|&pred| {
                    entries == 1
                        && successors[pred].as_slice() == [header]
                        && dominates(pred, header)
                })
                .map(|index| pcs[index]);
            let mut member_pcs = Vec::with_capacity(n);
            for (block, &member) in members.iter().enumerate() {
                if member {
                    loop_blocks[block] = true;
                    member_pcs.push(pcs[block]);
                }
            }
            loops.push(NaturalLoop {
                header: pcs[header],
                preheader,
                members: member_pcs,
            });
        }
        spend(&mut work, owners.len())?;
        let mut loop_nodes = Vec::with_capacity(owners.len());
        for owner in &owners {
            let member = match owner {
                Some((block, _)) => *loop_blocks.get(*block)?,
                None => false,
            };
            loop_nodes.push(member);
        }
        Some(Self {
            complete: true,
            pcs,
            dominators,
            owners,
            loop_nodes,
            loops,
        })
    }
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }
    pub(crate) fn contains(&self, node: u32) -> bool {
        self.loop_nodes.get(node as usize).copied().unwrap_or(false)
    }
    pub(crate) fn loops(&self) -> &[NaturalLoop] {
        &self.loops
    }
    pub(crate) fn dominates(&self, a: u32, b: u32) -> bool {
        let (Ok(a), Ok(b)) = (self.pcs.binary_search(&a), self.pcs.binary_search(&b)) else {
            return false;
        };
        self.dominators[b][a / 64] & (1 << (a % 64)) != 0
    }
    pub(crate) fn block_for_node(&self, node: u32) -> Option<u32> {
        let (block, _) = self.owners.get(node as usize).copied().flatten()?;
        self.pcs.get(block).copied()
    }
    #[allow(dead_code)]
    pub(crate) fn node_dominates(&self, a: u32, b: u32) -> bool {
        let Some((ab, ai)) = self.owners.get(a as usize).copied().flatten() else {
            return false;
        };
        let Some((bb, bi)) = self.owners.get(b as usize).copied().flatten() else {
            return false;
        };
        self.dominates(self.pcs[ab], self.pcs[bb]) && (ab != bb || ai <= bi)
    }
}

fn spend(work: &mut usize, cost: usize) -> Option<()> {
    *work = work.checked_sub(cost)?;
    Some(())
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::{bytecode::VerifyLimits, test_support::SnapshotFixture};

    fn translate(source: &str) -> OptimizedIr {
        let fixture = SnapshotFixture::compile(source);
        let verified = fixture.snapshot().verify(VerifyLimits::default()).unwrap();
        OptimizedIr::translate(&verified, 1).unwrap()
    }

    fn graph(successors: &[&[usize]], work: usize) -> LoopAnalysis {
        LoopAnalysis::analyze_graph(
            (0..successors.len() as u32).collect(),
            successors.iter().map(|s| s.to_vec()).collect(),
            (0..successors.len()).map(|i| Some((i, 0))).collect(),
            work,
        )
        .unwrap_or_default()
    }

    #[test]
    fn all_latches_are_unioned_and_numeric_pc_order_is_not_dominance() {
        // Entry 0 -> header 3 -> body 1/2 -> header 3, with exit 4.
        let proof = graph(&[&[3], &[3], &[3], &[1, 2, 4], &[]], 100_000);
        assert!(proof.is_complete());
        assert_eq!(proof.loops().len(), 1);
        assert_eq!(proof.loops()[0].header(), 3);
        assert_eq!(proof.loops()[0].members(), &[1, 2, 3]);
        assert_eq!(proof.loops()[0].preheader(), Some(0));
        assert!(proof.dominates(3, 1));
        assert!(!proof.dominates(1, 3));
        assert!(!proof.dominates(1, 2));
        assert!(!proof.contains(4));
    }

    #[test]
    fn unreachable_cycle_cannot_seed_a_loop_or_dominator_proof() {
        let proof = graph(&[&[1], &[], &[3], &[2]], 100_000);
        assert!(proof.is_complete());
        assert!(proof.loops().is_empty());
        assert!(!proof.dominates(2, 2));
        assert!(!proof.node_dominates(2, 2));
        assert!(!proof.contains(2));
    }

    #[test]
    fn multiple_entry_irreducible_cycle_has_no_proof() {
        let proof = graph(&[&[1, 2], &[2, 3], &[1, 3], &[]], 100_000);
        assert!(!proof.is_complete());
        assert!(proof.loops().is_empty());
        assert!(!proof.dominates(0, 1));
    }

    #[test]
    fn conditional_entry_is_not_a_dedicated_preheader() {
        let proof = graph(&[&[1, 3], &[2, 3], &[1], &[]], 100_000);
        assert!(proof.is_complete());
        assert_eq!(proof.loops().len(), 1);
        assert_eq!(proof.loops()[0].preheader(), None);
    }

    #[test]
    fn entry_self_loop_has_no_preheader() {
        let proof = graph(&[&[0, 1], &[]], 100_000);
        assert!(proof.is_complete());
        assert_eq!(proof.loops()[0].members(), &[0]);
        assert_eq!(proof.loops()[0].preheader(), None);
        assert!(proof.contains(0));
        assert!(!proof.contains(1));
    }

    #[test]
    fn nested_loops_have_distinct_headers_and_full_outer_membership() {
        let proof = graph(&[&[1], &[2, 5], &[3, 4], &[2], &[1], &[]], 100_000);
        assert!(proof.is_complete());
        assert_eq!(proof.loops().len(), 2);
        assert_eq!(proof.loops()[0].header(), 1);
        assert_eq!(proof.loops()[0].members(), &[1, 2, 3, 4]);
        assert_eq!(proof.loops()[1].header(), 2);
        assert_eq!(proof.loops()[1].members(), &[2, 3]);
        assert!(!proof.node_dominates(2, 1));
        assert!(proof.node_dominates(1, 2));
    }

    #[test]
    fn dominance_matches_path_removal_on_all_three_block_graphs() {
        // An independent oracle: A dominates reachable B iff deleting A
        // disconnects B from entry. Includes forward/backward edges, self
        // loops, unreachable blocks, diamonds, and irreducible cycles.
        for bits in 0u32..(1 << 9) {
            let edges: Vec<Vec<usize>> = (0..3)
                .map(|a| (0..3).filter(|&b| bits & (1 << (3 * a + b)) != 0).collect())
                .collect();
            let refs: Vec<_> = edges.iter().map(Vec::as_slice).collect();
            let proof = graph(&refs, 100_000);
            if !proof.is_complete() {
                continue;
            }
            let reachable_without = |removed: Option<usize>, target: usize| {
                let mut seen = [false; 3];
                let mut pending = vec![0];
                while let Some(at) = pending.pop() {
                    if Some(at) == removed || seen[at] {
                        continue;
                    }
                    seen[at] = true;
                    pending.extend(&edges[at]);
                }
                seen[target]
            };
            for a in 0..3 {
                for b in 0..3 {
                    let expected = reachable_without(None, b) && !reachable_without(Some(a), b);
                    assert_eq!(
                        proof.dominates(a as u32, b as u32),
                        expected,
                        "graph={bits:09b}, dominator={a}, block={b}"
                    );
                }
            }
        }
    }

    #[test]
    fn malformed_target_and_late_exhaustion_discard_proofs() {
        assert!(!graph(&[&[2], &[]], 100_000).is_complete());
        let cfg: &[&[usize]] = &[&[1], &[2, 3], &[1], &[]];
        let mut saw_complete = false;
        for budget in 0..2_000 {
            let proof = graph(cfg, budget);
            if proof.is_complete() {
                saw_complete = true;
                assert_eq!(proof.loops().len(), 1);
            } else {
                assert!(proof.loops().is_empty());
                assert!(!proof.dominates(0, 0));
                assert!(!proof.contains(1));
            }
        }
        assert!(saw_complete);
    }

    #[test]
    fn nested_verified_loops_prove_membership_and_dominance() {
        let ir = translate("(function(n) { let s=0; for(let i=0;i<n;i++) { for(let j=0;j<n;j++) s+=i+j; } return s; })");
        let proof = LoopAnalysis::analyze(&ir, 1_000_000);
        assert!(proof.is_complete());
        assert_eq!(proof.loops().len(), 2);
        for natural in proof.loops() {
            assert!(natural.contains_block(natural.header()));
            for &member in natural.members() {
                assert!(proof.dominates(natural.header(), member));
                let block = ir.blocks().iter().find(|b| b.start_pc() == member).unwrap();
                for &node in block.nodes() {
                    assert!(proof.contains(node));
                    assert_eq!(proof.block_for_node(node), Some(member));
                    assert!(proof.node_dominates(node, node));
                }
            }
            if let Some(preheader) = natural.preheader() {
                assert!(!natural.contains_block(preheader));
                assert!(proof.dominates(preheader, natural.header()));
            }
        }
        assert!(!proof.contains(u32::MAX));
        assert!(!proof.dominates(u32::MAX, 0));
    }

    #[test]
    fn exhausted_work_discards_all_proofs() {
        let ir = translate("(function(n) { while(n>0) n--; return n; })");
        for budget in [0, 1, 4, 16] {
            let proof = LoopAnalysis::analyze(&ir, budget);
            assert!(!proof.is_complete());
            assert!(proof.loops().is_empty());
            assert!(!proof.dominates(0, 0));
            assert!(ir.nodes().iter().all(|n| !proof.contains(n.id())));
        }
    }
}
