//! W^X executable-memory allocation for supported desktop targets.

#[cfg(all(
    target_os = "linux",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod linux;
#[cfg(all(
    target_os = "linux",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use linux as backend;

#[cfg(all(
    target_os = "macos",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod macos;
#[cfg(all(
    target_os = "macos",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use macos as backend;

#[cfg(all(
    target_os = "windows",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
mod windows;
#[cfg(all(
    target_os = "windows",
    target_endian = "little",
    any(target_arch = "x86_64", target_arch = "aarch64")
))]
use windows as backend;

#[cfg(not(any(
    all(
        target_os = "linux",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(
        target_os = "macos",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(
        target_os = "windows",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
mod unsupported;
#[cfg(not(any(
    all(
        target_os = "linux",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(
        target_os = "macos",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    ),
    all(
        target_os = "windows",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
)))]
use unsupported as backend;

use std::{
    collections::BTreeMap,
    ffi::c_void,
    fmt,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, OnceLock, Weak,
    },
};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::ptr::NonNull;

use crate::code_cache::{RelocationKind, ResolvedRelocation};

/// Deterministic platform failure used to verify interpreter fallback paths.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultInjection {
    PageSize,
    Mapping,
    Protection,
    InstructionCache,
    CfgRegistration,
    MacWriteProtection,
}

/// The process-wide policy for writing to the macOS JIT heap.
#[derive(Clone, Copy, Debug, Default)]
pub enum MacJitPolicy {
    /// Toggle the calling thread with `pthread_jit_write_protect_np`.
    #[default]
    ThreadWriteProtect,
    /// Route the copy through an embedder callback in Apple's JIT allowlist.
    ///
    /// The callback receives a pointer to [`MacJitWriteContext`]. It must
    /// validate the context and perform the copy, returning zero on success.
    /// This policy must first be installed through
    /// [`CodeAllocator::bootstrap_mac_jit_policy`] before constructing or
    /// using any allocator that could establish the default policy.
    AllowListCallback(unsafe extern "C" fn(*mut c_void) -> i32),
}

impl MacJitPolicy {
    fn is_same(self, other: Self) -> bool {
        match (self, other) {
            (Self::ThreadWriteProtect, Self::ThreadWriteProtect) => true,
            (Self::AllowListCallback(left), Self::AllowListCallback(right)) => {
                left as usize == right as usize
            }
            _ => false,
        }
    }
}

#[cfg(any(
    test,
    all(
        target_os = "macos",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]
#[derive(Debug)]
struct MacJitPolicySlot {
    active: OnceLock<MacJitPolicy>,
}

#[cfg(any(
    test,
    all(
        target_os = "macos",
        target_endian = "little",
        any(target_arch = "x86_64", target_arch = "aarch64")
    )
))]
impl MacJitPolicySlot {
    const fn new() -> Self {
        Self {
            active: OnceLock::new(),
        }
    }

    fn establish<F>(
        &self,
        requested: MacJitPolicy,
        unsafe_bootstrap: bool,
        check_write_protection: F,
    ) -> Result<(), CodeMemoryError>
    where
        F: FnOnce() -> Result<(), CodeMemoryError>,
    {
        if let Some(active) = self.active.get().copied() {
            return if active.is_same(requested) {
                Ok(())
            } else {
                Err(CodeMemoryError::IncompatibleMacJitPolicy)
            };
        }
        if matches!(requested, MacJitPolicy::AllowListCallback(_)) && !unsafe_bootstrap {
            return Err(CodeMemoryError::MacJitPolicyNotBootstrapped);
        }
        check_write_protection()?;
        match self.active.set(requested) {
            Ok(()) => Ok(()),
            Err(_) => {
                let active = self
                    .active
                    .get()
                    .copied()
                    .expect("macOS JIT policy was concurrently initialized");
                if active.is_same(requested) {
                    Ok(())
                } else {
                    Err(CodeMemoryError::IncompatibleMacJitPolicy)
                }
            }
        }
    }

