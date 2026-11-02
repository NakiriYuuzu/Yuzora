//! JIT policy and resource limits.

use std::sync::Arc;

use crate::{abi::AbiMismatch, JitError, JitMetrics};

pub const DEFAULT_CALL_THRESHOLD: u32 = 32;
pub const DEFAULT_LOOP_THRESHOLD: u32 = 56;
pub const DEFAULT_MAX_CODE_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
pub const DEFAULT_MAX_IR_BYTES: usize = 64 * 1024 * 1024;
/// Per-compile budget. Sanitizer-instrumented test builds run the compiler an
/// order of magnitude slower, where a single Tier 1 compile can exceed the
/// production budget on CI runners and then time out on every retry, so
/// their default budget is ten minutes.
pub const DEFAULT_COMPILE_TIMEOUT_MS: u64 = if cfg!(rquickjs_sanitizer) {
    600_000
} else {
    30_000
};
pub const DEFAULT_MAX_QUEUE_LEN: usize = 256;
pub const DEFAULT_WORKERS: usize = 1;
pub const DEFAULT_MAX_COMPILE_ATTEMPTS: u8 = 4;

/// Explicit tier policy used by reproducible measurements and embedders that
/// prefer bounded compilation. Automatic is the production default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum JitTierPolicy {
    #[default]
    Automatic,
    BaselineOnly,
    Optimize,
}

/// Bounded policy and resource limits for one JIT runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JitDiagnosticKind {
    AbiMismatch(AbiMismatch),
    BackendAttachment,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JitDiagnostic {
    kind: JitDiagnosticKind,
}

impl JitDiagnostic {
    pub const fn kind(&self) -> &JitDiagnosticKind {
        &self.kind
    }
}

type DiagnosticCallback = Arc<dyn Fn(&JitDiagnostic) + Send + Sync>;
type MetricsObserver = Arc<dyn Fn(&JitMetrics) + Send + Sync>;

#[derive(Clone)]
pub struct JitConfig {
    call_threshold: u32,
    loop_threshold: u32,
    max_code_bytes: usize,
    max_metadata_bytes: usize,
    max_snapshot_bytes: usize,
    max_ir_bytes: usize,
    compile_timeout_ms: u64,
    max_queue_len: usize,
    workers: usize,
    max_compile_attempts: u8,
    tier_policy: JitTierPolicy,
    #[cfg(feature = "test-support")]
    stress_gc: bool,
    #[cfg(feature = "test-support")]
    force_optimized: bool,
    diagnostic_callback: Option<DiagnosticCallback>,
    metrics_observer: Option<MetricsObserver>,
}

impl core::fmt::Debug for JitConfig {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("JitConfig")
            .field("call_threshold", &self.call_threshold)
            .field("loop_threshold", &self.loop_threshold)
            .field("max_code_bytes", &self.max_code_bytes)
            .field("max_metadata_bytes", &self.max_metadata_bytes)
            .field("max_snapshot_bytes", &self.max_snapshot_bytes)
            .field("max_ir_bytes", &self.max_ir_bytes)
            .field("compile_timeout_ms", &self.compile_timeout_ms)
            .field("max_queue_len", &self.max_queue_len)
            .field("workers", &self.workers)
            .field("max_compile_attempts", &self.max_compile_attempts)
            .field("tier_policy", &self.tier_policy)
            .field("stress_gc", &{
                #[cfg(feature = "test-support")]
                {
                    self.stress_gc
                }
                #[cfg(not(feature = "test-support"))]
                {
                    false
                }
            })
            .field("force_optimized", &{
                #[cfg(feature = "test-support")]
                {
                    self.force_optimized
                }
                #[cfg(not(feature = "test-support"))]
                {
                    false
                }
            })
            .field(
                "has_diagnostic_callback",
                &self.diagnostic_callback.is_some(),
            )
            .field("has_metrics_observer", &self.metrics_observer.is_some())
            .finish()
    }
}

