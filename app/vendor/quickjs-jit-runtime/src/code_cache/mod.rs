//! Owned compiled artifacts and their runtime cache.

mod artifact;
mod evict;

#[cfg(feature = "compiler")]
pub use artifact::OptimizedArtifactMetadata;
pub use artifact::{
    ArtifactDependency, ArtifactKey, ArtifactVersionIdentity, BenefitSnapshot, CodeAllocation,
    CompiledArtifact, FrameState, FrameStateLocationKind, Relocation, RelocationKind,
    RelocationResolveError, RelocationTarget, ResolvedRelocation, StackMap, UnwindKind,
    UnwindMetadata,
};

use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};

use crate::runtime::{FunctionKey, Tier};

#[derive(Debug)]
pub(super) struct CachedArtifact {
    artifact: CompiledArtifact,
    execution_pins: AtomicUsize,
    deopt_references: AtomicUsize,
    last_used: AtomicU64,
    invalidated: AtomicBool,
    _deopt_target: Option<DeoptPin>,
    charge_bytes: usize,
    code_bytes: usize,
    metadata_bytes: usize,
}

impl CachedArtifact {
    fn new(
        artifact: CompiledArtifact,
        last_used: u64,
        deopt_target: Option<DeoptPin>,
        charge_bytes: usize,
        code_bytes: usize,
        metadata_bytes: usize,
    ) -> Self {
        Self {
            artifact,
            execution_pins: AtomicUsize::new(0),
            deopt_references: AtomicUsize::new(0),
            last_used: AtomicU64::new(last_used),
            invalidated: AtomicBool::new(false),
            _deopt_target: deopt_target,
            charge_bytes,
            code_bytes,
            metadata_bytes,
        }
    }

    pub(super) fn is_evictable(&self) -> bool {
        self.execution_pins.load(Ordering::Acquire) == 0
            && self.deopt_references.load(Ordering::Acquire) == 0
    }

    pub(super) fn eviction_order(&self) -> (u128, u64, ArtifactKey) {
        (
            u128::from(self.artifact.benefit_score()).saturating_mul(1_000_000)
                / (self.charge_bytes.max(1) as u128),
            self.last_used.load(Ordering::Acquire),
            self.artifact.key(),
        )
    }

    pub(super) fn eviction_plan_order(&self) -> (u8, u128, u64, ArtifactKey) {
        let key = self.artifact.key();
        if self.invalidated.load(Ordering::Acquire) {
            let tier_order: u8 = match key.tier {
                Tier::Optimizing => 0,
                Tier::Baseline => 1,
            };
            (0, u128::from(tier_order), 0, key)
        } else {
            let (benefit, last_used, key) = self.eviction_order();
            (1, benefit, last_used, key)
        }
    }

