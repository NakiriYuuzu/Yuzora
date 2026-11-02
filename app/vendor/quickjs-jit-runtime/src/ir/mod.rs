//! A compact, target-independent baseline IR.

mod baseline;
mod facts;
mod frame_state;
mod heap;
mod inlining;
mod loops;
mod optimized;
mod property;
mod ranges;
mod scalar;
pub use inlining::{
    FrameInlineCallee, InlineCallKind, InlineCallee, InlineContinuation, InlineFrameBoundary,
    ScalarFrameInlineRegion,
};
mod types;

#[cfg(feature = "test-support")]
pub(crate) use baseline::with_execution_trace;
pub(crate) use baseline::MAX_HELPER_SCRATCH_SLOTS;
pub use baseline::{BaselineIr, IrBlock, IrCatchSite, IrExceptionHandler, IrInstruction};
pub(crate) use facts::KnownFacts;
pub(crate) use frame_state::FrameStateKind;
pub use frame_state::{FrameSlot, FrameState, FrameStateId, FrameStateTable};
pub use heap::{HeapKey, HeapLocation, ScalarHeapEffect, ScalarHeapOperation};
pub(crate) use loops::LoopAnalysis;
pub use optimized::{
    optimized_vocabulary_admits, DeoptMap, DeoptOwnership, DeoptPhase, DeoptSlot,
    DeoptValidationError, GuardSite, Materialization, MaterializedFrame, MaterializedValue,
    OptimizedBlock, OptimizedEffect, OptimizedFrameShape, OptimizedIr, OptimizedMetrics,
    OptimizedNode, OptimizedNodeKind, OwnedMaterializeError, OwnedMaterializedFrame,
    OwnedMaterializedValue, OwnershipTransitionError, SsaValueOwnership, ValueRepresentation,
};
pub(crate) use property::{
    eligible_property_observation, PropertyAccessPlan, PropertyBoundary, PropertyCandidate,
    PropertyPlan,
};
pub use ranges::IntegerRangeAnalysis;
pub use scalar::{
    ScalarBinaryOp, ScalarBitwiseOp, ScalarCall, ScalarCheck, ScalarCompareOp, ScalarFrameState,
    ScalarGraph, ScalarInlineRegion, ScalarInlineStep, ScalarNumericMode, ScalarPhiInput,
    ScalarValue, ScalarValueId,
};
pub use types::{
    BinaryOp, GenericOp, IrOp, IteratorOp, PollKind, StackOp, TaggedValue, UnaryOp, VarRefMode,
};

pub(crate) use optimized::optimized_opcodes_supported;
pub(crate) use scalar::stack_permutation;