impl PartialEq for JitConfig {
    fn eq(&self, other: &Self) -> bool {
        self.call_threshold == other.call_threshold
            && self.loop_threshold == other.loop_threshold
            && self.max_code_bytes == other.max_code_bytes
            && self.max_metadata_bytes == other.max_metadata_bytes
            && self.max_snapshot_bytes == other.max_snapshot_bytes
            && self.max_ir_bytes == other.max_ir_bytes
            && self.compile_timeout_ms == other.compile_timeout_ms
            && self.max_queue_len == other.max_queue_len
            && self.workers == other.workers
            && self.max_compile_attempts == other.max_compile_attempts
            && self.tier_policy == other.tier_policy
            && {
                #[cfg(feature = "test-support")]
                {
                    self.stress_gc == other.stress_gc
                }
                #[cfg(not(feature = "test-support"))]
                {
                    true
                }
            }
            && {
                #[cfg(feature = "test-support")]
                {
                    self.force_optimized == other.force_optimized
                }
                #[cfg(not(feature = "test-support"))]
                {
                    true
                }
            }
            && callbacks_equal(&self.diagnostic_callback, &other.diagnostic_callback)
            && callbacks_equal(&self.metrics_observer, &other.metrics_observer)
    }
}

impl Eq for JitConfig {}

fn callbacks_equal<T: ?Sized>(left: &Option<Arc<T>>, right: &Option<Arc<T>>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => Arc::ptr_eq(left, right),
        (None, None) => true,
        _ => false,
    }
}

impl JitConfig {
    /// Starts configuring a JIT runtime with bounded defaults.
    pub fn builder() -> JitConfigBuilder {
        JitConfigBuilder::default()
    }

    pub const fn call_threshold(&self) -> u32 {
        self.call_threshold
    }

    pub const fn loop_threshold(&self) -> u32 {
        self.loop_threshold
    }

    pub const fn max_code_bytes(&self) -> usize {
        self.max_code_bytes
    }
    pub const fn max_metadata_bytes(&self) -> usize {
        self.max_metadata_bytes
    }
    pub const fn max_snapshot_bytes(&self) -> usize {
        self.max_snapshot_bytes
    }
    pub const fn max_ir_bytes(&self) -> usize {
        self.max_ir_bytes
    }
    pub const fn compile_timeout_ms(&self) -> u64 {
        self.compile_timeout_ms
    }

    pub const fn max_queue_len(&self) -> usize {
        self.max_queue_len
    }

    pub const fn workers(&self) -> usize {
        self.workers
    }

