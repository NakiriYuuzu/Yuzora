//! Known value identities for speculative optimization.
//!
//! Following Maglev's known-node information, facts describe SSA values, not
//! bytecode adjacency. Joins retain a fact only when every incoming value agrees.
//! The unreached lattice element lets a seeded loop converge without allowing a
//! seedless cycle to manufacture a proof. Heap effects will use the same value
//! identities; a frame slot alone is never an object identity.

use super::{FrameSlot, OptimizedNode, OptimizedNodeKind, ScalarGraph, ScalarValue, ScalarValueId};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Identity {
    #[default]
    Unreached,
    Argument(u16),
    /// The defining SSA value after resolving agreeing Phis and preserved
    /// frame reads. Only equality is meaningful; it never names an argument.
    Value(u32),
    Unknown,
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::{bytecode::VerifyLimits, ir::OptimizedIr, test_support::SnapshotFixture};

    #[test]
    fn guarded_frame_aliases_require_the_exact_nonreentrant_operation() {
        for (source, expected_guarded, expected_generic) in [
            ("(function(a,b){let x=a.x;return a.y})", Some(0), None),
            ("(function(a,b){let x=a.x;a=b;return a.y})", Some(1), None),
            ("(function(a,b){b();return a.y})", None, None),
            ("(function(a,b){a=b;return a.y})", Some(1), Some(1)),
        ] {
            let fixture = SnapshotFixture::compile(source);
            let verified = fixture.snapshot().verify(VerifyLimits::default()).unwrap();
            let ir = OptimizedIr::translate(&verified, 605).unwrap();
            let graph = ir.scalar_graph();
            let base = ir
                .nodes()
                .iter()
                .filter_map(|node| graph.heap_operation(node.id()))
                .next_back()
                .unwrap()
                .location()
                .base;
            let guarded = KnownFacts::analyze_with_preserved_frame_reads(
                graph,
                ir.nodes(),
                |node| graph.heap_operation(node).is_some(),
                100_000,
            );
            assert_eq!(guarded.entry_argument(base), expected_guarded, "{source}");
            assert_eq!(
                KnownFacts::analyze(graph, ir.nodes(), false, 100_000).entry_argument(base),
                expected_generic,
                "{source}"
            );
            let exhausted =
                KnownFacts::analyze_with_preserved_frame_reads(graph, ir.nodes(), |_| true, 0);
            assert_eq!(exhausted.entry_argument(base), None);
        }
    }

    #[test]
    fn identity_flows_only_through_agreeing_phi_and_revalidated_poll_aliases() {
        let property_base = |source: &str| {
            let fixture = SnapshotFixture::compile(source);
            let verified = fixture.snapshot().verify(VerifyLimits::default()).unwrap();
            let ir = OptimizedIr::translate(&verified, 606).unwrap();
            let graph = ir.scalar_graph();
            let base = ir
                .nodes()
                .iter()
                .filter_map(|node| graph.heap_operation(node.id()))
                .next_back()
                .unwrap()
                .location()
                .base;
            (ir, base)
        };

        let (ir, base) =
            property_base("(function(flag,a){let object=a;if(flag)object=a;return object.x})");
        assert_eq!(
            KnownFacts::analyze(ir.scalar_graph(), ir.nodes(), false, 100_000).entry_argument(base),
            Some(1),
            "all Phi inputs preserve the same entry identity"
        );

        let (ir, base) =
            property_base("(function(flag,a,b){let object=a;if(flag)object=b;return object.x})");
        assert_eq!(
            KnownFacts::analyze(ir.scalar_graph(), ir.nodes(), false, 100_000).entry_argument(base),
            None,
            "mixed predecessor identities must intersect to unknown"
        );

        let (ir, _) = property_base("(function(a,n){while(n>0)n=n-1;return a.x})");
        let poll = ir
            .nodes()
            .iter()
            .find(|node| {
                matches!(
                    node.kind(),
                    OptimizedNodeKind::GuardNumeric { mid_loop: true, .. }
                )
            })
            .unwrap()
            .id();
        let input = ScalarValueId::for_test(0);
        let poll_alias = ScalarValueId::for_test(1);
        let mut states = vec![None; poll as usize + 1];
        states[poll as usize] = Some(super::super::ScalarFrameState {
            pc: ir.nodes()[poll as usize].pc(),
            arguments: vec![input].into(),
            locals: Box::new([]),
            stack: Box::new([]),
            parent: None,
        });
        let graph = ScalarGraph::for_test(
            vec![
                ScalarValue::Input {
                    block_pc: 0,
                    slot: FrameSlot::Argument(0),
                },
                ScalarValue::FrameRead {
                    node: poll,
                    slot: FrameSlot::Argument(0),
                },
            ],
            states,
        );
        assert_eq!(
            KnownFacts::analyze(&graph, ir.nodes(), false, 100_000).entry_argument(poll_alias),
            None,
            "a generic analysis cannot trust the frame reload after a poll"
        );
        assert_eq!(
            KnownFacts::analyze(&graph, ir.nodes(), true, 100_000).entry_argument(poll_alias),
            Some(0),
            "a consumer that revalidates each poll may preserve the alias"
        );
    }

    #[test]
    fn independent_seedless_cycle_invalidates_a_seeded_phi_identity() {
        let graph = ScalarGraph::for_test(
            vec![
                ScalarValue::Input {
                    block_pc: 0,
                    slot: FrameSlot::Argument(2),
                },
                ScalarValue::Phi {
                    block_pc: 1,
                    slot: FrameSlot::Local(0),
                    inputs: vec![
                        super::super::ScalarPhiInput {
                            predecessor: Some(0),
                            value: ScalarValueId::for_test(0),
                        },
                        super::super::ScalarPhiInput {
                            predecessor: Some(1),
                            value: ScalarValueId::for_test(2),
                        },
                    ]
                    .into(),
                },
                ScalarValue::Phi {
                    block_pc: 1,
                    slot: FrameSlot::Local(1),
                    inputs: vec![super::super::ScalarPhiInput {
                        predecessor: Some(1),
                        value: ScalarValueId::for_test(2),
                    }]
                    .into(),
                },
            ],
            Vec::new(),
        );

        assert_eq!(
            KnownFacts::analyze(&graph, &[], false, 100).entry_argument(ScalarValueId::for_test(1)),
            None,
            "an independently seedless incoming SCC is an unknown edge"
        );
    }
}

