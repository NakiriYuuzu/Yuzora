//! Runtime-visible JIT metrics.

/// Immutable metrics exposed by the no-op JIT backend.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct JitMetrics {
    native_enabled: bool,
    pub queued: u64,
    pub compiling: u64,
    pub compile_failures: u64,
    /// Worker compilations rejected because the bytecode is not supported.
    pub unsupported_opcode_failures: u64,
    /// Tier 1 compilations rejected by the closed semantic policy.
    pub tier1_rejections: u64,
    /// Worker compilations rejected by a compiler resource bound.
    pub resource_limit_failures: u64,
    pub compile_timeouts: u64,
    /// Worker compilations cancelled during shutdown or invalidation.
    pub cancelled_compilations: u64,
    /// Compiler panics contained by the background worker boundary.
    pub compiler_panics: u64,
    /// Worker results or artifacts rejected as structurally invalid.
    pub invalid_artifacts: u64,
    /// Valid compiler results that failed publication or cache installation.
    pub install_failures: u64,
    pub stale_results: u64,
    pub installed: u64,
    pub blacklisted: u64,
    pub retired: u64,
    pub queue_saturated: u64,
    pub completion_queue_saturated: u64,
    pub worker_queue_saturated: u64,
    pub resource_limit_rejections: u64,
    pub code_bytes: usize,
    pub metadata_bytes: usize,
    pub pending_worker_jobs: usize,
    pub pending_snapshot_bytes: usize,
    pub active_ir_bytes: usize,
    pub peak_compiler_bytes: usize,
    pub evicted: u64,
    pub native_entries: u64,
    /// Nonempty entry handles acquired from the backend, including OSR.
    /// Cached C entries execute again without increasing this counter.
    pub native_acquisitions: u64,
    pub native_exits: u64,
    pub native_fallbacks: u64,
    pub native_retries: u64,
    pub osr_entries: u64,
    pub osr_attempts: u64,
    pub osr_validated_successes: u64,
    pub osr_generated_retries: u64,
    pub hot_call_queues: u64,
    pub hot_loop_queues: u64,
    pub adaptive_neutral_queues: u64,
    pub adaptive_inputs_recorded: u64,
    pub adaptive_size_factor_disabled: u64,
    pub snapshot_requests: u64,
    pub osr_not_ready: u64,
    pub osr_map_misses: u64,
    pub osr_validation_failures: u64,
    pub tier2_entries: u64,
    pub tier2_guard_failures: u64,
    pub deopts: u64,
    pub deopt_materializations: u64,
    pub side_exits: u64,
    pub optimized_demotions: u64,
    /// Baseline artifacts unpublished after automatic profitability rejection.
    pub interpreter_demotions: u64,
    pub stable_path_compile_requests: u64,
    pub side_path_entries: u64,
    pub boxes_elided: u64,
    pub cse_eliminated: u64,
    pub dead_nodes_eliminated: u64,
    pub dependency_invalidations: u64,
    /// Functions kept in the interpreter because every native tier would
    /// pay the generic call bridge on each call without a loop to amortize it.
    pub generic_call_rejections: u64,
    pub profitability_evaluations: u64,
    pub profitability_approved: u64,
    pub profitability_rejected: u64,
    pub benefit_recordings: u64,
    pub measured_benefit_ns: u64,
    pub compile_ns: u64,
    pub install_ns: u64,
}

impl JitMetrics {
    pub(crate) const fn disabled() -> Self {
        Self {
            native_enabled: false,
            queued: 0,
            compiling: 0,
            compile_failures: 0,
            unsupported_opcode_failures: 0,
            tier1_rejections: 0,
            resource_limit_failures: 0,
            compile_timeouts: 0,
            cancelled_compilations: 0,
            compiler_panics: 0,
            invalid_artifacts: 0,
            install_failures: 0,
            stale_results: 0,
            installed: 0,
            blacklisted: 0,
            retired: 0,
            queue_saturated: 0,
            completion_queue_saturated: 0,
            worker_queue_saturated: 0,
            resource_limit_rejections: 0,
            code_bytes: 0,
            metadata_bytes: 0,
            pending_worker_jobs: 0,
            pending_snapshot_bytes: 0,
            active_ir_bytes: 0,
            peak_compiler_bytes: 0,
            evicted: 0,
            native_entries: 0,
            native_acquisitions: 0,
            native_exits: 0,
            native_fallbacks: 0,
            native_retries: 0,
            osr_entries: 0,
            osr_attempts: 0,
            osr_validated_successes: 0,
            osr_generated_retries: 0,
            hot_call_queues: 0,
            hot_loop_queues: 0,
            adaptive_neutral_queues: 0,
            adaptive_inputs_recorded: 0,
            adaptive_size_factor_disabled: 0,
            snapshot_requests: 0,
            osr_not_ready: 0,
            osr_map_misses: 0,
            osr_validation_failures: 0,
            tier2_entries: 0,
            tier2_guard_failures: 0,
            deopts: 0,
            deopt_materializations: 0,
            side_exits: 0,
            optimized_demotions: 0,
            interpreter_demotions: 0,
            stable_path_compile_requests: 0,
            side_path_entries: 0,
            boxes_elided: 0,
            cse_eliminated: 0,
            dead_nodes_eliminated: 0,
            dependency_invalidations: 0,
            generic_call_rejections: 0,
            profitability_evaluations: 0,
            profitability_approved: 0,
            profitability_rejected: 0,
            benefit_recordings: 0,
            measured_benefit_ns: 0,
            compile_ns: 0,
            install_ns: 0,
        }
    }

    /// Reports whether this runtime can currently enter native JIT code.
    pub const fn native_enabled(&self) -> bool {
        self.native_enabled
    }

    pub(crate) fn set_native_enabled(&mut self, enabled: bool) {
        self.native_enabled = enabled;
    }
}
