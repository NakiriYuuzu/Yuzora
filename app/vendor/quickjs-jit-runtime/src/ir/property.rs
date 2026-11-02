//! Guarded own-property cache planning for production SSA lowering.
//!
//! The plan authorizes only lazy guarded accesses. The backend must materialize
//! dirty values before every deopt (including arithmetic overflow), helper,
//! safepoint and return, and invalidate values after reentrant boundaries.

use std::collections::{BTreeMap, BTreeSet};

use super::{FrameSlot, KnownFacts, OptimizedIr, ScalarHeapEffect, ScalarHeapOperation};
use crate::runtime::{ObservedType, PropertyAttributes, ShapeObservation};

const MAX_CANDIDATES: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PropertyCandidate {
    pub argument: u16,
    pub atom: u32,
    pub observation: ShapeObservation,
}

#[derive(Clone, Debug)]
pub(crate) struct PropertyAccessPlan {
    pub candidate: usize,
    pub store: bool,
    /// Flush and invalidate these entries before this access: distinct
    /// argument numbers do not establish that the receiver objects differ.
    pub aliases: Box<[usize]>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PropertyBoundary {
    None,
    Flush,
    #[default]
    FlushInvalidate,
}

#[derive(Debug, Default)]
pub(crate) struct PropertyPlan {
    candidates: Vec<PropertyCandidate>,
    accesses: Vec<Option<PropertyAccessPlan>>,
    boundaries: Vec<PropertyBoundary>,
}

impl PropertyPlan {
    pub(crate) fn analyze(
        ir: &OptimizedIr,
        facts: &KnownFacts,
        guarded_sites: &BTreeMap<u32, Box<[ShapeObservation]>>,
        non_owning_frame_write: impl Fn(u32) -> bool,
        mut work: usize,
    ) -> Self {
        let nodes = ir.nodes();
        // Charge traversal and allocation before constructing partial state.
        let Some(remaining) = work.checked_sub(nodes.len().saturating_mul(3)) else {
            return Self::default();
        };
        work = remaining;
        let graph = ir.scalar_graph();
        let changed_arguments: BTreeSet<_> = nodes
            .iter()
            .filter_map(|node| match graph.effect_for_node(node.id()) {
                ScalarHeapEffect::FrameWrite(FrameSlot::Argument(argument)) => Some(argument),
                _ => None,
            })
            .collect();
        // The boolean marks conflicting feedback for an otherwise identical
        // location. Reject the entire location instead of choosing a stale site.
        let mut contracts: Vec<(PropertyCandidate, bool)> = Vec::new();
        let mut accesses = vec![None; nodes.len()];
        for node in nodes {
            if node.eliminated() {
                continue;
            }
            let Some(operation) = graph.heap_operation(node.id()) else {
                continue;
            };
            let (base, atom, store) = match *operation {
                ScalarHeapOperation::GetProperty { base, atom, .. } => (base, atom, false),
                ScalarHeapOperation::PutProperty { base, atom, .. } => (base, atom, true),
                _ => continue,
            };
            let Some(argument) = facts.entry_argument(base) else {
                continue;
            };
            if changed_arguments.contains(&argument) {
                continue;
            }
            let Some(observations) = guarded_sites.get(&node.pc()) else {
                continue;
            };
            let [observation] = observations.as_ref() else {
                continue;
            };
            if !eligible_property_observation(*observation)
                || graph
                    .frame_state_for_node(operation.frame_state_node())
                    .is_none()
            {
                continue;
            }
            let Some(remaining) = work.checked_sub(contracts.len().saturating_add(1)) else {
                return Self::default();
            };
            work = remaining;
            let candidate = if let Some(index) = contracts
                .iter()
                .position(|(candidate, _)| candidate.argument == argument && candidate.atom == atom)
            {
                contracts[index].1 |= contracts[index].0.observation != *observation;
                index
            } else {
                if contracts.len() == MAX_CANDIDATES {
                    return Self::default();
                }
                contracts.push((
                    PropertyCandidate {
                        argument,
                        atom,
                        observation: *observation,
                    },
                    false,
                ));
                contracts.len() - 1
            };
            let Some(access) = accesses.get_mut(node.id() as usize) else {
                return Self::default();
            };
            *access = Some((candidate, store));
        }
        let mut plan = Self::default();
        let mut remap = vec![None; contracts.len()];
        for (index, (candidate, conflicting)) in contracts.iter().enumerate() {
            if !conflicting {
                remap[index] = Some(plan.candidates.len());
                plan.candidates.push(*candidate);
            }
        }
        if plan.candidates.is_empty() {
            return Self::default();
        }
        plan.accesses = vec![None; nodes.len()];
        plan.boundaries = vec![PropertyBoundary::FlushInvalidate; nodes.len()];
        for node in nodes {
            let Some(remaining) = work.checked_sub(plan.candidates.len().saturating_add(1)) else {
                return Self::default();
            };
            work = remaining;
            let id = node.id() as usize;
            if id >= nodes.len() {
                return Self::default();
            }
            if let Some((candidate, store)) = accesses[id] {
                if let Some(candidate) = remap[candidate] {
                    let atom = plan.candidates[candidate].atom;
                    let aliases = plan
                        .candidates
                        .iter()
                        .enumerate()
                        .filter_map(|(index, other)| {
                            (index != candidate && other.atom == atom).then_some(index)
                        })
                        .collect();
                    plan.accesses[id] = Some(PropertyAccessPlan {
                        candidate,
                        store,
                        aliases,
                    });
                    plan.boundaries[id] = PropertyBoundary::None;
                    continue;
                }
            }
            plan.boundaries[id] = if node.eliminated() {
                PropertyBoundary::None
            } else {
                match graph.effect_for_node(node.id()) {
                    ScalarHeapEffect::Pure => PropertyBoundary::None,
                    ScalarHeapEffect::Exit => PropertyBoundary::Flush,
                    ScalarHeapEffect::FrameWrite(_) if non_owning_frame_write(node.id()) => {
                        PropertyBoundary::None
                    }
                    _ => PropertyBoundary::FlushInvalidate,
                }
            };
        }
        plan
    }