    pub const fn max_compile_attempts(&self) -> u8 {
        self.max_compile_attempts
    }
    pub const fn tier_policy(&self) -> JitTierPolicy {
        self.tier_policy
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub const fn stress_gc(&self) -> bool {
        self.stress_gc
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub const fn force_optimized(&self) -> bool {
        self.force_optimized
    }

    pub(crate) fn report(&self, kind: JitDiagnosticKind) {
        if let Some(callback) = &self.diagnostic_callback {
            callback(&JitDiagnostic { kind });
        }
    }

    pub(crate) fn observe(&self, metrics: &JitMetrics) {
        if let Some(callback) = &self.metrics_observer {
            callback(metrics);
        }
    }
}

impl Default for JitConfig {
    fn default() -> Self {
        Self {
            call_threshold: DEFAULT_CALL_THRESHOLD,
            loop_threshold: DEFAULT_LOOP_THRESHOLD,
            max_code_bytes: DEFAULT_MAX_CODE_BYTES,
            max_metadata_bytes: DEFAULT_MAX_METADATA_BYTES,
            max_snapshot_bytes: DEFAULT_MAX_SNAPSHOT_BYTES,
            max_ir_bytes: DEFAULT_MAX_IR_BYTES,
            compile_timeout_ms: DEFAULT_COMPILE_TIMEOUT_MS,
            max_queue_len: DEFAULT_MAX_QUEUE_LEN,
            workers: DEFAULT_WORKERS,
            max_compile_attempts: DEFAULT_MAX_COMPILE_ATTEMPTS,
            tier_policy: JitTierPolicy::Automatic,
            #[cfg(feature = "test-support")]
            stress_gc: false,
            #[cfg(feature = "test-support")]
            force_optimized: false,
            diagnostic_callback: None,
            metrics_observer: None,
        }
    }
}

/// Builder for [`JitConfig`].
#[derive(Clone, Debug, Default)]
pub struct JitConfigBuilder {
    config: JitConfig,
}

impl JitConfigBuilder {
    pub fn call_threshold(mut self, value: u32) -> Self {
        self.config.call_threshold = value;
        self
    }

    pub fn loop_threshold(mut self, value: u32) -> Self {
        self.config.loop_threshold = value;
        self
    }

    pub fn max_code_bytes(mut self, value: usize) -> Self {
        self.config.max_code_bytes = value;
        self
    }
    pub fn max_metadata_bytes(mut self, value: usize) -> Self {
        self.config.max_metadata_bytes = value;
        self
    }
    pub fn max_snapshot_bytes(mut self, value: usize) -> Self {
        self.config.max_snapshot_bytes = value;
        self
    }
    pub fn max_ir_bytes(mut self, value: usize) -> Self {
        self.config.max_ir_bytes = value;
        self
    }
    pub fn compile_timeout_ms(mut self, value: u64) -> Self {
        self.config.compile_timeout_ms = value;
        self
    }

    pub fn max_queue_len(mut self, value: usize) -> Self {
        self.config.max_queue_len = value;
        self
    }

    pub fn workers(mut self, value: usize) -> Self {
        self.config.workers = value;
        self
    }

    pub fn max_compile_attempts(mut self, value: u8) -> Self {
        self.config.max_compile_attempts = value;
        self
    }

    pub fn tier_policy(mut self, value: JitTierPolicy) -> Self {
        self.config.tier_policy = value;
        self
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn stress_gc(mut self, value: bool) -> Self {
        self.config.stress_gc = value;
        self
    }

    #[cfg(feature = "test-support")]
    #[doc(hidden)]
    pub fn force_optimized_for_test(mut self, value: bool) -> Self {
        self.config.force_optimized = value;
        self
    }

    pub fn diagnostic_callback<F>(mut self, callback: F) -> Self
    where
        F: Fn(&JitDiagnostic) + Send + Sync + 'static,
    {
        self.config.diagnostic_callback = Some(Arc::new(callback));
        self
    }

    /// Observes the latest metrics after each explicit [`crate::Jit::poll`].
    ///
    /// The callback runs after the QuickJS runtime lock is released. Panics are
    /// contained and do not cross the runtime or C ABI boundary.
    pub fn metrics_observer<F>(mut self, callback: F) -> Self
    where
        F: Fn(&JitMetrics) + Send + Sync + 'static,
    {
        self.config.metrics_observer = Some(Arc::new(callback));
        self
    }

    pub fn build(self) -> Result<JitConfig, JitError> {
        if self.config.call_threshold == 0 {
            return Err(JitError::InvalidConfig("call_threshold"));
        }
        if self.config.loop_threshold == 0 {
            return Err(JitError::InvalidConfig("loop_threshold"));
        }
        if self.config.max_code_bytes == 0 {
            return Err(JitError::InvalidConfig("max_code_bytes"));
        }
        if self.config.max_metadata_bytes == 0 {
            return Err(JitError::InvalidConfig("max_metadata_bytes"));
        }
        if self.config.max_snapshot_bytes == 0 {
            return Err(JitError::InvalidConfig("max_snapshot_bytes"));
        }
        if self.config.max_ir_bytes == 0 {
            return Err(JitError::InvalidConfig("max_ir_bytes"));
        }
        if self.config.compile_timeout_ms == 0 {
            return Err(JitError::InvalidConfig("compile_timeout_ms"));
        }
        if self.config.max_queue_len == 0 {
            return Err(JitError::InvalidConfig("max_queue_len"));
        }
        if self.config.workers == 0 {
            return Err(JitError::InvalidConfig("workers"));
        }
        if self.config.max_compile_attempts == 0 {
            return Err(JitError::InvalidConfig("max_compile_attempts"));
        }
        Ok(self.config)
    }
}