    pub(super) fn deopt_target_key(&self) -> Option<ArtifactKey> {
        self._deopt_target
            .as_ref()
            .map(|pin| pin.target.artifact.key())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CacheError {
    CapacityZero,
    AllArtifactsPinned,
    ArtifactPinned,
    MissingArtifact,
    MissingDeoptTarget,
    ArtifactTooLarge,
    ChargeOverflow,
    PublishFailed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CacheInsert {
    evicted: Box<[ArtifactKey]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReclamationPoll {
    reclaimed: usize,
    may_have_remaining: bool,
}

impl ReclamationPoll {
    pub const fn reclaimed(self) -> usize {
        self.reclaimed
    }

    pub const fn may_have_remaining(self) -> bool {
        self.may_have_remaining
    }
}

impl CacheInsert {
    pub fn evicted(&self) -> Option<ArtifactKey> {
        self.evicted.first().copied()
    }

    pub fn evictions(&self) -> &[ArtifactKey] {
        &self.evicted
    }
}

#[derive(Debug)]
pub struct CodeCache {
    max_bytes: usize,
    max_code_bytes: usize,
    max_metadata_bytes: usize,
    separate_limits: bool,
    charged_bytes: usize,
    charged_code_bytes: usize,
    charged_metadata_bytes: usize,
    clock: u64,
    artifacts: BTreeMap<ArtifactKey, Arc<CachedArtifact>>,
    // Only resident artifacts may be cached here. `remove` clears this owner
    // before uncharging/removing its target, preserving deopt-pin lifetimes.
    last_benefit_target: Option<Arc<CachedArtifact>>,
    reclaim_needed: Arc<AtomicBool>,
}

impl CodeCache {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            max_code_bytes: max_bytes,
            max_metadata_bytes: max_bytes,
            separate_limits: false,
            charged_bytes: 0,
            charged_code_bytes: 0,
            charged_metadata_bytes: 0,
            clock: 0,
            artifacts: BTreeMap::new(),
            last_benefit_target: None,
            reclaim_needed: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn new_with_separate_limits(max_code_bytes: usize, max_metadata_bytes: usize) -> Self {
        let mut cache = Self::new(max_code_bytes.saturating_add(max_metadata_bytes));
        cache.max_code_bytes = max_code_bytes;
        cache.max_metadata_bytes = max_metadata_bytes;
        cache.separate_limits = true;
        cache
    }

    fn next_tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.clock
    }

    pub fn insert(&mut self, artifact: CompiledArtifact) -> Result<CacheInsert, CacheError> {
        if self.max_bytes == 0 {
            return Err(CacheError::CapacityZero);
        }
        let charge_bytes = artifact.charge_bytes().ok_or(CacheError::ChargeOverflow)?;
        let code_bytes = artifact.code_bytes();
        let metadata_bytes = artifact
            .metadata_bytes()
            .ok_or(CacheError::ChargeOverflow)?;
        if self.separate_limits
            && (code_bytes > self.max_code_bytes || metadata_bytes > self.max_metadata_bytes)
        {
            return Err(CacheError::ArtifactTooLarge);
        }
        if charge_bytes > self.max_bytes {
            return Err(CacheError::ArtifactTooLarge);
        }
        let key = artifact.key();
        let deopt_target = if key.tier == Tier::Optimizing {
            let baseline = self
                .artifacts
                .get(&key.with_tier(Tier::Baseline))
                .ok_or(CacheError::MissingDeoptTarget)?;
            Some(DeoptPin::new(
                Arc::clone(baseline),
                Arc::clone(&self.reclaim_needed),
            ))
        } else {
            None
        };
        let (existing_charge, existing_code, existing_metadata) =
            if let Some(existing) = self.artifacts.get(&key) {
                if !existing.is_evictable() {
                    return Err(CacheError::ArtifactPinned);
                }
                (
                    existing.charge_bytes,
                    existing.code_bytes,
                    existing.metadata_bytes,
                )
            } else {
                (0, 0, 0)
            };
        let retained_bytes = self
            .charged_bytes
            .checked_sub(existing_charge)
            .ok_or(CacheError::ChargeOverflow)?;
        let desired_bytes = retained_bytes
            .checked_add(charge_bytes)
            .ok_or(CacheError::ChargeOverflow)?;
        let desired_code = self
            .charged_code_bytes
            .saturating_sub(existing_code)
            .checked_add(code_bytes)
            .ok_or(CacheError::ChargeOverflow)?;
        let desired_metadata = self
            .charged_metadata_bytes
            .saturating_sub(existing_metadata)
            .checked_add(metadata_bytes)
            .ok_or(CacheError::ChargeOverflow)?;
        let needed_bytes = desired_bytes.saturating_sub(self.max_bytes);
        let needed_code = if self.separate_limits {
            desired_code.saturating_sub(self.max_code_bytes)
        } else {
            0
        };
        let needed_metadata = if self.separate_limits {
            desired_metadata.saturating_sub(self.max_metadata_bytes)
        } else {
            0
        };
        let selected = evict::plan(
            &self.artifacts,
            key,
            needed_bytes,
            needed_code,
            needed_metadata,
        )?;
        if existing_charge != 0 || self.artifacts.contains_key(&key) {
            self.remove(key);
        }
        for candidate in &selected {
            self.remove(*candidate);
        }
        let tick = self.next_tick();
        self.charged_bytes = self
            .charged_bytes
            .checked_add(charge_bytes)
            .ok_or(CacheError::ChargeOverflow)?;
        self.charged_code_bytes = self
            .charged_code_bytes
            .checked_add(code_bytes)
            .ok_or(CacheError::ChargeOverflow)?;
        self.charged_metadata_bytes = self
            .charged_metadata_bytes
            .checked_add(metadata_bytes)
            .ok_or(CacheError::ChargeOverflow)?;
        self.artifacts.insert(
            key,
            Arc::new(CachedArtifact::new(
                artifact,
                tick,
                deopt_target,
                charge_bytes,
                code_bytes,
                metadata_bytes,
            )),
        );
        Ok(CacheInsert {
            evicted: selected.into_boxed_slice(),
        })
    }

    fn remove(&mut self, key: ArtifactKey) -> Option<Arc<CachedArtifact>> {
        if self
            .last_benefit_target
            .as_ref()
            .is_some_and(|target| target.artifact.key() == key)
        {
            self.last_benefit_target = None;
        }
        let artifact = self.artifacts.remove(&key)?;
        self.charged_bytes = self.charged_bytes.saturating_sub(artifact.charge_bytes);
        self.charged_code_bytes = self.charged_code_bytes.saturating_sub(artifact.code_bytes);
        self.charged_metadata_bytes = self
            .charged_metadata_bytes
            .saturating_sub(artifact.metadata_bytes);
        Some(artifact)
    }

    pub fn contains(&self, key: ArtifactKey) -> bool {
        self.artifacts.contains_key(&key)
    }

    pub fn touch(&mut self, key: ArtifactKey) -> bool {
        let tick = self.next_tick();
        let Some(artifact) = self.artifacts.get(&key) else {
            return false;
        };
        artifact.last_used.store(tick, Ordering::Release);
        true
    }

    pub fn record_benefit(&mut self, key: ArtifactKey, benefit: u64) -> Result<(), CacheError> {
        let tick = self.next_tick();
        if self
            .last_benefit_target
            .as_ref()
            .is_none_or(|target| target.artifact.key() != key)
        {
            self.last_benefit_target = Some(Arc::clone(
                self.artifacts
                    .get(&key)
                    .ok_or(CacheError::MissingArtifact)?,
            ));
        }
        // Repeated short native returns borrow the resident Arc without either
        // another tree search or another reference-count operation.
        let artifact = self.last_benefit_target.as_ref().unwrap();
        artifact.artifact.record_benefit(benefit);
        artifact.last_used.store(tick, Ordering::Release);
        Ok(())
    }

    pub fn pin(&mut self, key: ArtifactKey) -> Option<ExecutionPin> {
        let tick = self.next_tick();
        let artifact = Arc::clone(self.artifacts.get(&key)?);
        if artifact.invalidated.load(Ordering::Acquire) {
            return None;
        }
        artifact.execution_pins.fetch_add(1, Ordering::AcqRel);
        artifact.last_used.store(tick, Ordering::Release);
        Some(ExecutionPin {
            artifact,
            reclaim_needed: Arc::clone(&self.reclaim_needed),
        })
    }

    pub fn len(&self) -> usize {
        self.artifacts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.artifacts.is_empty()
    }

    pub const fn charged_bytes(&self) -> usize {
        self.charged_bytes
    }

    pub const fn charged_code_bytes(&self) -> usize {
        self.charged_code_bytes
    }
    pub const fn charged_metadata_bytes(&self) -> usize {
        self.charged_metadata_bytes
    }

    pub fn deopt_references(&self, key: ArtifactKey) -> Option<usize> {
        self.artifacts
            .get(&key)
            .map(|artifact| artifact.deopt_references.load(Ordering::Acquire))
    }

    pub fn invalidate(&mut self, function: FunctionKey) -> usize {
        self.invalidate_deferred(function);
        self.collect_invalidated()
    }

    pub(crate) fn invalidate_deferred(&mut self, function: FunctionKey) {
        let mut matched = false;
        for (key, artifact) in &self.artifacts {
            if key.function_id == function.id && key.generation == function.generation {
                artifact.invalidated.store(true, Ordering::Release);
                matched = true;
            }
        }
        if matched {
            self.reclaim_needed.store(true, Ordering::Release);
        }
    }

    pub fn collect_invalidated(&mut self) -> usize {
        self.reclaim_needed.store(false, Ordering::Release);
        let poll = self.collect_invalidated_with_budget(usize::MAX);
        if poll.may_have_remaining {
            self.reclaim_needed.store(true, Ordering::Release);
        }
        poll.reclaimed
    }

    pub fn collect_invalidated_with_budget(&mut self, budget: usize) -> ReclamationPoll {
        let mut reclaimed = 0usize;
        while reclaimed < budget {
            let Some(candidate) = self.next_invalidated_candidate() else {
                break;
            };
            self.remove(candidate);
            reclaimed += 1;
        }
        ReclamationPoll {
            reclaimed,
            may_have_remaining: self.next_invalidated_candidate().is_some(),
        }
    }

    /// Reclaims invalidated artifacts after a pin release has made progress possible.
    pub fn poll_reclamation(&mut self) -> usize {
        self.poll_reclamation_with_budget(usize::MAX).reclaimed
    }

    pub fn poll_reclamation_with_budget(&mut self, budget: usize) -> ReclamationPoll {
        if !self.reclaim_needed.swap(false, Ordering::AcqRel) {
            return ReclamationPoll {
                reclaimed: 0,
                may_have_remaining: false,
            };
        }
        let poll = self.collect_invalidated_with_budget(budget);
        if poll.may_have_remaining {
            self.reclaim_needed.store(true, Ordering::Release);
        }
        poll
    }

    pub(crate) fn reclamation_requested(&self) -> bool {
        self.reclaim_needed.load(Ordering::Acquire)
    }

    fn next_invalidated_candidate(&self) -> Option<ArtifactKey> {
        self.artifacts
            .iter()
            .filter(|(_, artifact)| {
                artifact.invalidated.load(Ordering::Acquire) && artifact.is_evictable()
            })
            .min_by_key(|(key, _)| {
                let tier_order = match key.tier {
                    Tier::Optimizing => 0,
                    Tier::Baseline => 1,
                };
                (tier_order, **key)
            })
            .map(|(key, _)| *key)
    }
}

#[derive(Debug)]
struct DeoptPin {
    target: Arc<CachedArtifact>,
    reclaim_needed: Arc<AtomicBool>,
}

impl DeoptPin {
    fn new(target: Arc<CachedArtifact>, reclaim_needed: Arc<AtomicBool>) -> Self {
        target.deopt_references.fetch_add(1, Ordering::AcqRel);
        Self {
            target,
            reclaim_needed,
        }
    }
}

impl Drop for DeoptPin {
    fn drop(&mut self) {
        let previous = self.target.deopt_references.fetch_sub(1, Ordering::AcqRel);
        if previous == 1 && self.target.invalidated.load(Ordering::Acquire) {
            self.reclaim_needed.store(true, Ordering::Release);
        }
    }
}

#[derive(Debug)]
pub struct ExecutionPin {
    artifact: Arc<CachedArtifact>,
    reclaim_needed: Arc<AtomicBool>,
}

impl ExecutionPin {
    pub fn key(&self) -> ArtifactKey {
        self.artifact.artifact.key()
    }

    pub fn artifact(&self) -> &CompiledArtifact {
        &self.artifact.artifact
    }
}

impl Drop for ExecutionPin {
    fn drop(&mut self) {
        let previous = self.artifact.execution_pins.fetch_sub(1, Ordering::AcqRel);
        if previous == 1 && self.artifact.invalidated.load(Ordering::Acquire) {
            self.reclaim_needed.store(true, Ordering::Release);
        }
    }
}

#[cfg(test)]
mod benefit_tests {
    use super::*;

    fn artifact(id: u64, tier: Tier) -> CompiledArtifact {
        let mut key = CompiledArtifact::fake(tier).key();
        key.function_id = id;
        CompiledArtifact::from_parts(
            key,
            CodeAllocation::inert(vec![0]),
            vec![],
            vec![],
            vec![],
            vec![],
        )
    }

    #[test]
    fn repeated_benefit_preserves_exact_counts_ticks_and_eviction_order() {
        let mut cache = CodeCache::new(2);
        let a = artifact(1, Tier::Baseline).key();
        let b = artifact(2, Tier::Baseline).key();
        cache.insert(artifact(1, Tier::Baseline)).unwrap();
        cache.insert(artifact(2, Tier::Baseline)).unwrap();
        cache.record_benefit(a, 10).unwrap();
        cache.record_benefit(b, 10).unwrap();
        // Equal benefit, but the repeated zero update must still touch b.
        cache.touch(a);
        cache.record_benefit(b, 0).unwrap();
        let resident = &cache.artifacts[&b];
        assert_eq!(
            resident.artifact.benefit(),
            BenefitSnapshot {
                executions: 2,
                score: 10
            }
        );
        assert_eq!(resident.last_used.load(Ordering::Acquire), 6);
        assert_eq!(
            cache.insert(artifact(3, Tier::Baseline)).unwrap().evicted(),
            Some(a)
        );
    }

    #[test]
    fn benefit_target_replacement_drops_old_artifact_and_starts_new_counters() {
        let mut cache = CodeCache::new(2);
        let key = artifact(1, Tier::Baseline).key();
        cache.insert(artifact(1, Tier::Baseline)).unwrap();
        let old = Arc::downgrade(&cache.artifacts[&key]);
        cache.record_benefit(key, 10).unwrap();
        cache.insert(artifact(1, Tier::Baseline)).unwrap();
        assert!(old.upgrade().is_none());
        cache.record_benefit(key, 7).unwrap();
        assert_eq!(
            cache.artifacts[&key].artifact.benefit(),
            BenefitSnapshot {
                executions: 1,
                score: 7
            }
        );
    }

    #[test]
    fn benefit_target_does_not_retain_evicted_artifact() {
        let mut cache = CodeCache::new(1);
        let key = artifact(1, Tier::Baseline).key();
        cache.insert(artifact(1, Tier::Baseline)).unwrap();
        let old = Arc::downgrade(&cache.artifacts[&key]);
        cache.record_benefit(key, 10).unwrap();
        cache.insert(artifact(2, Tier::Baseline)).unwrap();
        assert!(old.upgrade().is_none());
        assert_eq!(
            cache.record_benefit(key, 3),
            Err(CacheError::MissingArtifact)
        );
        assert_eq!(
            cache.clock, 4,
            "even a missing target advances the cache clock"
        );
    }

    #[test]
    fn benefit_target_does_not_delay_deopt_dependency_reclamation() {
        let mut cache = CodeCache::new(2);
        let key = artifact(1, Tier::Optimizing).key();
        cache.insert(artifact(1, Tier::Baseline)).unwrap();
        cache.insert(artifact(1, Tier::Optimizing)).unwrap();
        let pin = cache.pin(key).unwrap();
        let old = Arc::downgrade(&cache.artifacts[&key]);
        cache.record_benefit(key, 10).unwrap();
        assert_eq!(cache.invalidate(FunctionKey::new(1, 0)), 0);
        // An invalidated but still resident artifact retains existing recording
        // semantics; it becomes inaccessible only when actually removed.
        cache.record_benefit(key, 20).unwrap();
        assert_eq!(pin.artifact().benefit().score, 30);
        drop(pin);
        assert_eq!(cache.poll_reclamation(), 2);
        assert!(old.upgrade().is_none());
        assert_eq!(cache.charged_bytes(), 0);
    }
}
