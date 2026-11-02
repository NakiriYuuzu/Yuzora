use std::collections::BTreeMap;

use super::{FunctionKey, ObservedType};

/// Maximum number of receiver shapes one property site may record before it
/// becomes megamorphic. Both native tiers emit at most this many inline
/// shape/generation compares per site; a megamorphic site uses the generic
/// property helper instead of guarding (and deoptimizing) on a shape.
pub const POLYMORPHIC_PROPERTY_LIMIT: usize = 4;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ShapeToken {
    identity: u64,
    generation: u64,
}

impl ShapeToken {
    pub const fn new(identity: u64, generation: u64) -> Self {
        Self {
            identity,
            generation,
        }
    }
    pub const fn identity(self) -> u64 {
        self.identity
    }
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PrototypeDependencyToken {
    identity: u64,
    generation: u64,
}

impl PrototypeDependencyToken {
    pub const fn new(identity: u64, generation: u64) -> Self {
        Self {
            identity,
            generation,
        }
    }
    pub const fn identity(self) -> u64 {
        self.identity
    }
    pub const fn generation(self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PropertyAttributes(u8);

impl PropertyAttributes {
    pub const NONE: Self = Self(0);
    pub const WRITABLE: Self = Self(1 << 0);
    pub const ENUMERABLE: Self = Self(1 << 1);
    pub const CONFIGURABLE: Self = Self(1 << 2);
    pub const ACCESSOR: Self = Self(1 << 3);

    pub const fn bits(self) -> u8 {
        self.0
    }
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & 0x0f)
    }
    pub const fn contains(self, attributes: Self) -> bool {
        self.0 & attributes.0 == attributes.0
    }
}

impl core::ops::BitOr for PropertyAttributes {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ShapeObservation {
    shape: ShapeToken,
    prototype: PrototypeDependencyToken,
    offset: u32,
    attributes: PropertyAttributes,
    value: ObservedType,
}

impl ShapeObservation {
    pub const fn new(
        shape: ShapeToken,
        prototype: PrototypeDependencyToken,
        offset: u32,
        attributes: PropertyAttributes,
        value: ObservedType,
    ) -> Self {
        Self {
            shape,
            prototype,
            offset,
            attributes,
            value,
        }
    }
    pub const fn shape(self) -> ShapeToken {
        self.shape
    }
    pub const fn prototype(self) -> PrototypeDependencyToken {
        self.prototype
    }
    pub const fn offset(self) -> u32 {
        self.offset
    }
    pub const fn attributes(self) -> PropertyAttributes {
        self.attributes
    }
    pub const fn value(self) -> ObservedType {
        self.value
    }
    pub const fn tokens_are_current(
        self,
        shape: ShapeToken,
        prototype: PrototypeDependencyToken,
    ) -> bool {
        self.shape.identity == shape.identity
            && self.shape.generation == shape.generation
            && self.prototype.identity == prototype.identity
            && self.prototype.generation == prototype.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShapeFeedbackState {
    Monomorphic,
    Polymorphic,
    Megamorphic,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ShapeFeedbackSite {
    state: ShapeFeedbackState,
    observations: Vec<ShapeObservation>,
}

impl ShapeFeedbackSite {
    pub const fn state(&self) -> ShapeFeedbackState {
        self.state
    }
    pub fn observations(&self) -> &[ShapeObservation] {
        &self.observations
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct ShapeFeedbackKey {
    function: FunctionKey,
    pc: u32,
}

#[derive(Clone, Debug)]
pub struct ShapeFeedbackTable {
    polymorphic_limit: usize,
    sites: BTreeMap<ShapeFeedbackKey, ShapeFeedbackSite>,
    /// Advances whenever any site's state or observation list changes.
    version: u64,
}

impl ShapeFeedbackTable {
    pub fn new(polymorphic_limit: usize) -> Self {
        Self {
            polymorphic_limit: polymorphic_limit.max(1),
            sites: BTreeMap::new(),
            version: 0,
        }
    }

    /// Monotonic change counter for site states and observation sets.
    /// Repeating a recorded observation only refreshes its recency, so
    /// steady-state probes do not invalidate consumers that cache decisions
    /// derived from snapshots.
    pub const fn version(&self) -> u64 {
        self.version
    }

    pub fn observe(
        &mut self,
        function: FunctionKey,
        pc: u32,
        observation: ShapeObservation,
    ) -> ShapeFeedbackState {
        let site = self
            .sites
            .entry(ShapeFeedbackKey { function, pc })
            .or_insert_with(|| ShapeFeedbackSite {
                state: ShapeFeedbackState::Monomorphic,
                observations: Vec::with_capacity(self.polymorphic_limit),
            });
        if site.state == ShapeFeedbackState::Megamorphic {
            return site.state;
        }
        // Each observation leaves at most one entry per shape identity, so an
        // exact match of the newest entry is a repeat that changes nothing,
        // and an exact match elsewhere only moves it to the end.
        if site.observations.last() == Some(&observation) {
            return site.state;
        }
        if !site.observations.contains(&observation) {
            self.version = self.version.wrapping_add(1);
        }
        // A generation change for the same shape identity is invalidation,
        // not polymorphism. Forget the stale layout so recompilation never
        // emits a guard for a class/layout version that can no longer match.
        // At one exact bytecode/property site a shape token identifies one
        // layout. If metadata for that token changes, the newest observation
        // replaces it; keeping both would make the first matching guard select
        // a stale offset or attribute set.
        site.observations
            .retain(|current| current.shape().identity() != observation.shape().identity());
        if !site.observations.contains(&observation) {
            if site.observations.len() >= self.polymorphic_limit {
                site.observations.clear();
                site.state = ShapeFeedbackState::Megamorphic;
                return site.state;
            }
            site.observations.push(observation);
        }
        site.state = if site.observations.len() == 1 {
            ShapeFeedbackState::Monomorphic
        } else {
            ShapeFeedbackState::Polymorphic
        };
        site.state
    }

    pub fn get(&self, function: FunctionKey, pc: u32) -> Option<&ShapeFeedbackSite> {
        self.sites.get(&ShapeFeedbackKey { function, pc })
    }
    pub fn snapshot(&self, function: FunctionKey) -> Vec<(u32, ShapeFeedbackSite)> {
        self.sites
            .iter()
            .filter(|(key, _)| key.function == function)
            .map(|(key, site)| (key.pc, site.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(shape: u64, prototype_generation: u64, offset: u32) -> ShapeObservation {
        ShapeObservation::new(
            ShapeToken::new(shape, 1),
            PrototypeDependencyToken::new(9, prototype_generation),
            offset,
            PropertyAttributes::WRITABLE | PropertyAttributes::ENUMERABLE,
            ObservedType::Int32,
        )
    }

    #[test]
    fn sites_are_keyed_by_exact_function_generation_and_pc() {
        let mut table = ShapeFeedbackTable::new(2);
        let current = FunctionKey::new(7, 2);
        let old = FunctionKey::new(7, 1);
        table.observe(current, 11, observation(1, 3, 16));
        table.observe(current, 12, observation(2, 3, 24));
        table.observe(old, 11, observation(3, 3, 32));

        assert_eq!(
            table.get(current, 11).unwrap().observations()[0].offset(),
            16
        );
        assert_eq!(
            table.get(current, 12).unwrap().observations()[0].offset(),
            24
        );
        assert_eq!(table.get(old, 11).unwrap().observations()[0].offset(), 32);
    }

    #[test]
    fn lattice_widens_monotonically_and_is_bounded() {
        let mut table = ShapeFeedbackTable::new(2);
        let function = FunctionKey::new(4, 1);
        assert_eq!(
            table.observe(function, 8, observation(1, 1, 8)),
            ShapeFeedbackState::Monomorphic
        );
        assert_eq!(
            table.observe(function, 8, observation(2, 1, 16)),
            ShapeFeedbackState::Polymorphic
        );
        assert_eq!(
            table.observe(function, 8, observation(3, 1, 24)),
            ShapeFeedbackState::Megamorphic
        );
        assert_eq!(
            table.observe(function, 8, observation(1, 1, 8)),
            ShapeFeedbackState::Megamorphic
        );
        assert!(table.get(function, 8).unwrap().observations().is_empty());
    }

    #[test]
    fn version_tracks_observation_sets_but_not_repeats_or_recency() {
        let mut table = ShapeFeedbackTable::new(3);
        let function = FunctionKey::new(4, 1);
        let start = table.version();
        table.observe(function, 8, observation(1, 1, 8));
        let first = table.version();
        assert_ne!(first, start);
        table.observe(function, 8, observation(1, 1, 8));
        assert_eq!(table.version(), first);
        table.observe(function, 8, observation(2, 1, 16));
        let second = table.version();
        assert_ne!(second, first);
        // Alternating known shapes only reorders the site.
        table.observe(function, 8, observation(1, 1, 8));
        table.observe(function, 8, observation(2, 1, 16));
        assert_eq!(table.version(), second);
        assert_eq!(
            table.get(function, 8).unwrap().state(),
            ShapeFeedbackState::Polymorphic
        );
        // A new layout for a known shape identity replaces it.
        table.observe(function, 8, observation(1, 1, 24));
        assert_ne!(table.version(), second);
        let third = table.version();
        table.observe(function, 8, observation(3, 1, 32));
        table.observe(function, 8, observation(4, 1, 40));
        assert_ne!(table.version(), third);
        let megamorphic = table.version();
        table.observe(function, 8, observation(5, 1, 48));
        assert_eq!(table.version(), megamorphic);
    }

    #[test]
    fn observation_requires_exact_shape_and_prototype_generations() {
        let observed = observation(5, 7, 40);
        assert!(
            observed.tokens_are_current(ShapeToken::new(5, 1), PrototypeDependencyToken::new(9, 7))
        );
        assert!(!observed
            .tokens_are_current(ShapeToken::new(5, 2), PrototypeDependencyToken::new(9, 7)));
        assert!(!observed
            .tokens_are_current(ShapeToken::new(5, 1), PrototypeDependencyToken::new(9, 8)));
    }

    #[test]
    fn a_new_generation_replaces_the_stale_layout_for_the_same_shape() {
        let mut table = ShapeFeedbackTable::new(3);
        let function = FunctionKey::new(4, 1);
        table.observe(function, 8, observation(5, 1, 8));
        let replacement = ShapeObservation::new(
            ShapeToken::new(5, 2),
            PrototypeDependencyToken::new(9, 1),
            24,
            PropertyAttributes::WRITABLE,
            ObservedType::Int32,
        );
        assert_eq!(
            table.observe(function, 8, replacement),
            ShapeFeedbackState::Monomorphic
        );
        assert_eq!(
            table.get(function, 8).unwrap().observations(),
            &[replacement]
        );
    }
}
