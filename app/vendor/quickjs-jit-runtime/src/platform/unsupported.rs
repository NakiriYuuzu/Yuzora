use super::CodeMemoryError;

pub(super) const SUPPORTED: bool = false;
pub(super) const INDIRECT_TARGET_ALIGNMENT: usize = 16;

pub(super) fn round_to_page(_len: usize) -> Result<usize, CodeMemoryError> {
    Err(CodeMemoryError::UnsupportedPlatform)
}

pub(super) fn prepare_mac_jit_policy(_policy: super::MacJitPolicy) -> Result<(), CodeMemoryError> {
    Err(CodeMemoryError::UnsupportedPlatform)
}

pub(super) fn bootstrap_mac_jit_policy(
    _policy: super::MacJitPolicy,
) -> Result<(), CodeMemoryError> {
    Err(CodeMemoryError::UnsupportedPlatform)
}

#[derive(Debug)]
pub(super) struct Mapping;

impl Mapping {
    pub(super) fn allocate(
        _len: usize,
        _owner_id: u64,
        _mac_jit_policy: super::MacJitPolicy,
    ) -> Result<Self, CodeMemoryError> {
        Err(CodeMemoryError::UnsupportedPlatform)
    }

    pub(super) fn as_ptr(&self) -> *const u8 {
        std::ptr::null()
    }

    pub(super) fn publish(
        &mut self,
        _bytes: &[u8],
        _indirect_targets: &[usize],
        _fault: Option<super::FaultInjection>,
    ) -> Result<(), CodeMemoryError> {
        Err(CodeMemoryError::UnsupportedPlatform)
    }
}
