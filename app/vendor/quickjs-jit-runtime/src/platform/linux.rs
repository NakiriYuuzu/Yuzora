use std::{io, ptr::NonNull};

use super::{
    nonnull_mmap_address_or_cleanup, CodeMemoryError, FaultInjection, MacJitPolicy,
    INJECTED_RAW_CODE,
};

pub(super) const SUPPORTED: bool = true;
pub(super) const INDIRECT_TARGET_ALIGNMENT: usize = 16;

pub(super) fn round_to_page(len: usize) -> Result<usize, CodeMemoryError> {
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if page_size <= 0 {
        return Err(CodeMemoryError::PageSizeFailed {
            operation: "sysconf(_SC_PAGESIZE)",
            raw_code: raw_os_error(),
        });
    }
    let page_size = usize::try_from(page_size).map_err(|_| CodeMemoryError::SizeOverflow)?;
    len.checked_add(page_size - 1)
        .map(|rounded| rounded / page_size * page_size)
        .ok_or(CodeMemoryError::SizeOverflow)
}

pub(super) fn prepare_mac_jit_policy(_policy: MacJitPolicy) -> Result<(), CodeMemoryError> {
    Ok(())
}

pub(super) fn bootstrap_mac_jit_policy(_policy: MacJitPolicy) -> Result<(), CodeMemoryError> {
    Ok(())
}

#[derive(Debug)]
pub(super) struct Mapping {
    ptr: NonNull<u8>,
    len: usize,
}

unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Mapping {
    pub(super) fn allocate(
        len: usize,
        _owner_id: u64,
        _mac_jit_policy: MacJitPolicy,
    ) -> Result<Self, CodeMemoryError> {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(CodeMemoryError::MappingFailed {
                operation: "mmap(PROT_READ|PROT_WRITE)",
                raw_code: raw_os_error(),
            });
        }
        let ptr = unsafe {
            nonnull_mmap_address_or_cleanup(
                ptr,
                len,
                "mmap(PROT_READ|PROT_WRITE) returned address zero",
                unmap,
            )
        }?;
        Ok(Self { ptr, len })
    }

    #[cfg(all(test, target_endian = "little", target_arch = "aarch64"))]
    fn allocate_at_for_cache_test(len: usize, address: *mut u8) -> Result<Self, CodeMemoryError> {
        let ptr = unsafe {
            libc::mmap(
                address.cast(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS | libc::MAP_FIXED_NOREPLACE,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(CodeMemoryError::MappingFailed {
                operation: "mmap(PROT_READ|PROT_WRITE, MAP_FIXED_NOREPLACE) for cache test",
                raw_code: raw_os_error(),
            });
        }
        if ptr != address.cast() {
            let _ = unsafe { libc::munmap(ptr, len) };
            return Err(CodeMemoryError::MappingFailed {
                operation: "mmap(MAP_FIXED_NOREPLACE) returned a different cache-test address",
                raw_code: 0,
            });
        }
        let Some(ptr) = NonNull::new(ptr.cast()) else {
            let _ = unsafe { libc::munmap(ptr, len) };
            return Err(CodeMemoryError::MappingFailed {
                operation: "mmap(MAP_FIXED_NOREPLACE) returned address zero for cache test",
                raw_code: 0,
            });
        };
        Ok(Self { ptr, len })
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
        if fault == Some(FaultInjection::MacWriteProtection) {
            return Err(CodeMemoryError::WriteProtectionFailed {
                operation: "fault injection before platform write-protection transition",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), self.ptr.as_ptr(), bytes.len());
        }
        if fault == Some(FaultInjection::Protection) {
            return Err(CodeMemoryError::ProtectionFailed {
                operation: "fault injection before mprotect(PROT_READ|PROT_EXEC)",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        let result = unsafe {
            libc::mprotect(
                self.ptr.as_ptr().cast(),
                self.len,
                libc::PROT_READ | libc::PROT_EXEC,
            )
        };
        if result != 0 {
            return Err(CodeMemoryError::ProtectionFailed {
                operation: "mprotect(PROT_READ|PROT_EXEC)",
                raw_code: raw_os_error(),
            });
        }
        if fault == Some(FaultInjection::InstructionCache) {
            return Err(CodeMemoryError::InstructionCacheFlushFailed {
                operation: "fault injection before instruction-cache synchronization",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        unsafe {
            synchronize_instruction_cache(self.ptr.as_ptr(), bytes.len());
        }
        if fault == Some(FaultInjection::CfgRegistration) {
            return Err(CodeMemoryError::CfgRegistrationFailed {
                operation: "fault injection before indirect-target registration",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        Ok(())
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        let _ = unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}

fn raw_os_error() -> i64 {
    i64::from(io::Error::last_os_error().raw_os_error().unwrap_or(-1))
}

unsafe fn unmap(address: *mut std::ffi::c_void, len: usize) {
    let _ = unsafe { libc::munmap(address, len) };
}

#[cfg(all(target_endian = "little", target_arch = "x86_64"))]
unsafe fn synchronize_instruction_cache(_start: *mut u8, _len: usize) {
    std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
}

#[cfg(all(target_endian = "little", target_arch = "aarch64"))]
unsafe fn synchronize_instruction_cache(start: *mut u8, len: usize) {
    use core::arch::asm;

    if len == 0 {
        return;
    }
    let mut ctr_el0: usize;
    unsafe {
        asm!("mrs {ctr}, ctr_el0", ctr = out(reg) ctr_el0, options(nomem, nostack));
    }
    let data_line = 4_usize << ((ctr_el0 >> 16) & 0xf);
    let instruction_line = 4_usize << (ctr_el0 & 0xf);
    let end = start as usize + len;

    let mut address = (start as usize) & !(data_line - 1);
    while address < end {
        unsafe {
            asm!("dc cvau, {address}", address = in(reg) address, options(nostack));
        }
        address += data_line;
    }
    unsafe {
        asm!("dsb ish", options(nostack));
    }

    address = (start as usize) & !(instruction_line - 1);
    while address < end {
        unsafe {
            asm!("ic ivau, {address}", address = in(reg) address, options(nostack));
        }
        address += instruction_line;
    }
    unsafe {
        asm!("dsb ish", "isb", options(nostack));
    }
}

#[cfg(all(test, target_endian = "little", target_arch = "aarch64"))]
mod tests {
    use super::{round_to_page, Mapping};

    fn return_code(value: u16) -> [u8; 8] {
        let mut code = [0_u8; 8];
        let instruction = 0x5280_0000_u32 | (u32::from(value) << 5);
        code[..4].copy_from_slice(&instruction.to_le_bytes());
        code[4..].copy_from_slice(&0xd65f_03c0_u32.to_le_bytes());
        code
    }

    fn execute_across_thread(address: usize) -> i32 {
        std::thread::spawn(move || {
            let entry: unsafe extern "C" fn() -> i32 = unsafe {
                std::mem::transmute::<*const u8, unsafe extern "C" fn() -> i32>(
                    address as *const u8,
                )
            };
            unsafe { entry() }
        })
        .join()
        .expect("cache-test execution thread")
    }

    #[test]
    fn cache_sync_republishes_new_bytes_at_the_same_virtual_address() {
        let len = round_to_page(32).expect("page size");
        let mut first =
            Mapping::allocate(len, 1, crate::platform::MacJitPolicy::ThreadWriteProtect)
                .expect("first mapping");
        first
            .publish(&return_code(41), &[0], None)
            .expect("publish first version");
        let address = first.as_ptr() as usize;
        assert_eq!(execute_across_thread(address), 41);
        drop(first);

        let mut second = Mapping::allocate_at_for_cache_test(len, address as *mut u8)
            .expect("reuse exact virtual address without replacing a live mapping");
        assert_eq!(second.as_ptr() as usize, address);
        second
            .publish(&return_code(42), &[0], None)
            .expect("publish second version");
        assert_eq!(execute_across_thread(address), 42);
    }
}
