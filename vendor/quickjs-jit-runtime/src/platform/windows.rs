use std::ptr::NonNull;

use windows_sys::Win32::{
    Foundation::GetLastError,
    System::{
        Diagnostics::Debug::FlushInstructionCache,
        Memory::{
            SetProcessValidCallTargets, VirtualAlloc, VirtualFree, VirtualProtect,
            CFG_CALL_TARGET_INFO, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ,
            PAGE_READWRITE, PAGE_TARGETS_INVALID, PAGE_TARGETS_NO_UPDATE,
        },
        SystemInformation::{GetSystemInfo, SYSTEM_INFO},
        SystemServices::{CFG_CALL_TARGET_VALID, PROCESS_MITIGATION_CONTROL_FLOW_GUARD_POLICY},
        Threading::{GetCurrentProcess, GetProcessMitigationPolicy, ProcessControlFlowGuardPolicy},
    },
};

use super::{CodeMemoryError, FaultInjection, MacJitPolicy, INJECTED_RAW_CODE};

pub(super) const SUPPORTED: bool = true;
pub(super) const INDIRECT_TARGET_ALIGNMENT: usize = 16;

pub(super) fn round_to_page(len: usize) -> Result<usize, CodeMemoryError> {
    let mut system_info = SYSTEM_INFO::default();
    unsafe {
        GetSystemInfo(&mut system_info);
    }
    let page_size = system_info.dwPageSize as usize;
    if page_size == 0 {
        return Err(CodeMemoryError::PageSizeFailed {
            operation: "GetSystemInfo",
            raw_code: raw_os_error(),
        });
    }
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
        // Reserving with PAGE_TARGETS_INVALID records an all-invalid CFG bitmap
        // without creating an accessible executable page. The separate commit
        // is writable and non-executable.
        let reserved = unsafe {
            VirtualAlloc(
                std::ptr::null(),
                len,
                MEM_RESERVE,
                PAGE_EXECUTE_READ | PAGE_TARGETS_INVALID,
            )
        };
        let Some(reserved) = NonNull::<u8>::new(reserved.cast()) else {
            return Err(CodeMemoryError::MappingFailed {
                operation: "VirtualAlloc(MEM_RESERVE, PAGE_TARGETS_INVALID)",
                raw_code: raw_os_error(),
            });
        };
        let committed =
            unsafe { VirtualAlloc(reserved.as_ptr().cast(), len, MEM_COMMIT, PAGE_READWRITE) };
        if committed.is_null() {
            let raw_code = raw_os_error();
            unsafe {
                VirtualFree(reserved.as_ptr().cast(), 0, MEM_RELEASE);
            }
            return Err(CodeMemoryError::MappingFailed {
                operation: "VirtualAlloc(MEM_COMMIT, PAGE_READWRITE)",
                raw_code,
            });
        }
        debug_assert_eq!(committed.cast::<u8>(), reserved.as_ptr());
        Ok(Self { ptr: reserved, len })
    }

    pub(super) fn as_ptr(&self) -> *const u8 {
        self.ptr.as_ptr()
    }

    pub(super) fn publish(
        &mut self,
        bytes: &[u8],
        indirect_targets: &[usize],
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
                operation: "fault injection before VirtualProtect(PAGE_EXECUTE_READ)",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        let mut old_protection = 0;
        let protected = unsafe {
            VirtualProtect(
                self.ptr.as_ptr().cast(),
                self.len,
                PAGE_EXECUTE_READ | PAGE_TARGETS_NO_UPDATE,
                &mut old_protection,
            )
        };
        if protected == 0 {
            return Err(CodeMemoryError::ProtectionFailed {
                operation: "VirtualProtect(PAGE_EXECUTE_READ|PAGE_TARGETS_NO_UPDATE)",
                raw_code: raw_os_error(),
            });
        }

        if fault == Some(FaultInjection::InstructionCache) {
            return Err(CodeMemoryError::InstructionCacheFlushFailed {
                operation: "fault injection before FlushInstructionCache",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        let process = unsafe { GetCurrentProcess() };
        let flushed =
            unsafe { FlushInstructionCache(process, self.ptr.as_ptr().cast(), bytes.len()) };
        if flushed == 0 {
            return Err(CodeMemoryError::InstructionCacheFlushFailed {
                operation: "FlushInstructionCache",
                raw_code: raw_os_error(),
            });
        }

        if fault == Some(FaultInjection::CfgRegistration) {
            return Err(CodeMemoryError::CfgRegistrationFailed {
                operation: "fault injection before SetProcessValidCallTargets",
                raw_code: INJECTED_RAW_CODE,
            });
        }
        if cfg_enabled(process)? {
            let target_count =
                u32::try_from(indirect_targets.len()).map_err(|_| CodeMemoryError::SizeOverflow)?;
            let mut targets = indirect_targets
                .iter()
                .copied()
                .map(|offset| CFG_CALL_TARGET_INFO {
                    Offset: offset,
                    Flags: CFG_CALL_TARGET_VALID as usize,
                })
                .collect::<Vec<_>>();
            let registered = unsafe {
                SetProcessValidCallTargets(
                    process,
                    self.ptr.as_ptr().cast(),
                    self.len,
                    target_count,
                    targets.as_mut_ptr(),
                )
            };
            if registered == 0 {
                return Err(CodeMemoryError::CfgRegistrationFailed {
                    operation: "SetProcessValidCallTargets",
                    raw_code: raw_os_error(),
                });
            }
        }
        Ok(())
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        unsafe {
            VirtualFree(self.ptr.as_ptr().cast(), 0, MEM_RELEASE);
        }
    }
}

fn cfg_enabled(process: windows_sys::Win32::Foundation::HANDLE) -> Result<bool, CodeMemoryError> {
    let mut policy = PROCESS_MITIGATION_CONTROL_FLOW_GUARD_POLICY::default();
    let queried = unsafe {
        GetProcessMitigationPolicy(
            process,
            ProcessControlFlowGuardPolicy,
            (&mut policy as *mut PROCESS_MITIGATION_CONTROL_FLOW_GUARD_POLICY).cast(),
            size_of::<PROCESS_MITIGATION_CONTROL_FLOW_GUARD_POLICY>(),
        )
    };
    if queried == 0 {
        return Err(CodeMemoryError::CfgRegistrationFailed {
            operation: "GetProcessMitigationPolicy(ProcessControlFlowGuardPolicy)",
            raw_code: raw_os_error(),
        });
    }
    let flags = unsafe { policy.Anonymous.Flags };
    Ok(flags & 1 != 0)
}

fn raw_os_error() -> i64 {
    i64::from(unsafe { GetLastError() })
}