    fn active(&self) -> Option<MacJitPolicy> {
        self.active.get().copied()
    }
}

/// Backwards-compatible name for [`MacJitPolicy`].
pub type MacJitMode = MacJitPolicy;

/// Copy request passed to a macOS JIT write-allowlist callback.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MacJitWriteContext {
    pub destination: *mut u8,
    pub source: *const u8,
    pub len: usize,
}

pub(super) const INJECTED_RAW_CODE: i64 = -1;

/// Failures while reserving, writing, or publishing executable memory.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum CodeMemoryError {
    UnsupportedPlatform,
    LimitExceeded,
    InvalidSize,
    SizeOverflow,
    WriteOutOfBounds {
        offset: usize,
        len: usize,
        allocation_len: usize,
    },
    MappingFailed {
        operation: &'static str,
        raw_code: i64,
    },
    PageSizeFailed {
        operation: &'static str,
        raw_code: i64,
    },
    ProtectionFailed {
        operation: &'static str,
        raw_code: i64,
    },
    InstructionCacheFlushFailed {
        operation: &'static str,
        raw_code: i64,
    },
    CfgRegistrationFailed {
        operation: &'static str,
        raw_code: i64,
    },
    WriteProtectionFailed {
        operation: &'static str,
        raw_code: i64,
    },
    MissingEntitlement {
        operation: &'static str,
        raw_code: i64,
    },
    WriteCallbackRejected {
        operation: &'static str,
        raw_code: i64,
    },
    UnsupportedWriteProtection {
        operation: &'static str,
        raw_code: i64,
    },
    MacJitPolicyNotBootstrapped,
    IncompatibleMacJitPolicy,
    RelocationOutOfBounds {
        offset: usize,
        width: usize,
        allocation_len: usize,
    },
    RelocationValueOutOfRange {
        offset: usize,
        target: u64,
        addend: i64,
    },
    UnsupportedRelocationKind {
        kind: RelocationKind,
    },
    TargetIsaMismatch,
    UnresolvedRelocationTarget,
    UnwindRegistrationUnsupported,
    UnwindRegistrationFailed {
        operation: &'static str,
    },
    InvalidIndirectTarget {
        offset: usize,
        alignment: usize,
        allocation_len: usize,
    },
    UndeclaredIndirectTarget {
        offset: usize,
    },
    OwnerConfigurationMismatch {
        owner_id: u64,
    },
    NativeDisabled,
}

impl fmt::Display for CodeMemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                f.write_str("executable memory is unsupported on this platform")
            }
            Self::LimitExceeded => f.write_str("executable-memory limit exceeded"),
            Self::InvalidSize => f.write_str("executable-memory allocation size must be nonzero"),
            Self::SizeOverflow => f.write_str("executable-memory allocation size overflowed"),
            Self::WriteOutOfBounds {
                offset,
                len,
                allocation_len,
            } => write!(
                f,
                "code write at {offset} with length {len} exceeds allocation length {allocation_len}"
            ),
            Self::MappingFailed {
                operation,
                raw_code,
            }
            | Self::PageSizeFailed {
                operation,
                raw_code,
            }
            | Self::ProtectionFailed {
                operation,
                raw_code,
            }
            | Self::InstructionCacheFlushFailed {
                operation,
                raw_code,
            }
            | Self::CfgRegistrationFailed {
                operation,
                raw_code,
            }
            | Self::WriteProtectionFailed {
                operation,
                raw_code,
            }
            | Self::MissingEntitlement {
                operation,
                raw_code,
            }
            | Self::WriteCallbackRejected {
                operation,
                raw_code,
            }
            | Self::UnsupportedWriteProtection {
                operation,
                raw_code,
            } => write!(f, "{operation} failed with OS error {raw_code}"),
            Self::MacJitPolicyNotBootstrapped => f.write_str(
                "the macOS JIT callback policy requires unsafe process bootstrap before allocator construction",
            ),
            Self::IncompatibleMacJitPolicy => {
                f.write_str("a different macOS JIT policy is already active in this process")
            }
            Self::RelocationOutOfBounds {
                offset,
                width,
                allocation_len,
            } => write!(
                f,
                "relocation at {offset} with width {width} exceeds allocation length {allocation_len}"
            ),
            Self::RelocationValueOutOfRange {
                offset,
                target,
                addend,
            } => write!(
                f,
                "relocation at {offset} cannot represent target {target} plus addend {addend}"
            ),
            Self::UnsupportedRelocationKind { kind } => {
                write!(f, "relocation kind {kind:?} is unsupported by the publisher")
            }
            Self::TargetIsaMismatch => {
                f.write_str("compiled target ISA does not match the publishing host")
            }
            Self::UnresolvedRelocationTarget => {
                f.write_str("compiled relocation target was not resolved before publication")
            }
            Self::UnwindRegistrationUnsupported => {
                f.write_str("native unwind registration is unsupported for this emitted format")
            }
            Self::UnwindRegistrationFailed { operation } => {
                write!(f, "native unwind registration failed while attempting to {operation}")
            }
            Self::InvalidIndirectTarget {
                offset,
                alignment,
                allocation_len,
            } => write!(
                f,
                "indirect target {offset} is outside allocation length {allocation_len} or not {alignment}-byte aligned"
            ),
            Self::UndeclaredIndirectTarget { offset } => {
                write!(f, "indirect target {offset} was not declared")
            }
            Self::OwnerConfigurationMismatch { owner_id } => write!(
                f,
                "runtime owner {owner_id} already has a different executable-memory configuration"
            ),
            Self::NativeDisabled => {
                f.write_str("native installation is disabled for this allocator owner")
            }
        }
    }
}