    pub(crate) fn bytes_upper_bound(nodes: usize) -> usize {
        // Includes transient contract/remap vectors, the changed-root tree,
        // provisional node accesses and worst-case per-access alias vectors.
        nodes.saturating_mul(512).saturating_add(8192)
    }

    pub(crate) fn candidates(&self) -> &[PropertyCandidate] {
        &self.candidates
    }

    pub(crate) fn access(&self, node: u32) -> Option<&PropertyAccessPlan> {
        self.accesses.get(node as usize)?.as_ref()
    }

    pub(crate) fn before_node(&self, node: u32) -> PropertyBoundary {
        self.boundaries
            .get(node as usize)
            .copied()
            .unwrap_or_default()
    }
}

pub(crate) fn eligible_property_observation(observation: ShapeObservation) -> bool {
    observation.shape().identity() != 0
        && observation.shape().generation() != 0
        && observation.prototype().identity() == 0
        && observation.prototype().generation() == 0
        && observation.offset() <= (i32::MAX as u32 - 8) / 16
        && observation
            .attributes()
            .contains(PropertyAttributes::WRITABLE)
        && !observation
            .attributes()
            .contains(PropertyAttributes::ACCESSOR)
        && matches!(
            observation.value(),
            ObservedType::Int32
                | ObservedType::Float64
                | ObservedType::Bool
                | ObservedType::Null
                | ObservedType::Undefined
        )
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::{
        bytecode::VerifyLimits,
        ir::{ScalarHeapEffect, ScalarHeapOperation},
        runtime::{ObservedType, PropertyAttributes, PrototypeDependencyToken, ShapeToken},
        test_support::SnapshotFixture,
    };

    fn observation(offset: u32) -> ShapeObservation {
        ShapeObservation::new(
            ShapeToken::new(123, 1),
            PrototypeDependencyToken::new(0, 0),
            offset,
            PropertyAttributes::WRITABLE,
            ObservedType::Int32,
        )
    }

    fn fixture(source: &str) -> (OptimizedIr, BTreeMap<u32, Box<[ShapeObservation]>>) {
        let fixture = SnapshotFixture::compile(source);
        let verified = fixture.snapshot().verify(VerifyLimits::default()).unwrap();
        let ir = OptimizedIr::translate(&verified, 1).unwrap();
        let mut atoms = BTreeMap::new();
        let mut sites = BTreeMap::new();
        for node in ir.nodes() {
            if let Some(
                ScalarHeapOperation::GetProperty { atom, .. }
                | ScalarHeapOperation::PutProperty { atom, .. },
            ) = ir.scalar_graph().heap_operation(node.id())
            {
                let next = atoms.len() as u32;
                let offset = *atoms.entry(*atom).or_insert(next);
                sites.insert(node.pc(), vec![observation(offset)].into_boxed_slice());
            }
        }
        (ir, sites)
    }

    fn plan(
        ir: &OptimizedIr,
        sites: &BTreeMap<u32, Box<[ShapeObservation]>>,
        work: usize,
    ) -> PropertyPlan {
        let facts = KnownFacts::analyze_with_preserved_frame_reads(
            ir.scalar_graph(),
            ir.nodes(),
            |id| {
                sites.contains_key(&ir.nodes()[id as usize].pc())
                    || matches!(
                        ir.scalar_graph().effect_for_node(id),
                        ScalarHeapEffect::Safepoint
                    )
            },
            100_000,
        );
        PropertyPlan::analyze(ir, &facts, sites, |_| false, work)
    }

    #[test]
    fn same_atom_arguments_may_alias_but_guarded_distinct_fields_do_not() {
        let (ir, sites) = fixture("(function(a,b) { return a.x + b.x + a.y; })");
        let plan = plan(&ir, &sites, 100_000);
        assert_eq!(plan.candidates().len(), 3);
        let accesses: Vec<_> = ir
            .nodes()
            .iter()
            .filter_map(|node| plan.access(node.id()))
            .collect();
        assert_eq!(accesses.len(), 3);
        assert_eq!(accesses[0].aliases.as_ref(), &[accesses[1].candidate]);
        assert_eq!(accesses[1].aliases.as_ref(), &[accesses[0].candidate]);
        assert!(accesses[2].aliases.is_empty());
        for node in ir
            .nodes()
            .iter()
            .filter(|node| plan.access(node.id()).is_some())
        {
            assert_eq!(plan.before_node(node.id()), PropertyBoundary::None);
        }
    }

    #[test]
    fn unknown_property_and_poll_are_world_boundaries() {
        let (ir, mut sites) = fixture(
            "(function(a,n) { a.x = 1; for(let i=0;i<n;i++) { a.x = 2; a.y; } return a.x; })",
        );
        let unknown = ir
            .nodes()
            .iter()
            .find(|node| {
                matches!(
                    ir.scalar_graph().heap_operation(node.id()),
                    Some(ScalarHeapOperation::GetProperty { .. })
                )
            })
            .unwrap();
        sites.remove(&unknown.pc());
        let plan = plan(&ir, &sites, 100_000);
        assert!(!plan.candidates().is_empty());
        assert_eq!(
            plan.before_node(unknown.id()),
            PropertyBoundary::FlushInvalidate
        );
        assert!(plan.access(unknown.id()).is_none());
        let polls: Vec<_> = ir
            .nodes()
            .iter()
            .filter(|node| {
                ir.scalar_graph().effect_for_node(node.id()) == ScalarHeapEffect::Safepoint
            })
            .collect();
        assert!(!polls.is_empty());
        for node in polls {
            assert_eq!(
                plan.before_node(node.id()),
                PropertyBoundary::FlushInvalidate
            );
        }
        let exit = ir
            .nodes()
            .iter()
            .find(|node| ir.scalar_graph().effect_for_node(node.id()) == ScalarHeapEffect::Exit)
            .unwrap();
        assert_eq!(plan.before_node(exit.id()), PropertyBoundary::Flush);
    }

    #[test]
    fn argument_reassignment_rejects_even_an_earlier_cached_receiver() {
        let (ir, sites) = fixture("(function(a,b) { a.x = 2; a = b; return a.x; })");
        let plan = plan(&ir, &sites, 100_000);
        assert!(plan
            .candidates()
            .iter()
            .all(|candidate| candidate.argument != 0));
        let first_store = ir
            .nodes()
            .iter()
            .find(|node| {
                matches!(
                    ir.scalar_graph().heap_operation(node.id()),
                    Some(ScalarHeapOperation::PutProperty { .. })
                )
            })
            .unwrap();
        assert!(plan.access(first_store.id()).is_none());
    }

    #[test]
    fn inconsistent_layout_and_nonprimitive_feedback_cannot_share_a_cache() {
        let (ir, mut sites) = fixture("(function(a) { a.x = 2; return a.x; })");
        assert_eq!(plan(&ir, &sites, 100_000).candidates().len(), 1);
        let pc = *sites.keys().last().unwrap();
        sites.insert(pc, vec![observation(99)].into_boxed_slice());
        assert!(plan(&ir, &sites, 100_000).candidates().is_empty());
        for feedback in sites.values_mut() {
            *feedback = vec![ShapeObservation::new(
                ShapeToken::new(123, 1),
                PrototypeDependencyToken::new(0, 0),
                0,
                PropertyAttributes::WRITABLE,
                ObservedType::Object,
            )]
            .into_boxed_slice();
        }
        assert!(plan(&ir, &sites, 100_000).candidates().is_empty());
    }

    #[test]
    fn exhausted_budget_discards_partial_candidates_and_actions() {
        let (ir, sites) = fixture("(function(a,b) { a.x = 2; b.x = 3; return a.y; })");
        assert!(!plan(&ir, &sites, 100_000).candidates().is_empty());
        let mut completed = false;
        for budget in 0..10_000 {
            let plan = plan(&ir, &sites, budget);
            if !plan.candidates().is_empty() {
                assert_eq!(plan.candidates().len(), 3);
                completed = true;
                break;
            }
            assert!(plan.candidates().is_empty());
            assert!(ir
                .nodes()
                .iter()
                .all(|node| plan.access(node.id()).is_none()));
        }
        assert!(completed);
    }

    #[test]
    fn stable_xy_loop_uses_the_same_candidates_at_every_guarded_access() {
        let (ir, sites) = fixture("(function(n, seed, point) { point.x=seed; point.y=1; let toggle=0; for(let i=0;i<n;i++) { point.x=point.x+point.y; point.y=point.y+toggle; toggle=1-toggle; } return point.x+point.y; })");
        let plan = plan(&ir, &sites, 100_000);
        assert_eq!(plan.candidates().len(), 2);
        assert!(plan
            .candidates()
            .iter()
            .all(|candidate| candidate.argument == 2));
        let mut stores = 0;
        let mut loads = 0;
        for node in ir.nodes() {
            if sites.contains_key(&node.pc()) {
                let access = plan
                    .access(node.id())
                    .expect("stable property must use its SSA cache");
                assert_eq!(
                    plan.before_node(node.id()),
                    PropertyBoundary::None,
                    "a guarded field access must update/read the virtual field; \
                     materializing at each bytecode would only be a read cache"
                );
                assert!(access.aliases.is_empty());
                if access.store {
                    stores += 1;
                } else {
                    loads += 1;
                }
            }
        }
        assert_eq!(stores, 4);
        assert_eq!(loads, 5);

        // The loop still has a real observable poll.  It is the publication
        // boundary for all dirty x/y fields, rather than either put_field.
        // This distinction is the essential write-sinking invariant used by
        // V8/JSC virtual-object lowering.
        assert!(ir.nodes().iter().any(|node| {
            ir.scalar_graph().effect_for_node(node.id()) == ScalarHeapEffect::Safepoint
                && plan.before_node(node.id()) == PropertyBoundary::FlushInvalidate
        }));
    }

    #[test]
    fn frame_writes_need_a_nonowning_proof_before_retaining_fields() {
        let (ir, sites) = fixture("(function(a,v) { a.x=1; let local=v; return a.x+local; })");
        let facts = KnownFacts::analyze_with_preserved_frame_reads(
            ir.scalar_graph(),
            ir.nodes(),
            |id| sites.contains_key(&ir.nodes()[id as usize].pc()),
            100_000,
        );
        let conservative = PropertyPlan::analyze(&ir, &facts, &sites, |_| false, 100_000);
        let numeric = PropertyPlan::analyze(&ir, &facts, &sites, |_| true, 100_000);
        let writes: Vec<_> = ir
            .nodes()
            .iter()
            .filter(|node| {
                matches!(
                    ir.scalar_graph().effect_for_node(node.id()),
                    ScalarHeapEffect::FrameWrite(FrameSlot::Local(_))
                )
            })
            .collect();
        assert!(!writes.is_empty());
        for node in writes {
            assert_eq!(
                conservative.before_node(node.id()),
                PropertyBoundary::FlushInvalidate
            );
            assert_eq!(numeric.before_node(node.id()), PropertyBoundary::None);
        }
    }

    #[test]
    fn accessor_prototype_readonly_and_dead_shape_contracts_are_rejected() {
        let (ir, mut sites) = fixture("(function(a) { a.x=1; return a.x; })");
        for observation in [
            ShapeObservation::new(
                ShapeToken::new(1, 1),
                PrototypeDependencyToken::new(0, 0),
                0,
                PropertyAttributes::WRITABLE | PropertyAttributes::ACCESSOR,
                ObservedType::Int32,
            ),
            ShapeObservation::new(
                ShapeToken::new(1, 1),
                PrototypeDependencyToken::new(3, 1),
                0,
                PropertyAttributes::WRITABLE,
                ObservedType::Int32,
            ),
            ShapeObservation::new(
                ShapeToken::new(1, 1),
                PrototypeDependencyToken::new(0, 0),
                0,
                PropertyAttributes::NONE,
                ObservedType::Int32,
            ),
            ShapeObservation::new(
                ShapeToken::new(1, 0),
                PrototypeDependencyToken::new(0, 0),
                0,
                PropertyAttributes::WRITABLE,
                ObservedType::Int32,
            ),
        ] {
            for site in sites.values_mut() {
                *site = vec![observation].into_boxed_slice();
            }
            assert!(plan(&ir, &sites, 100_000).candidates().is_empty());
        }
    }
}
