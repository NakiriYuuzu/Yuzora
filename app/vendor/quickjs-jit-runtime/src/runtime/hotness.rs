//! Bounded, deterministic hotness bookkeeping.

pub const BASE_CALL_THRESHOLD: u32 = 32;
pub const BASE_LOOP_THRESHOLD: u32 = 56;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Profile {
    pub bytecodes: u64,
    pub helper_calls: u64,
    pub compile_ns: u64,
    pub install_ns: u64,
    pub executions: u64,
    pub interpreter_ns: u64,
    pub baseline_ns: u64,
    pub optimized_ns: u64,
    pub code_bytes: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Decision {
    Interpret,
    Baseline,
    Optimize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProfitabilityRationale {
    FixedPolicy,
    InsufficientMeasurement,
    NoBaselineBenefit,
    HelperDominated,
    BeforeBreakEven,
    PositiveAmortizedBenefit,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProfitabilityDecision {
    pub tier: Decision,
    pub rationale: ProfitabilityRationale,
    pub gross_benefit_ns: u64,
    pub net_benefit_ns: i128,
    pub break_even_executions: u64,
    /// Net nanoseconds saved per native-code byte. This is the cache's
    /// benefit-density input; zero-sized artifacts deliberately score zero.
    pub benefit_density: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Profitability {
    fixed: Option<Decision>,
    helper_ratio_limit_per_mille: u16,
}

impl Default for Profitability {
    fn default() -> Self {
        Self {
            fixed: None,
            helper_ratio_limit_per_mille: 250,
        }
    }
}

impl Profitability {
    pub const fn fixed(tier: Decision) -> Self {
        Self {
            fixed: Some(tier),
            helper_ratio_limit_per_mille: 250,
        }
    }

    pub fn evaluate(self, profile: Profile) -> ProfitabilityDecision {
        if let Some(tier) = self.fixed {
            return decision(tier, ProfitabilityRationale::FixedPolicy, 0, 0, 0, 0);
        }
        if profile.executions == 0 || profile.interpreter_ns == 0 {
            return decision(
                Decision::Interpret,
                ProfitabilityRationale::InsufficientMeasurement,
                0,
                0,
                0,
                0,
            );
        }
        // All three timing fields are cumulative over `executions`. Normalize
        // exactly once here; callers must never pre-multiply an average.
        let interpreter_per_execution = profile.interpreter_ns / profile.executions;
        let baseline_per_execution = profile.baseline_ns / profile.executions;
        let optimized_per_execution = profile.optimized_ns / profile.executions;
        if baseline_per_execution >= interpreter_per_execution {
            return decision(
                Decision::Interpret,
                ProfitabilityRationale::NoBaselineBenefit,
                0,
                0,
                0,
                0,
            );
        }

        let baseline_saving = interpreter_per_execution.saturating_sub(baseline_per_execution);
        if profile.bytecodes != 0
            && profile.helper_calls.saturating_mul(1_000)
                >= profile
                    .bytecodes
                    .saturating_mul(u64::from(self.helper_ratio_limit_per_mille))
        {
            return measured_decision(
                Decision::Baseline,
                ProfitabilityRationale::HelperDominated,
                baseline_saving,
                profile,
            );
        }

        let optimized_saving = baseline_per_execution.saturating_sub(optimized_per_execution);
        let compile_cost = profile.compile_ns.saturating_add(profile.install_ns);
        let break_even = ceil_div(compile_cost, optimized_saving);
        let gross = optimized_saving.saturating_mul(profile.executions);
        let net = i128::from(gross) - i128::from(compile_cost);
        if optimized_saving == 0 || net <= 0 {
            measured_decision(
                Decision::Baseline,
                ProfitabilityRationale::BeforeBreakEven,
                optimized_saving,
                profile,
            )
        } else {
            measured_decision(
                Decision::Optimize,
                ProfitabilityRationale::PositiveAmortizedBenefit,
                optimized_saving,
                profile,
            )
        }
        .with_break_even(break_even)
    }

    /// Decides whether production should pay for an initial Tier-2 compile.
    ///
    /// Tier-2 time cannot exist before the first compile, so this deliberately
    /// uses measured baseline time plus a conservative, documented 25% saving
    /// model. Once Tier-2 runs, `evaluate` replaces the model with observations.
    pub fn evaluate_trial(self, mut profile: Profile) -> ProfitabilityDecision {
        if let Some(tier) = self.fixed {
            return decision(tier, ProfitabilityRationale::FixedPolicy, 0, 0, 0, 0);
        }
        if profile.executions < 8 || profile.baseline_ns == 0 || profile.bytecodes == 0 {
            return decision(
                Decision::Baseline,
                ProfitabilityRationale::InsufficientMeasurement,
                0,
                0,
                0,
                0,
            );
        }
        let baseline_per_execution = profile.baseline_ns / profile.executions;
        let modeled_optimized = baseline_per_execution.saturating_mul(3) / 4;
        profile.interpreter_ns = baseline_per_execution
            .saturating_mul(2)
            .saturating_mul(profile.executions);
        profile.optimized_ns = modeled_optimized.saturating_mul(profile.executions);
        // The trial-specific helper gate above has already classified this
        // profile; avoid applying the stricter post-trial gate a second time.
        profile.helper_calls = 0;
        self.evaluate(profile)
    }
}

const fn ceil_div(numerator: u64, denominator: u64) -> u64 {
    match (
        numerator.checked_div(denominator),
        numerator.checked_rem(denominator),
    ) {
        (Some(quotient), Some(0)) => quotient,
        (Some(quotient), Some(_)) => quotient.saturating_add(1),
        _ => u64::MAX,
    }
}

fn measured_decision(
    tier: Decision,
    rationale: ProfitabilityRationale,
    saved_per_execution: u64,
    profile: Profile,
) -> ProfitabilityDecision {
    let gross = saved_per_execution.saturating_mul(profile.executions);
    let cost = profile.compile_ns.saturating_add(profile.install_ns);
    let net = i128::from(gross) - i128::from(cost);
    let density = if profile.code_bytes == 0 || net <= 0 {
        0
    } else {
        u64::try_from(net).unwrap_or(u64::MAX) / profile.code_bytes
    };
    decision(
        tier,
        rationale,
        gross,
        net,
        ceil_div(cost, saved_per_execution),
        density,
    )
}

const fn decision(
    tier: Decision,
    rationale: ProfitabilityRationale,
    gross_benefit_ns: u64,
    net_benefit_ns: i128,
    break_even_executions: u64,
    benefit_density: u64,
) -> ProfitabilityDecision {
    ProfitabilityDecision {
        tier,
        rationale,
        gross_benefit_ns,
        net_benefit_ns,
        break_even_executions,
        benefit_density,
    }
}

impl ProfitabilityDecision {
    const fn with_break_even(mut self, break_even_executions: u64) -> Self {
        self.break_even_executions = break_even_executions;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HotReason {
    NeutralBase,
    CallThreshold,
    LoopThreshold,
    BytecodeScale,
    HelperDensity,
    MeasuredWork,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HotThresholds {
    pub calls: u32,
    pub loops: u32,
    pub rationale: HotReason,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AdaptiveInputs {
    pub bytecode_bytes: u32,
    pub helper_ops: u32,
    pub instruction_count: u32,
    pub measured_work: Option<u32>,
}

impl AdaptiveInputs {
    /// A deliberately integer-only preliminary policy. Task 14 calibrates its
    /// coefficients from reproducible measurements; absent measurements the
    /// documented base thresholds remain exactly neutral.
    pub fn thresholds(self) -> HotThresholds {
        let Some(work) = self.measured_work else {
            return HotThresholds {
                calls: BASE_CALL_THRESHOLD,
                loops: BASE_LOOP_THRESHOLD,
                rationale: HotReason::NeutralBase,
            };
        };
        if work >= 4_096 {
            return HotThresholds {
                calls: BASE_CALL_THRESHOLD / 2,
                loops: BASE_LOOP_THRESHOLD / 2,
                rationale: HotReason::MeasuredWork,
            };
        }
        if self.instruction_count != 0
            && self.helper_ops.saturating_mul(4) >= self.instruction_count
        {
            return HotThresholds {
                calls: BASE_CALL_THRESHOLD.saturating_mul(2),
                loops: BASE_LOOP_THRESHOLD.saturating_mul(2),
                rationale: HotReason::HelperDensity,
            };
        }
        if self.bytecode_bytes >= 4_096 {
            return HotThresholds {
                calls: BASE_CALL_THRESHOLD.saturating_mul(2),
                loops: BASE_LOOP_THRESHOLD.saturating_mul(2),
                rationale: HotReason::BytecodeScale,
            };
        }
        HotThresholds {
            calls: BASE_CALL_THRESHOLD,
            loops: BASE_LOOP_THRESHOLD,
            rationale: HotReason::NeutralBase,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HotDecision {
    Cold,
    Queue(HotReason),
    AlreadyQueued,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HotnessState {
    calls: u32,
    loops: u32,
    exits: u32,
    queued: bool,
}

impl HotnessState {
    pub fn record_call_event(&mut self, count: u32) -> HotDecision {
        self.record_call_event_with_thresholds(count, AdaptiveInputs::default().thresholds())
    }

    pub fn record_loop_event(&mut self, count: u32) -> HotDecision {
        self.record_loop_event_with_thresholds(count, AdaptiveInputs::default().thresholds())
    }

    pub fn record_call_event_with_thresholds(
        &mut self,
        count: u32,
        thresholds: HotThresholds,
    ) -> HotDecision {
        self.record_calls(count);
        self.decide(self.calls >= thresholds.calls, HotReason::CallThreshold)
    }

    pub fn record_loop_event_with_thresholds(
        &mut self,
        count: u32,
        thresholds: HotThresholds,
    ) -> HotDecision {
        self.record_loops(count);
        self.decide(self.loops >= thresholds.loops, HotReason::LoopThreshold)
    }

    fn decide(&mut self, hot: bool, reason: HotReason) -> HotDecision {
        if !hot {
            HotDecision::Cold
        } else if self.mark_queued() {
            HotDecision::Queue(reason)
        } else {
            HotDecision::AlreadyQueued
        }
    }
    pub fn record_calls(&mut self, count: u32) {
        self.calls = self.calls.saturating_add(count);
    }

    pub fn record_loops(&mut self, count: u32) {
        self.loops = self.loops.saturating_add(count);
    }

    pub fn record_exits(&mut self, count: u32) {
        self.exits = self.exits.saturating_add(count);
    }

    pub const fn calls(&self) -> u32 {
        self.calls
    }

    pub const fn loops(&self) -> u32 {
        self.loops
    }

    pub const fn exits(&self) -> u32 {
        self.exits
    }

    pub fn mark_queued(&mut self) -> bool {
        if self.queued {
            false
        } else {
            self.queued = true;
            true
        }
    }

    pub fn clear_queued(&mut self) {
        self.queued = false;
    }
}