impl Identity {
    fn join(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unreached, value) | (value, Self::Unreached) => value,
            (a, b) if a == b => a,
            _ => Self::Unknown,
        }
    }
}

pub(crate) struct KnownFacts {
    identities: Vec<Identity>,
}

impl KnownFacts {
    /// Poll aliases are admitted only when the consumer republishes/revalidates
    /// its guarded facts at each actual poll. Unknown calls never preserve them.
    /// Exhausting the work budget discards the entire partially computed proof.
    pub(crate) fn analyze(
        graph: &ScalarGraph,
        nodes: &[OptimizedNode],
        preserve_poll_bindings: bool,
        work: usize,
    ) -> Self {
        Self::analyze_with_preserved_frame_reads(
            graph,
            nodes,
            |node| {
                preserve_poll_bindings
                    && nodes.get(node as usize).is_some_and(|node| {
                        matches!(node.kind(), OptimizedNodeKind::GuardNumeric { .. })
                    })
            },
            work,
        )
    }

    /// A caller may preserve a frame read only when the selected native path
    /// cannot reenter or mutate that frame (or when poll state is revalidated).
    /// The predicate is an emitted-guard contract, never a feedback-only hint.
    pub(crate) fn analyze_with_preserved_frame_reads(
        graph: &ScalarGraph,
        nodes: &[OptimizedNode],
        preserve: impl Fn(u32) -> bool,
        mut work: usize,
    ) -> Self {
        let mut identities = vec![Identity::Unreached; graph.values().len()];
        loop {
            let mut changed = false;
            for (index, value) in graph.values().iter().enumerate() {
                let cost = match value {
                    ScalarValue::Phi { inputs, .. } => inputs.len().max(1),
                    _ => 1,
                };
                let Some(left) = work.checked_sub(cost) else {
                    identities.fill(Identity::Unknown);
                    return Self { identities };
                };
                work = left;
                let next = match value {
                    ScalarValue::Input {
                        block_pc: 0,
                        slot: FrameSlot::Argument(argument),
                    } => Identity::Argument(*argument),
                    ScalarValue::Phi { inputs, .. } => {
                        inputs.iter().fold(Identity::Unreached, |known, input| {
                            known.join(identities[input.value.index()])
                        })
                    }
                    ScalarValue::FrameRead { node, slot }
                        if nodes.get(*node as usize).is_some() && preserve(*node) =>
                    {
                        graph
                            .frame_state_for_node(*node)
                            .and_then(|state| match slot {
                                FrameSlot::Argument(index) => {
                                    state.arguments.get(usize::from(*index))
                                }
                                FrameSlot::Local(index) => state.locals.get(usize::from(*index)),
                                FrameSlot::Stack(index) => state.stack.get(usize::from(*index)),
                            })
                            .map_or(Identity::Unknown, |source| identities[source.index()])
                    }
                    // Every other producer defines its own identity. An
                    // unpreserved frame read is a fresh definition as well.
                    _ => u32::try_from(index).map_or(Identity::Unknown, Identity::Value),
                };
                let next = identities[index].join(next);
                changed |= identities[index] != next;
                identities[index] = next;
            }
            if !changed {
                // `Unreached` is the optimistic lattice bottom used to let a
                // seeded loop converge. At the first fixed point, any value
                // still at bottom belongs to a seedless component. Turn those
                // components into Unknown and iterate once more so consumers
                // cannot ignore an independently seedless incoming Phi edge.
                // This mirrors the fail-closed second phase used by the
                // representation analysis in `ScalarGraph`.
                if identities.contains(&Identity::Unreached) {
                    let Some(left) = work.checked_sub(identities.len()) else {
                        identities.fill(Identity::Unknown);
                        return Self { identities };
                    };
                    work = left;
                    for identity in &mut identities {
                        if *identity == Identity::Unreached {
                            *identity = Identity::Unknown;
                        }
                    }
                    continue;
                }
                return Self { identities };
            }
        }
    }

    pub(crate) fn entry_argument(&self, value: ScalarValueId) -> Option<u16> {
        match self.identities.get(value.index())? {
            Identity::Argument(argument) => Some(*argument),
            _ => None,
        }
    }

    /// Both SSA values denote the same definition on every path: an entry
    /// argument, or one producer reached only through agreeing Phis and frame
    /// reads preserved by the caller's emitted-guard contract.
    pub(crate) fn same_value(&self, left: ScalarValueId, right: ScalarValueId) -> bool {
        match (
            self.identities.get(left.index()),
            self.identities.get(right.index()),
        ) {
            (Some(Identity::Argument(a)), Some(Identity::Argument(b))) => a == b,
            (Some(Identity::Value(a)), Some(Identity::Value(b))) => a == b,
            _ => false,
        }
    }

    pub(crate) const fn bytes_per_value() -> usize {
        core::mem::size_of::<Identity>()
    }
}
