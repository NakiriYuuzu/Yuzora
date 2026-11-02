//! Bounded pre-effect array-mode observations. Profiles choose guards; they
//! never prove that class, length lookup, storage or buffer state is still valid.

use std::collections::BTreeMap;

use super::{FeedbackState, FunctionKey};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ArrayMode {
    Generic = 0,
    Packed = 1,
    Int32 = 2,
    Float64 = 3,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArrayAccess {
    Load,
    Store,
    Length,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArrayHazards(u32);

impl ArrayHazards {
    pub const NONE: Self = Self(0);
    pub const EXOTIC: Self = Self(1 << 8);
    pub const SLOW_ARRAY: Self = Self(1 << 9);
    pub const RESIZABLE: Self = Self(1 << 10);
    pub const DETACHED: Self = Self(1 << 11);
    pub const IMMUTABLE: Self = Self(1 << 12);
    pub const SHARED: Self = Self(1 << 13);
    pub const INVALID_BACKING: Self = Self(1 << 14);
    pub const fn bits(self) -> u32 {
        self.0
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

const STORE_FLAG: u32 = 1 << 6;
const LENGTH_FLAG: u32 = 1 << 7;
const HAZARD_MASK: u32 = ((1 << 15) - 1) & !((1 << 8) - 1);

fn decode(mode: u32, flags: u32) -> Option<(ArrayAccess, ArrayMode, ArrayHazards)> {
    if flags & !(STORE_FLAG | LENGTH_FLAG | HAZARD_MASK) != 0 {
        return None;
    }
    let access = match flags & (STORE_FLAG | LENGTH_FLAG) {
        0 => ArrayAccess::Load,
        STORE_FLAG => ArrayAccess::Store,
        LENGTH_FLAG => ArrayAccess::Length,
        _ => return None,
    };
    let mode = match mode {
        0 => ArrayMode::Generic,
        1 => ArrayMode::Packed,
        2 => ArrayMode::Int32,
        3 => ArrayMode::Float64,
        _ => return None,
    };
    Some((access, mode, ArrayHazards(flags & HAZARD_MASK)))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArrayFeedbackSnapshot {
    function: FunctionKey,
    pc: u32,
    access: ArrayAccess,
    modes: u8,
    hazards: ArrayHazards,
    state: FeedbackState,
}

impl ArrayFeedbackSnapshot {
    pub const fn function(self) -> FunctionKey {
        self.function
    }
    pub const fn pc(self) -> u32 {
        self.pc
    }
    pub const fn access(self) -> ArrayAccess {
        self.access
    }
    pub const fn hazards(self) -> ArrayHazards {
        self.hazards
    }
    pub const fn state(self) -> FeedbackState {
        self.state
    }
    pub fn modes(self) -> impl Iterator<Item = ArrayMode> {
        [
            ArrayMode::Generic,
            ArrayMode::Packed,
            ArrayMode::Int32,
            ArrayMode::Float64,
        ]
        .into_iter()
        .filter(move |mode| self.modes & (1 << *mode as u8) != 0)
    }
    pub fn can_specialize(self) -> bool {
        self.state != FeedbackState::Megamorphic
            && self.hazards.is_empty()
            && self.modes != 0
            && self.modes & 1 == 0
    }
}

#[derive(Debug)]
pub struct ArrayFeedbackTable {
    capacity: usize,
    diversity_limit: u32,
    entries: BTreeMap<(FunctionKey, u32), ArrayFeedbackSnapshot>,
    version: u64,
    dropped: u64,
}

impl ArrayFeedbackTable {
    /// Drops the observations of a freed function generation.
    /// Like `FeedbackTable::forget_function`, this leaves the version alone.
    pub fn forget_function(&mut self, function: FunctionKey) {
        self.entries.retain(|(key, _), _| *key != function);
    }

    pub fn new(capacity: usize, diversity_limit: usize) -> Self {
        Self {
            capacity,
            diversity_limit: diversity_limit.clamp(1, 3) as u32,
            entries: BTreeMap::new(),
            version: 0,
            dropped: 0,
        }
    }

    /// Observation storage is a finite mask per site, not an execution log.
    /// The site count is bounded independently of receiver diversity. No JS
    /// pointers are retained, and repeated observations allocate no memory.
    pub fn observe(
        &mut self,
        function: FunctionKey,
        pc: u32,
        access: ArrayAccess,
        mode: ArrayMode,
        hazards: ArrayHazards,
    ) -> FeedbackState {
        let key = (function, pc);
        if !self.entries.contains_key(&key) && self.entries.len() >= self.capacity {
            self.dropped = self.dropped.saturating_add(1);
            return FeedbackState::Megamorphic;
        }
        let entry = self.entries.entry(key).or_insert(ArrayFeedbackSnapshot {
            function,
            pc,
            access,
            modes: 0,
            hazards: ArrayHazards::NONE,
            state: FeedbackState::Monomorphic,
        });
        let before = *entry;
        entry.modes |= 1 << mode as u8;
        entry.hazards.0 |= hazards.0;
        let diversity = entry.modes.count_ones();
        entry.state = if entry.state == FeedbackState::Megamorphic
            || entry.access != access
            || diversity > self.diversity_limit
        {
            FeedbackState::Megamorphic
        } else if diversity > 1 {
            FeedbackState::Polymorphic
        } else {
            FeedbackState::Monomorphic
        };
        if *entry != before {
            self.version = self.version.saturating_add(1);
        }
        entry.state
    }

    /// Reject malformed wire observations completely before touching a site.
    pub fn observe_raw(
        &mut self,
        function: FunctionKey,
        pc: u32,
        mode: u32,
        flags: u32,
    ) -> Option<FeedbackState> {
        let (access, mode, hazards) = decode(mode, flags)?;
        Some(self.observe(function, pc, access, mode, hazards))
    }

    pub fn snapshot(&self) -> Box<[ArrayFeedbackSnapshot]> {
        self.entries.values().copied().collect()
    }
    pub const fn version(&self) -> u64 {
        self.version
    }
    pub const fn dropped_observations(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finite_array_modes_widen_without_changing_on_repeated_observations() {
        let function = FunctionKey::new(41, 1);
        let mut table = ArrayFeedbackTable::new(2, 2);
        for (mode, state, version) in [
            (ArrayMode::Packed, FeedbackState::Monomorphic, 1),
            (ArrayMode::Packed, FeedbackState::Monomorphic, 1),
            (ArrayMode::Int32, FeedbackState::Polymorphic, 2),
            (ArrayMode::Float64, FeedbackState::Megamorphic, 3),
            (ArrayMode::Packed, FeedbackState::Megamorphic, 3),
        ] {
            assert_eq!(
                table.observe(function, 7, ArrayAccess::Load, mode, ArrayHazards::NONE),
                state
            );
            assert_eq!(table.version(), version);
        }
        let snapshot = table.snapshot();
        assert_eq!(
            snapshot[0].modes().collect::<Vec<_>>(),
            [ArrayMode::Packed, ArrayMode::Int32, ArrayMode::Float64]
        );
        assert!(!snapshot[0].can_specialize());
    }

    #[test]
    fn capacity_generation_hazards_and_snapshot_immutability_are_preserved() {
        let function = FunctionKey::new(42, 1);
        let mut table = ArrayFeedbackTable::new(2, 3);
        table.observe(
            function,
            3,
            ArrayAccess::Length,
            ArrayMode::Int32,
            ArrayHazards::NONE,
        );
        let before = table.snapshot();
        assert_eq!(before.len(), 1);
        assert!(before[0].can_specialize());
        table.observe(
            function,
            3,
            ArrayAccess::Length,
            ArrayMode::Int32,
            ArrayHazards::RESIZABLE,
        );
        let after = table.snapshot();
        assert_eq!(after[0].hazards(), ArrayHazards::RESIZABLE);
        assert!(!after[0].can_specialize());
        assert!(before[0].can_specialize());
        table.observe(
            FunctionKey::new(42, 2),
            3,
            ArrayAccess::Length,
            ArrayMode::Float64,
            ArrayHazards::NONE,
        );
        assert_eq!(table.snapshot().len(), 2);
        let version = table.version();
        assert_eq!(
            table.observe(
                function,
                4,
                ArrayAccess::Load,
                ArrayMode::Packed,
                ArrayHazards::NONE
            ),
            FeedbackState::Megamorphic
        );
        assert_eq!(table.version(), version);
        assert_eq!(table.snapshot().len(), 2);
        assert_eq!(table.dropped_observations(), 1);
        let mut zero = ArrayFeedbackTable::new(0, 3);
        assert_eq!(
            zero.observe(
                function,
                3,
                ArrayAccess::Length,
                ArrayMode::Packed,
                ArrayHazards::NONE
            ),
            FeedbackState::Megamorphic
        );
        assert!(zero.snapshot().is_empty());
    }

    #[test]
    fn malformed_wire_flags_do_not_mutate_feedback_and_access_conflicts_fail_closed() {
        let function = FunctionKey::new(43, 1);
        let mut table = ArrayFeedbackTable::new(1, 3);
        for (mode, flags) in [(4, 0), (1, 1), (1, STORE_FLAG | LENGTH_FLAG), (1, 1 << 31)] {
            assert_eq!(table.observe_raw(function, 5, mode, flags), None);
            assert_eq!(table.version(), 0);
            assert!(table.snapshot().is_empty());
        }
        assert_eq!(
            table.observe_raw(function, 5, 2, LENGTH_FLAG | ArrayHazards::DETACHED.bits()),
            Some(FeedbackState::Monomorphic)
        );
        assert_eq!(table.snapshot()[0].hazards(), ArrayHazards::DETACHED);
        assert!(!table.snapshot()[0].can_specialize());
        assert_eq!(
            table.observe_raw(function, 5, 2, STORE_FLAG),
            Some(FeedbackState::Megamorphic)
        );
    }

    #[test]
    fn aggregate_feedback_retains_array_only_identity_and_counts_capacity_rejections() {
        let function = FunctionKey::new(44, 1);
        let mut table = super::super::FeedbackTable::new(1, 3);
        table.observe_array_raw(function, 8, 1, 0);
        let before = table.snapshot(7);
        assert_eq!(before.function(), Some(function));
        assert!(before.array_at(function, 8).unwrap().can_specialize());
        assert!(before.array_feedback_bytes() > 0);
        let version = table.version();
        table.observe_array_raw(function, 8, 1, 0);
        assert_eq!(table.version(), version);
        table.observe_array_raw(function, 8, 2, ArrayHazards::RESIZABLE.bits());
        assert_eq!(table.version(), version + 1);
        assert!(before.array_at(function, 8).unwrap().can_specialize());
        assert!(!table
            .snapshot(8)
            .array_at(function, 8)
            .unwrap()
            .can_specialize());
        table.observe_array_raw(function, 9, 1, 0);
        assert_eq!(table.dropped_observations(), 1);
        assert_eq!(table.snapshot(9).arrays().len(), 1);
    }
}