impl std::error::Error for CodeMemoryError {}

#[derive(Debug)]
struct AllocatorState {
    limit: usize,
    reserved: AtomicUsize,
    native: Mutex<NativeState>,
    fault: Option<FaultInjection>,
    owner_id: u64,
    mac_jit_policy: MacJitPolicy,
}

#[derive(Debug)]
struct NativeState {
    enabled: bool,
    epoch: u64,
}

impl AllocatorState {
    fn lock_native(&self) -> MutexGuard<'_, NativeState> {
        self.native
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn disable_native(&self, native: &mut NativeState) {
        native.enabled = false;
        native.epoch = native.epoch.wrapping_add(1);
    }

    fn configuration_matches(
        &self,
        limit: usize,
        mac_jit_policy: MacJitPolicy,
        fault: Option<FaultInjection>,
    ) -> bool {
        self.limit == limit && self.fault == fault && self.mac_jit_policy.is_same(mac_jit_policy)
    }
}

/// A logical executable-memory owner with an independent quota.
#[derive(Clone, Debug)]
pub struct CodeAllocator {
    state: Arc<AllocatorState>,
}

impl CodeAllocator {
    /// Creates an allocator for the current supported host.
    pub fn for_host() -> Result<Self, CodeMemoryError> {
        Self::new_unregistered(next_owner_id(), usize::MAX, MacJitPolicy::default(), None)
    }

    /// Creates an allocator whose live mappings may use at most `limit` bytes.
    pub fn with_limit(limit: usize) -> Result<Self, CodeMemoryError> {
        Self::new_unregistered(next_owner_id(), limit, MacJitPolicy::default(), None)
    }

    /// Creates an allocator for one runtime owner with the default macOS mode.
    pub fn for_runtime(owner_id: u64, limit: usize) -> Result<Self, CodeMemoryError> {
        Self::new_registered(owner_id, limit, MacJitPolicy::default(), None)
    }

    /// Creates an allocator for one runtime and an explicit macOS write mode.
    pub fn for_runtime_with_mac_mode(
        owner_id: u64,
        limit: usize,
        mac_jit_mode: MacJitMode,
    ) -> Result<Self, CodeMemoryError> {
        Self::for_runtime_with_mac_policy(owner_id, limit, mac_jit_mode)
    }

