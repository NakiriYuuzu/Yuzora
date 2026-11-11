//! Abstract heap effects for semantic SSA operations.
//!
//! As in JSC's clobberize model, a generic JavaScript property access clobbers
//! the world. Only a guarded, non-reentrant own-data access may use the narrower
//! location effect. Distinct SSA bases are not evidence of distinct objects.

use super::{FrameSlot, ScalarValueId};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum HeapKey {
    Property(u32),
    /// The original JavaScript index, before any property-key coercion.
    Element(ScalarValueId),
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct HeapLocation {
    pub base: ScalarValueId,
    pub key: HeapKey,
}

impl HeapLocation {
    /// Valid for guarded own data accesses only. Unknown indices can denote
    /// named properties, and different SSA indices can have equal values.
    pub fn may_alias(self, other: Self) -> bool {
        !matches!((self.key, other.key), (HeapKey::Property(a), HeapKey::Property(b)) if a != b)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScalarHeapEffect {
    Pure,
    /// A local/argument assignment. Dropping the previous value can release an
    /// object; consumers need ownership proofs before moving stores across it.
    FrameWrite(FrameSlot),
    Read(HeapLocation),
    Write(HeapLocation),
    /// Unknown calls, accessors, Proxies, coercion and reference finalization.
    Reentrant,
    /// Polls can observe heap state and invalidate speculative dependencies.
    Safepoint,
    /// Returning exposes outstanding writes to the caller.
    Exit,
}

impl ScalarHeapEffect {
    /// Heap facts survive only effects known not to overwrite their location.
    /// A frame write alone says nothing about the released value's ownership.
    pub fn invalidates(self, location: HeapLocation) -> bool {
        match self {
            Self::Pure | Self::Read(_) | Self::Exit => false,
            Self::Write(written) => written.may_alias(location),
            Self::FrameWrite(_) | Self::Reentrant | Self::Safepoint => true,
        }
    }

    pub fn observes_heap(self) -> bool {
        !matches!(self, Self::Pure | Self::Write(_))
    }
}

/// Effectful operations are indexed by instruction, independently of whether
/// they produce a value. Stores must never disappear from the semantic graph
/// merely because the bytecode has zero stack outputs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScalarHeapOperation {
    GetProperty {
        base: ScalarValueId,
        atom: u32,
        result: ScalarValueId,
        frame_state_node: u32,
    },
    PutProperty {
        base: ScalarValueId,
        atom: u32,
        value: ScalarValueId,
        frame_state_node: u32,
    },
    GetElement {
        base: ScalarValueId,
        index: ScalarValueId,
        result: ScalarValueId,
        frame_state_node: u32,
    },
    PutElement {
        base: ScalarValueId,
        index: ScalarValueId,
        value: ScalarValueId,
        frame_state_node: u32,
    },
}

impl ScalarHeapOperation {
    pub fn location(self) -> HeapLocation {
        match self {
            Self::GetProperty { base, atom, .. } | Self::PutProperty { base, atom, .. } => {
                HeapLocation {
                    base,
                    key: HeapKey::Property(atom),
                }
            }
            Self::GetElement { base, index, .. } | Self::PutElement { base, index, .. } => {
                HeapLocation {
                    base,
                    key: HeapKey::Element(index),
                }
            }
        }
    }

    pub fn frame_state_node(self) -> u32 {
        match self {
            Self::GetProperty {
                frame_state_node, ..
            }
            | Self::PutProperty {
                frame_state_node, ..
            }
            | Self::GetElement {
                frame_state_node, ..
            }
            | Self::PutElement {
                frame_state_node, ..
            } => frame_state_node,
        }
    }

    /// Conditional contract, not a proof: lowering must guard own data slots,
    /// non-exotic receivers, keys, shape, bounds and ownership as appropriate.
    /// Stores must neither transition shape nor grow arrays, invoke setters,
    /// release a potentially finalizable value, or access detached buffers.
    /// Any failed condition exits using the operation's pre-effect frame state.
    pub fn guarded_effect(self) -> ScalarHeapEffect {
        match self {
            Self::GetProperty { .. } | Self::GetElement { .. } => {
                ScalarHeapEffect::Read(self.location())
            }
            Self::PutProperty { .. } | Self::PutElement { .. } => {
                ScalarHeapEffect::Write(self.location())
            }
        }
    }
}
