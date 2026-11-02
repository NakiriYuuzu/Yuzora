//! Exact identity and frame metadata for Tier 1 on-stack replacement.

use crate::bytecode::{SlotKind, VerifiedFunction};

use super::FunctionKey;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OsrKey {
    function: FunctionKey,
    pc: u32,
}

impl OsrKey {
    pub const fn new(function: FunctionKey, pc: u32) -> Self {
        Self { function, pc }
    }

    pub const fn function(self) -> FunctionKey {
        self.function
    }
    pub const fn pc(self) -> u32 {
        self.pc
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OsrMap {
    key: OsrKey,
    entry_offset: u32,
    stack_depth: u16,
    argument_count: u16,
    local_count: u16,
    live_slots: Box<[SlotKind]>,
}

impl OsrMap {
    pub fn new(
        key: OsrKey,
        entry_offset: u32,
        stack_depth: u16,
        live_slots: Vec<SlotKind>,
    ) -> Self {
        Self {
            key,
            entry_offset,
            stack_depth,
            argument_count: 0,
            local_count: 0,
            live_slots: live_slots.into_boxed_slice(),
        }
    }

    /// Binds a separately emitted ABI entry to a verifier-proven loop state.
    /// Offset zero is intentionally reserved for the ordinary function entry.
    pub fn from_verified(function: &VerifiedFunction, pc: u32, entry_offset: u32) -> Option<Self> {
        if entry_offset == 0 {
            return None;
        }
        let point = function
            .osr_points()
            .iter()
            .find(|point| point.pc() == pc)?;
        let snapshot = function.snapshot();
        let fixed = usize::from(snapshot.arg_count())
            .checked_add(usize::from(snapshot.local_count()))?
            .checked_add(usize::from(snapshot.closure_count()))?;
        let stack_depth = point.live_slots().len().checked_sub(fixed)?;
        if snapshot.closure_count() != 0 {
            return None;
        }
        Some(Self {
            key: OsrKey::new(
                FunctionKey::new(snapshot.function_id(), snapshot.generation()),
                pc,
            ),
            entry_offset,
            stack_depth: u16::try_from(stack_depth).ok()?,
            argument_count: snapshot.arg_count(),
            local_count: snapshot.local_count(),
            live_slots: point.live_slots().to_vec().into_boxed_slice(),
        })
    }

    pub const fn key(&self) -> OsrKey {
        self.key
    }
    pub const fn entry_offset(&self) -> u32 {
        self.entry_offset
    }
    pub const fn stack_depth(&self) -> u16 {
        self.stack_depth
    }
    pub const fn argument_count(&self) -> u16 {
        self.argument_count
    }
    pub const fn local_count(&self) -> u16 {
        self.local_count
    }
    pub fn live_slots(&self) -> &[SlotKind] {
        &self.live_slots
    }

    pub fn matches(
        &self,
        function: FunctionKey,
        pc: u32,
        stack_depth: u16,
        slots: &[SlotKind],
    ) -> bool {
        self.key == OsrKey::new(function, pc)
            && self.stack_depth == stack_depth
            && self.live_slots.as_ref() == slots
    }
}