    /// Creates an allocator for one runtime and the already-established
    /// process-wide macOS JIT policy.
    pub fn for_runtime_with_mac_policy(
        owner_id: u64,
        limit: usize,
        mac_jit_policy: MacJitPolicy,
    ) -> Result<Self, CodeMemoryError> {
        Self::new_registered(owner_id, limit, mac_jit_policy, None)
    }

    /// Establishes the immutable process-wide macOS JIT policy.
    ///
    /// Callback policy bootstrap must happen before constructing or using any
    /// macOS code allocator that could immutably establish the default
    /// [`MacJitPolicy::ThreadWriteProtect`] policy. Waiting only until before
    /// JIT heap creation is not sufficient. Repeating bootstrap with a
    /// different policy is rejected.
    ///
    /// # Safety
    ///
    /// For [`MacJitPolicy::AllowListCallback`], the caller guarantees all of
    /// the following process-wide preconditions:
    ///
    /// - the embedder registers the exact callback in the entitlement-backed
    ///   Apple JIT callback allowlist;
    /// - the embedder freezes the late-loaded callback allowlist (for example,
    ///   with `pthread_jit_write_freeze_callbacks_np`) before the first
    ///   callback use whenever the platform or entitlement requires it;
    /// - the callback's code, address, and ABI remain valid for the process
    ///   lifetime; and
    /// - the callback returns normally: it never unwinds, never performs
    ///   `longjmp` or another non-local control transfer, and never re-enters
    ///   this allocator in a way that could skip restoration of JIT write
    ///   protection.
    ///
    /// Violating these preconditions may terminate the process or leave JIT
    /// write protection disabled. Neither outcome is detectable or recoverable
    /// by this crate.
    pub unsafe fn bootstrap_mac_jit_policy(policy: MacJitPolicy) -> Result<(), CodeMemoryError> {
        if !backend::SUPPORTED {
            return Err(CodeMemoryError::UnsupportedPlatform);
        }
        backend::bootstrap_mac_jit_policy(policy)
    }

    /// Creates an allocator that fails one platform operation deterministically.
    #[doc(hidden)]
    pub fn with_fault_injection(
        limit: usize,
        fault: FaultInjection,
    ) -> Result<Self, CodeMemoryError> {
        Self::new_unregistered(next_owner_id(), limit, MacJitPolicy::default(), Some(fault))
    }

    /// Creates or reuses a fault-injected state for one runtime owner.
    #[doc(hidden)]
    pub fn for_runtime_with_fault_injection(
        owner_id: u64,
        limit: usize,
        fault: FaultInjection,
    ) -> Result<Self, CodeMemoryError> {
        Self::new_registered(owner_id, limit, MacJitPolicy::default(), Some(fault))
    }

    fn new_unregistered(
        owner_id: u64,
        limit: usize,
        mac_jit_policy: MacJitPolicy,
        fault: Option<FaultInjection>,
    ) -> Result<Self, CodeMemoryError> {
        if !backend::SUPPORTED {
            return Err(CodeMemoryError::UnsupportedPlatform);
        }
        backend::prepare_mac_jit_policy(mac_jit_policy)?;
        Ok(Self {
            state: Arc::new(AllocatorState {
                limit,
                reserved: AtomicUsize::new(0),
                native: Mutex::new(NativeState {
                    enabled: true,
                    epoch: 0,
                }),
                fault,
                owner_id,
                mac_jit_policy,
            }),
        })
    }

    fn new_registered(
        owner_id: u64,
        limit: usize,
        mac_jit_policy: MacJitPolicy,
        fault: Option<FaultInjection>,
    ) -> Result<Self, CodeMemoryError> {
        if !backend::SUPPORTED {
            return Err(CodeMemoryError::UnsupportedPlatform);
        }
        let mut owners = owner_registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        owners.retain(|_, state| state.strong_count() != 0);
        if let Some(state) = owners.get(&owner_id).and_then(Weak::upgrade) {
            if !state.configuration_matches(limit, mac_jit_policy, fault) {
                return Err(CodeMemoryError::OwnerConfigurationMismatch { owner_id });
            }
            return Ok(Self { state });
        }
        backend::prepare_mac_jit_policy(mac_jit_policy)?;
        let allocator = Self::new_unregistered(owner_id, limit, mac_jit_policy, fault)?;
        owners.insert(owner_id, Arc::downgrade(&allocator.state));
        Ok(allocator)
    }

