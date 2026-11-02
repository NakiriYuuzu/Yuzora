//! Runtime-thread tiering coordination.

mod array_feedback;
mod background;
mod coordinator;
mod feedback;
mod hotness;
mod install;
mod invalidate;
mod osr;
mod shape_feedback;

pub use crate::compiler::CompileFailure;
pub use array_feedback::{
    ArrayAccess, ArrayFeedbackSnapshot, ArrayFeedbackTable, ArrayHazards, ArrayMode,
};
pub use background::{BackgroundCompiler, BackgroundCompilerError};
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
pub(crate) use coordinator::tier2_direct_call_site_usable;
#[cfg(feature = "test-support")]
pub use coordinator::CompletionDisposition;
pub use coordinator::{
    compile_and_send, ArtifactEnvironment, AttemptId, CompileCompletion, CompileRequest,
    CompileState, CompiledCallTarget, CompiledCallTargetError, CompletionDrain,
    CompletionSendError, CompletionSender, Coordinator, FunctionKey, GuardId, QueueError,
    SideExitAction, SidePathProfile, Tier, DEFAULT_COMPLETION_DRAIN_BUDGET,
};
#[cfg(all(feature = "compiler", not(target_family = "wasm")))]
pub use coordinator::{DirectCallTarget, FrameInlineTarget, NativeCallTarget};
pub use feedback::{
    BinaryFeedbackFlags, BinaryFeedbackSnapshot, BoundedSpecializationSignature,
    BranchFeedbackSnapshot, CallFeedbackSnapshot, CallLinkStatus, CallSignatureFeedbackSnapshot,
    CallSpecializationKey, ConversionFeedbackSnapshot, FeedbackKind, FeedbackRepresentation,
    FeedbackSnapshot, FeedbackSnapshotEntry, FeedbackState, FeedbackTable, ObservedType,
    MAX_SPECIALIZED_ARGUMENTS,
};
pub use hotness::{
    AdaptiveInputs, Decision, HotDecision, HotReason, HotThresholds, HotnessState, Profile,
    Profitability, ProfitabilityDecision, ProfitabilityRationale, BASE_CALL_THRESHOLD,
    BASE_LOOP_THRESHOLD,
};
pub use invalidate::{DependencyError, DependencyGraph, DependencyKey};
pub use osr::{OsrKey, OsrMap};
pub use shape_feedback::{
    PropertyAttributes, PrototypeDependencyToken, ShapeFeedbackSite, ShapeFeedbackState,
    ShapeFeedbackTable, ShapeObservation, ShapeToken, POLYMORPHIC_PROPERTY_LIMIT,
};

#[cfg(test)]
mod hotness_tests {
    use super::{
        AdaptiveInputs, Decision, HotDecision, HotReason, HotnessState, Profile, Profitability,
        ProfitabilityRationale, BASE_CALL_THRESHOLD, BASE_LOOP_THRESHOLD,
    };

    #[test]
    fn hotness_counters_saturate_and_submission_is_single_shot() {
        let mut hotness = HotnessState::default();
        hotness.record_calls(u32::MAX);
        hotness.record_calls(1);
        hotness.record_loops(7);
        hotness.record_exits(3);

        assert_eq!(hotness.calls(), u32::MAX);
        assert_eq!(hotness.loops(), 7);
        assert_eq!(hotness.exits(), 3);
        assert!(hotness.mark_queued());
        assert!(!hotness.mark_queued());
        hotness.clear_queued();
        assert!(hotness.mark_queued());
    }

    #[test]
    fn exact_default_call_and_loop_boundaries_queue_once() {
        let mut calls = HotnessState::default();
        assert_eq!(
            calls.record_call_event(BASE_CALL_THRESHOLD - 1),
            HotDecision::Cold
        );
        assert_eq!(
            calls.record_call_event(1),
            HotDecision::Queue(HotReason::CallThreshold)
        );
        assert_eq!(calls.record_call_event(1), HotDecision::AlreadyQueued);

        let mut loops = HotnessState::default();
        assert_eq!(
            loops.record_loop_event(BASE_LOOP_THRESHOLD - 1),
            HotDecision::Cold
        );
        assert_eq!(
            loops.record_loop_event(1),
            HotDecision::Queue(HotReason::LoopThreshold)
        );
        assert_eq!(loops.record_loop_event(1), HotDecision::AlreadyQueued);
    }

