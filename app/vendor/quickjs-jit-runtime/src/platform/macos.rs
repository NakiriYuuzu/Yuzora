use std::{
    collections::BTreeMap,
    ffi::c_void,
    io,
    ptr::NonNull,
    sync::{Mutex, MutexGuard, OnceLock},
};

use super::{
    nonnull_mmap_address_or_cleanup, CodeMemoryError, FaultInjection, MacJitPolicy,
    MacJitPolicySlot, MacJitWriteContext, INJECTED_RAW_CODE,
};

pub(super) const SUPPORTED: bool = true;
pub(super) const INDIRECT_TARGET_ALIGNMENT: usize = 16;

const PROCESS_HEAP_BYTES: usize = 256 * 1024 * 1024;

pub(super) fn round_to_page(len: usize) -> Result<usize, CodeMemoryError> {
    let page_size = page_size()?;
    len.checked_add(page_size - 1)
        .map(|rounded| rounded / page_size * page_size)
        .ok_or(CodeMemoryError::SizeOverflow)
}

pub(super) fn prepare_mac_jit_policy(policy: MacJitPolicy) -> Result<(), CodeMemoryError> {
    establish_policy(policy, false)
}

pub(super) fn bootstrap_mac_jit_policy(policy: MacJitPolicy) -> Result<(), CodeMemoryError> {
    establish_policy(policy, true)
}

#[derive(Debug)]
pub(super) struct Mapping {
    ptr: NonNull<u8>,
    offset: usize,
    len: usize,
    owner_id: u64,
}

unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    pub(super) fn allocate(
        len: usize,
        owner_id: u64,
        policy: MacJitPolicy,
    ) -> Result<Self, CodeMemoryError> {
        let mut heap = heap(policy)?;
        let offset = heap.allocate(len, owner_id)?;
        let ptr = NonNull::new((heap.base + offset) as *mut u8).ok_or(
            CodeMemoryError::MappingFailed {
                operation: "suballocate process MAP_JIT heap",
                raw_code: i64::from(libc::ENOMEM),
            },
        )?;
        Ok(Self {
            ptr,
            offset,
            len,
            owner_id,
        })
    }

    pub(super) fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    pub(super) fn publish(
        &mut self,
        bytes: &[u8],
        _indirect_targets: &[usize],
        fault: Option<FaultInjection>,
    ) -> Result<(), CodeMemoryError> {
        if fault == Some(FaultInjection::Protection) {
            return Err(CodeMemoryError::ProtectionFailed {
                operation: "fault injection before macOS JIT publication",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        if fault == Some(FaultInjection::MacWriteProtection) {
            return Err(CodeMemoryError::WriteProtectionFailed {
                operation: "fault injection before macOS JIT write transition",
                raw_code: INJECTED_RAW_CODE,
            });
        }

        match active_policy()? {
            MacJitPolicy::ThreadWriteProtect => unsafe {
                let _write_scope = ThreadWriteScope::enter();
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr.as_ptr(), bytes.len());
            },
            MacJitPolicy::AllowListCallback(callback) => {
                let mut context = MacJitWriteContext {
                    destination: self.ptr.as_ptr(),
                    source: bytes.as_ptr(),
                    len: bytes.len(),
                };
                let status = unsafe {
                    pthread_jit_write_with_callback_np(
                        Some(callback),
                        (&mut context as *mut MacJitWriteContext).cast(),
                    )
                };
                if status != 0 {
                    return Err(CodeMemoryError::WriteCallbackRejected {
                        operation: "pthread_jit_write_with_callback_np",
                        raw_code: i64::from(status),
                    });
                }
            }
        }

        if fault == Some(FaultInjection::InstructionCache) {
            return Err(CodeMemoryError::InstructionCacheFlushFailed {
                operation: "fault injection before sys_icache_invalidate",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        unsafe {
            sys_icache_invalidate(self.ptr.as_ptr().cast(), bytes.len());
        }
        if fault == Some(FaultInjection::CfgRegistration) {
            return Err(CodeMemoryError::CfgRegistrationFailed {
                operation: "fault injection before indirect-target publication",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        Ok(())
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        if let Some(Ok(heap)) = process_heap().get() {
            let mut heap = heap.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            heap.release(self.offset, self.len, self.owner_id);
        }
    }
}

struct ThreadWriteScope;

impl ThreadWriteScope {
    unsafe fn enter() -> Self {
        unsafe {
            libc::pthread_jit_write_protect_np(0);
        }
        Self
    }
}

impl Drop for ThreadWriteScope {
    fn drop(&mut self) {
        unsafe {
            libc::pthread_jit_write_protect_np(1);
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct HeapAllocation {
    len: usize,
    owner_id: u64,
}

#[derive(Debug)]
struct ProcessHeap {
    base: usize,
    len: usize,
    free: BTreeMap<usize, usize>,
    allocations: BTreeMap<usize, HeapAllocation>,
}

impl ProcessHeap {
    fn create(policy: MacJitPolicy) -> Result<Mutex<Self>, CodeMemoryError> {
        require_write_protection_support()?;
        if !active_policy()?.is_same(policy) {
            return Err(CodeMemoryError::IncompatibleMacJitPolicy);
        }
        let len = round_to_page(PROCESS_HEAP_BYTES)?;
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE | libc::PROT_EXEC,
                libc::MAP_PRIVATE | libc::MAP_ANON | libc::MAP_JIT,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            let raw_code = raw_os_error();
            if raw_code == i64::from(libc::EPERM) || raw_code == i64::from(libc::EACCES) {
                return Err(CodeMemoryError::MissingEntitlement {
                    operation: "mmap(MAP_JIT)",
                    raw_code,
                });
            }
            return Err(CodeMemoryError::MappingFailed {
                operation: "mmap(MAP_JIT)",
                raw_code,
            });
        }
        let base = unsafe {
            nonnull_mmap_address_or_cleanup(ptr, len, "mmap(MAP_JIT) returned address zero", unmap)
        }?;
        let mut free = BTreeMap::new();
        free.insert(0, len);
        Ok(Mutex::new(Self {
            base: base.as_ptr() as usize,
            len,
            free,
            allocations: BTreeMap::new(),
        }))
    }

    fn allocate(&mut self, len: usize, owner_id: u64) -> Result<usize, CodeMemoryError> {
        let candidate = self
            .free
            .iter()
            .find_map(|(&offset, &available)| (available >= len).then_some((offset, available)));
        let Some((offset, available)) = candidate else {
            return Err(CodeMemoryError::MappingFailed {
                operation: "suballocate process MAP_JIT heap",
                raw_code: i64::from(libc::ENOMEM),
            });
        };
        self.free.remove(&offset);
        if available > len {
            self.free.insert(offset + len, available - len);
        }
        self.allocations
            .insert(offset, HeapAllocation { len, owner_id });
        Ok(offset)
    }

    fn release(&mut self, offset: usize, len: usize, owner_id: u64) {
        let Some(allocation) = self.allocations.remove(&offset) else {
            return;
        };
        if allocation.len != len || allocation.owner_id != owner_id {
            self.allocations.insert(offset, allocation);
            return;
        }

        let mut start = offset;
        let mut free_len = len;
        if let Some((&previous, &previous_len)) = self.free.range(..offset).next_back() {
            if previous + previous_len == offset {
                self.free.remove(&previous);
                start = previous;
                free_len += previous_len;
            }
        }
        if let Some((&next, &next_len)) = self.free.range(start..).next() {
            if start + free_len == next {
                self.free.remove(&next);
                free_len += next_len;
            }
        }
        debug_assert!(start + free_len <= self.len);
        self.free.insert(start, free_len);
    }
}

fn heap(policy: MacJitPolicy) -> Result<MutexGuard<'static, ProcessHeap>, CodeMemoryError> {
    prepare_mac_jit_policy(policy)?;
    let heap = process_heap()
        .get_or_init(|| ProcessHeap::create(policy))
        .as_ref()
        .map_err(Clone::clone)?;
    Ok(heap.lock().unwrap_or_else(|poisoned| poisoned.into_inner()))
}

fn process_heap() -> &'static OnceLock<Result<Mutex<ProcessHeap>, CodeMemoryError>> {
    static HEAP: OnceLock<Result<Mutex<ProcessHeap>, CodeMemoryError>> = OnceLock::new();
    &HEAP
}

fn establish_policy(policy: MacJitPolicy, unsafe_bootstrap: bool) -> Result<(), CodeMemoryError> {
    process_policy().establish(policy, unsafe_bootstrap, require_write_protection_support)
}

fn active_policy() -> Result<MacJitPolicy, CodeMemoryError> {
    process_policy()
        .active()
        .ok_or(CodeMemoryError::MacJitPolicyNotBootstrapped)
}

fn process_policy() -> &'static MacJitPolicySlot {
    static POLICY: MacJitPolicySlot = MacJitPolicySlot::new();
    &POLICY
}

fn require_write_protection_support() -> Result<(), CodeMemoryError> {
    if unsafe { libc::pthread_jit_write_protect_supported_np() } == 0 {
        return Err(CodeMemoryError::UnsupportedWriteProtection {
            operation: "pthread_jit_write_protect_supported_np",
            raw_code: 0,
        });
    }
    Ok(())
}

fn page_size() -> Result<usize, CodeMemoryError> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return Err(CodeMemoryError::PageSizeFailed {
            operation: "sysconf(_SC_PAGESIZE)",
            raw_code: raw_os_error(),
        });
    }
    usize::try_from(page_size).map_err(|_| CodeMemoryError::SizeOverflow)
}

fn raw_os_error() -> i64 {
    i64::from(io::Error::last_os_error().raw_os_error().unwrap_or(-1))
}

unsafe fn unmap(address: *mut c_void, len: usize) {
    let _ = unsafe { libc::munmap(address, len) };
}

unsafe extern "C" {
    fn pthread_jit_write_with_callback_np(
        callback: Option<unsafe extern "C" fn(*mut c_void) -> i32>,
        context: *mut c_void,
    ) -> i32;
    fn sys_icache_invalidate(start: *mut c_void, len: usize);
}