    /// Allocates writable, non-executable storage.
    pub fn allocate(&self, len: usize) -> Result<WritableCode, CodeMemoryError> {
        let mut native = self.state.lock_native();
        if !native.enabled {
            return Err(CodeMemoryError::NativeDisabled);
        }
        if len == 0 {
            return Err(CodeMemoryError::InvalidSize);
        }
        if self.state.fault == Some(FaultInjection::PageSize) {
            self.state.disable_native(&mut native);
            return Err(CodeMemoryError::PageSizeFailed {
                operation: "fault injection before querying executable-memory page size",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        let mapped_len = match backend::round_to_page(len) {
            Ok(mapped_len) => mapped_len,
            Err(error @ CodeMemoryError::PageSizeFailed { .. }) => {
                self.state.disable_native(&mut native);
                return Err(error);
            }
            Err(error) => return Err(error),
        };
        let reservation = Reservation::acquire(Arc::clone(&self.state), mapped_len)?;
        if self.state.fault == Some(FaultInjection::Mapping) {
            self.state.disable_native(&mut native);
            return Err(CodeMemoryError::MappingFailed {
                operation: "fault injection before executable-memory mapping",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        let mapping = match backend::Mapping::allocate(
            mapped_len,
            self.state.owner_id,
            self.state.mac_jit_policy,
        ) {
            Ok(mapping) => mapping,
            Err(error) => {
                self.state.disable_native(&mut native);
                return Err(error);
            }
        };
        let allocation_epoch = native.epoch;
        drop(native);
        Ok(WritableCode {
            mapping: Some(mapping),
            bytes: vec![0; len],
            reservation: Some(reservation),
            state: Arc::clone(&self.state),
            allocation_epoch,
            indirect_targets: vec![0],
        })
    }

    pub fn reserved_bytes(&self) -> usize {
        self.state.reserved.load(Ordering::Acquire)
    }

    pub fn native_enabled(&self) -> bool {
        self.state.lock_native().enabled
    }

    pub fn owner_id(&self) -> u64 {
        self.state.owner_id
    }
}

#[derive(Debug)]
struct Reservation {
    state: Arc<AllocatorState>,
    bytes: usize,
}

impl Reservation {
    fn acquire(state: Arc<AllocatorState>, bytes: usize) -> Result<Self, CodeMemoryError> {
        let mut current = state.reserved.load(Ordering::Acquire);
        loop {
            let next = current
                .checked_add(bytes)
                .ok_or(CodeMemoryError::LimitExceeded)?;
            if next > state.limit {
                return Err(CodeMemoryError::LimitExceeded);
            }
            match state.reserved.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(Self { state, bytes }),
                Err(observed) => current = observed,
            }
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.state.reserved.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

/// A writable, non-executable code allocation that has not been published.
#[derive(Debug)]
pub struct WritableCode {
    mapping: Option<backend::Mapping>,
    bytes: Vec<u8>,
    reservation: Option<Reservation>,
    state: Arc<AllocatorState>,
    allocation_epoch: u64,
    indirect_targets: Vec<usize>,
}

impl WritableCode {
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.mapping
            .as_ref()
            .expect("live writable mapping")
            .as_ptr()
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn write(&mut self, offset: usize, bytes: &[u8]) -> Result<(), CodeMemoryError> {
        let end = offset
            .checked_add(bytes.len())
            .ok_or(CodeMemoryError::WriteOutOfBounds {
                offset,
                len: bytes.len(),
                allocation_len: self.bytes.len(),
            })?;
        let allocation_len = self.bytes.len();
        let Some(destination) = self.bytes.get_mut(offset..end) else {
            return Err(CodeMemoryError::WriteOutOfBounds {
                offset,
                len: bytes.len(),
                allocation_len,
            });
        };
        destination.copy_from_slice(bytes);
        Ok(())
    }

    /// Applies already-resolved Cranelift relocations after validating the
    /// complete batch. No staged byte changes if any relocation is invalid.
    pub fn apply_relocations(
        &mut self,
        relocations: &[ResolvedRelocation],
    ) -> Result<(), CodeMemoryError> {
        let mut writes = Vec::with_capacity(relocations.len());
        for relocation in relocations {
            let offset = relocation.offset as usize;
            let width = match relocation.kind {
                RelocationKind::Abs8 => 8,
                RelocationKind::Abs4
                | RelocationKind::X86PCRel4
                | RelocationKind::X86CallPCRel4
                | RelocationKind::X86CallPLTRel4
                | RelocationKind::X86GOTPCRel4
                | RelocationKind::Arm64Call => 4,
                kind => return Err(CodeMemoryError::UnsupportedRelocationKind { kind }),
            };
            let end = offset
                .checked_add(width)
                .ok_or(CodeMemoryError::RelocationOutOfBounds {
                    offset,
                    width,
                    allocation_len: self.bytes.len(),
                })?;
            if end > self.bytes.len() {
                return Err(CodeMemoryError::RelocationOutOfBounds {
                    offset,
                    width,
                    allocation_len: self.bytes.len(),
                });
            }
            let absolute = i128::from(relocation.target) + i128::from(relocation.addend);
            let out_of_range = || CodeMemoryError::RelocationValueOutOfRange {
                offset,
                target: relocation.target,
                addend: relocation.addend,
            };
            let (bytes, width) = match relocation.kind {
                RelocationKind::Abs8 => {
                    let value = u64::try_from(absolute).map_err(|_| out_of_range())?;
                    (value.to_le_bytes(), 8)
                }
                RelocationKind::Abs4 => {
                    let value = u32::try_from(absolute).map_err(|_| out_of_range())?;
                    let mut bytes = [0; 8];
                    bytes[..4].copy_from_slice(&value.to_le_bytes());
                    (bytes, 4)
                }
                RelocationKind::X86PCRel4
                | RelocationKind::X86CallPCRel4
                | RelocationKind::X86CallPLTRel4
                | RelocationKind::X86GOTPCRel4 => {
                    let place = self.as_ptr() as usize as i128 + offset as i128;
                    let value = i32::try_from(absolute - place).map_err(|_| out_of_range())?;
                    let mut bytes = [0; 8];
                    bytes[..4].copy_from_slice(&value.to_le_bytes());
                    (bytes, 4)
                }
                RelocationKind::Arm64Call => {
                    let place = self.as_ptr() as usize as i128 + offset as i128;
                    let delta = absolute - place;
                    if delta % 4 != 0 || !(-(1_i128 << 27)..(1_i128 << 27)).contains(&delta) {
                        return Err(out_of_range());
                    }
                    let current = u32::from_le_bytes(
                        self.bytes[offset..end].try_into().expect("validated width"),
                    );
                    let immediate = ((delta >> 2) as u32) & 0x03ff_ffff;
                    let value = (current & 0xfc00_0000) | immediate;
                    let mut bytes = [0; 8];
                    bytes[..4].copy_from_slice(&value.to_le_bytes());
                    (bytes, 4)
                }
                kind => return Err(CodeMemoryError::UnsupportedRelocationKind { kind }),
            };
            writes.push((offset, bytes, width));
        }
        for (offset, bytes, width) in writes {
            self.bytes[offset..offset + width].copy_from_slice(&bytes[..width]);
        }
        Ok(())
    }

    /// Declares additional entry offsets that may be acquired indirectly.
    pub fn declare_indirect_targets(&mut self, offsets: &[usize]) -> Result<(), CodeMemoryError> {
        for &offset in offsets {
            validate_indirect_target(offset, self.bytes.len())?;
        }
        self.indirect_targets.extend_from_slice(offsets);
        self.indirect_targets.sort_unstable();
        self.indirect_targets.dedup();
        Ok(())
    }

    /// Copies staged bytes and permanently changes the mapping from RW to RX.
    pub fn publish(mut self) -> Result<ExecutableCode, CodeMemoryError> {
        let mut mapping = self.mapping.take().expect("publish called once");
        for &offset in &self.indirect_targets {
            validate_indirect_target(offset, self.bytes.len())?;
        }
        let mut native = self.state.lock_native();
        if !native.enabled || native.epoch != self.allocation_epoch {
            return Err(CodeMemoryError::NativeDisabled);
        }
        if let Err(error) = mapping.publish(&self.bytes, &self.indirect_targets, self.state.fault) {
            self.state.disable_native(&mut native);
            return Err(error);
        }
        if !native.enabled || native.epoch != self.allocation_epoch {
            return Err(CodeMemoryError::NativeDisabled);
        }
        let reservation = self.reservation.take().expect("live reservation");
        let executable = ExecutableCode {
            allocation: Arc::new(ExecutableAllocation {
                mapping,
                logical_len: self.bytes.len(),
                indirect_targets: self.indirect_targets.into_boxed_slice(),
                _reservation: reservation,
            }),
            entry_offset: 0,
        };
        drop(native);
        Ok(executable)
    }
}

#[derive(Debug)]
struct ExecutableAllocation {
    mapping: backend::Mapping,
    logical_len: usize,
    indirect_targets: Box<[usize]>,
    _reservation: Reservation,
}

/// Immutable executable code. Clones pin the underlying RX mapping.
#[derive(Clone, Debug)]
pub struct ExecutableCode {
    allocation: Arc<ExecutableAllocation>,
    entry_offset: usize,
}

impl ExecutableCode {
    pub fn len(&self) -> usize {
        self.allocation.logical_len
    }

    pub fn is_empty(&self) -> bool {
        self.allocation.logical_len == 0
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.allocation.mapping.as_ptr()
    }

    pub const fn is_writable(&self) -> bool {
        false
    }

    /// Acquires a previously declared and platform-aligned indirect entry.
    pub fn entry(&self, offset: usize) -> Result<Self, CodeMemoryError> {
        validate_indirect_target(offset, self.allocation.logical_len)?;
        if self
            .allocation
            .indirect_targets
            .binary_search(&offset)
            .is_err()
        {
            return Err(CodeMemoryError::UndeclaredIndirectTarget { offset });
        }
        Ok(Self {
            allocation: Arc::clone(&self.allocation),
            entry_offset: offset,
        })
    }

    /// Calls the declared entry at offset zero while holding an allocation pin.
    ///
    /// # Safety
    ///
    /// The published bytes must implement the platform C ABI and return `i32`.
    pub unsafe fn call0_i32(&self) -> i32 {
        let pin = Arc::clone(&self.allocation);
        assert!(pin
            .indirect_targets
            .binary_search(&self.entry_offset)
            .is_ok());
        let entry_ptr = unsafe { pin.mapping.as_ptr().add(self.entry_offset) };
        let entry: unsafe extern "C" fn() -> i32 = unsafe { std::mem::transmute(entry_ptr) };
        let result = unsafe { entry() };
        drop(pin);
        result
    }
}

fn validate_indirect_target(offset: usize, allocation_len: usize) -> Result<(), CodeMemoryError> {
    if offset >= allocation_len || !offset.is_multiple_of(backend::INDIRECT_TARGET_ALIGNMENT) {
        return Err(CodeMemoryError::InvalidIndirectTarget {
            offset,
            alignment: backend::INDIRECT_TARGET_ALIGNMENT,
            allocation_len,
        });
    }
    Ok(())
}

fn next_owner_id() -> u64 {
    static NEXT_OWNER_ID: AtomicU64 = AtomicU64::new(1);
    NEXT_OWNER_ID.fetch_add(1, Ordering::Relaxed)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
unsafe fn nonnull_mmap_address_or_cleanup(
    address: *mut c_void,
    len: usize,
    zero_address_operation: &'static str,
    cleanup: unsafe fn(*mut c_void, usize),
) -> Result<NonNull<u8>, CodeMemoryError> {
    if address.is_null() {
        unsafe {
            cleanup(address, len);
        }
        return Err(CodeMemoryError::MappingFailed {
            operation: zero_address_operation,
            raw_code: 0,
        });
    }
    Ok(unsafe { NonNull::new_unchecked(address.cast()) })
}

fn owner_registry() -> &'static Mutex<BTreeMap<u64, Weak<AllocatorState>>> {
    static OWNERS: OnceLock<Mutex<BTreeMap<u64, Weak<AllocatorState>>>> = OnceLock::new();
    OWNERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[cfg(test)]
mod mac_jit_policy_tests {
    use super::{CodeMemoryError, MacJitPolicy, MacJitPolicySlot};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};

    unsafe extern "C" fn first_callback(_context: *mut c_void) -> i32 {
        0
    }

    unsafe extern "C" fn second_callback(_context: *mut c_void) -> i32 {
        1
    }

    #[test]
    fn safe_callback_policy_requires_prior_unsafe_bootstrap() {
        let slot = MacJitPolicySlot::new();
        let support_checks = AtomicUsize::new(0);
        let result = slot.establish(
            MacJitPolicy::AllowListCallback(first_callback),
            false,
            || {
                support_checks.fetch_add(1, Ordering::Relaxed);
                Ok(())
            },
        );

        assert!(matches!(
            result,
            Err(CodeMemoryError::MacJitPolicyNotBootstrapped)
        ));
        assert_eq!(support_checks.load(Ordering::Relaxed), 0);
        assert!(slot.active().is_none());
    }

    #[test]
    fn policy_is_immutable_and_mismatch_avoids_platform_calls() {
        let slot = MacJitPolicySlot::new();
        slot.establish(
            MacJitPolicy::AllowListCallback(first_callback),
            true,
            || Ok(()),
        )
        .unwrap();

        slot.establish(
            MacJitPolicy::AllowListCallback(first_callback),
            false,
            || panic!("an established compatible policy needs no platform call"),
        )
        .unwrap();
        assert!(matches!(
            slot.establish(
                MacJitPolicy::AllowListCallback(second_callback),
                true,
                || panic!("an incompatible policy must be rejected first"),
            ),
            Err(CodeMemoryError::IncompatibleMacJitPolicy)
        ));
        assert!(matches!(
            slot.establish(MacJitPolicy::ThreadWriteProtect, true, || panic!(
                "an incompatible mode must be rejected first"
            )),
            Err(CodeMemoryError::IncompatibleMacJitPolicy)
        ));
    }

    #[test]
    fn unsupported_write_protection_does_not_initialize_policy() {
        let slot = MacJitPolicySlot::new();
        let unsupported = CodeMemoryError::UnsupportedWriteProtection {
            operation: "test support query",
            raw_code: 0,
        };

        assert_eq!(
            slot.establish(MacJitPolicy::ThreadWriteProtect, false, || {
                Err(unsupported.clone())
            }),
            Err(unsupported)
        );
        assert!(slot.active().is_none());
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod mmap_result_tests {
    use super::{nonnull_mmap_address_or_cleanup, CodeMemoryError};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicBool, Ordering};

    static CLEANED: AtomicBool = AtomicBool::new(false);

    unsafe fn record_cleanup(_address: *mut c_void, _len: usize) {
        CLEANED.store(true, Ordering::Release);
    }

    #[test]
    fn address_zero_is_categorized_and_cleaned_up() {
        CLEANED.store(false, Ordering::Release);
        let error = unsafe {
            nonnull_mmap_address_or_cleanup(
                std::ptr::null_mut(),
                4096,
                "test mmap returned address zero",
                record_cleanup,
            )
        }
        .unwrap_err();

        assert!(matches!(
            error,
            CodeMemoryError::MappingFailed {
                operation: "test mmap returned address zero",
                raw_code: 0,
            }
        ));
        assert!(CLEANED.load(Ordering::Acquire));
    }
}