    #[test]
    fn eight_short_callbacks_do_not_queue_and_counters_saturate() {
        let mut hotness = HotnessState::default();
        for _ in 0..8 {
            assert_eq!(hotness.record_call_event(1), HotDecision::Cold);
        }
        hotness.record_calls(u32::MAX);
        hotness.record_loops(u32::MAX);
        assert_eq!(hotness.calls(), u32::MAX);
        assert_eq!(hotness.loops(), u32::MAX);
    }

    #[test]
    fn adaptive_thresholds_are_integer_deterministic_and_neutral_without_measurements() {
        let neutral = AdaptiveInputs::default().thresholds();
        assert_eq!(neutral.calls, BASE_CALL_THRESHOLD);
        assert_eq!(neutral.loops, BASE_LOOP_THRESHOLD);
        assert_eq!(neutral.rationale, HotReason::NeutralBase);
        assert_eq!(neutral, AdaptiveInputs::default().thresholds());
    }

    #[test]
    fn adaptive_thresholds_are_consumed_by_hotness_decisions() {
        let thresholds = AdaptiveInputs::default().thresholds();
        let mut hotness = HotnessState::default();
        assert_eq!(
            hotness.record_call_event_with_thresholds(thresholds.calls - 1, thresholds),
            HotDecision::Cold
        );
        assert_eq!(
            hotness.record_call_event_with_thresholds(1, thresholds),
            HotDecision::Queue(HotReason::CallThreshold)
        );
    }

    #[test]
    fn measured_profitability_keeps_host_heavy_code_at_baseline() {
        let profile = Profile {
            bytecodes: 100,
            helper_calls: 90,
            compile_ns: 80_000,
            install_ns: 10_000,
            executions: 40,
            interpreter_ns: 120_000,
            baseline_ns: 100_000,
            optimized_ns: 95_000,
            code_bytes: 512,
        };
        let decision = Profitability::default().evaluate(profile);
        assert_eq!(decision.tier, Decision::Baseline);
        assert_eq!(decision.rationale, ProfitabilityRationale::HelperDominated);
    }

    #[test]
    fn measured_profitability_optimizes_hot_numeric_code_after_break_even() {
        let profile = Profile {
            bytecodes: 20_000_000,
            helper_calls: 2,
            compile_ns: 120_000,
            install_ns: 10_000,
            executions: 20,
            interpreter_ns: 20_000_000,
            baseline_ns: 4_000_000,
            optimized_ns: 1_000_000,
            code_bytes: 2048,
        };
        let decision = Profitability::default().evaluate(profile);
        assert_eq!(decision.tier, Decision::Optimize);
        assert_eq!(
            decision.rationale,
            ProfitabilityRationale::PositiveAmortizedBenefit
        );
        assert!(decision.net_benefit_ns > 0);
        assert!(decision.break_even_executions <= profile.executions);
    }

    #[test]
    fn fixed_policy_is_deterministic_for_reports() {
        let profile = Profile::default();
        let policy = Profitability::fixed(Decision::Interpret);
        assert_eq!(policy.evaluate(profile).tier, Decision::Interpret);
        assert_eq!(
            policy.evaluate(profile).rationale,
            ProfitabilityRationale::FixedPolicy
        );
    }

    #[test]
    fn automatic_trial_requires_measured_baseline_and_positive_modeled_benefit() {
        let cold = Profile {
            bytecodes: 10_000,
            ..Profile::default()
        };
        assert_eq!(
            Profitability::default().evaluate_trial(cold).tier,
            Decision::Baseline
        );
        let measured = Profile {
            bytecodes: 10_000,
            helper_calls: 2,
            compile_ns: 10_000,
            executions: 16,
            baseline_ns: 1_600_000,
            code_bytes: 512,
            ..Profile::default()
        };
        let decision = Profitability::default().evaluate_trial(measured);
        assert_eq!(decision.tier, Decision::Optimize);
        assert_eq!(
            decision.rationale,
            ProfitabilityRationale::PositiveAmortizedBenefit
        );
    }

    #[test]
    fn cumulative_timings_are_normalized_once() {
        let profile = Profile {
            bytecodes: 10_000,
            compile_ns: 100,
            executions: 10,
            interpreter_ns: 1_000,
            baseline_ns: 800,
            optimized_ns: 400,
            code_bytes: 10,
            ..Profile::default()
        };
        let decision = Profitability::default().evaluate(profile);
        assert_eq!(decision.gross_benefit_ns, 400);
        assert_eq!(decision.net_benefit_ns, 300);
    }
}
