use std::collections::HashMap;

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use super::{FunctionKey, PrototypeDependencyToken, ShapeObservation, ShapeToken};

pub(super) fn is_current_generation(generations: &HashMap<u64, u64>, key: FunctionKey) -> bool {
    generations.get(&key.id) == Some(&key.generation)
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DependencyKey {
    Function(FunctionKey),
    Shape(ShapeToken),
    Prototype(PrototypeDependencyToken),
}

impl DependencyKey {
    pub const fn function(function: FunctionKey) -> Self {
        Self::Function(function)
    }
    pub const fn shape(shape: ShapeToken) -> Self {
        Self::Shape(shape)
    }
    pub const fn prototype(prototype: PrototypeDependencyToken) -> Self {
        Self::Prototype(prototype)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DependencyError {
    StaleDependency,
    VersionRegression,
    DependencyLimit,
}

#[derive(Clone, Debug)]
struct DependencyNode {
    version: u64,
    valid: bool,
    dependencies: BTreeSet<DependencyKey>,
}

#[derive(Clone, Debug, Default)]
pub struct DependencyGraph {
    nodes: BTreeMap<DependencyKey, DependencyNode>,
    reverse: BTreeMap<DependencyKey, BTreeSet<DependencyKey>>,
    shape_generations: BTreeMap<u64, u64>,
    prototype_generations: BTreeMap<u64, u64>,
}

impl DependencyGraph {
    pub fn publish_shape(
        &mut self,
        token: ShapeToken,
    ) -> Result<Vec<DependencyKey>, DependencyError> {
        let identity = token.identity();
        let generation = token.generation();
        let invalidated = match self.shape_generations.get(&identity).copied() {
            Some(current) if generation < current => {
                return Err(DependencyError::VersionRegression)
            }
            Some(current) if generation == current => {
                let key = DependencyKey::shape(token);
                if !self.nodes.get(&key).is_some_and(|node| node.valid) {
                    return Err(DependencyError::StaleDependency);
                }
                return Ok(Vec::new());
            }
            Some(current) => {
                self.invalidate(DependencyKey::shape(ShapeToken::new(identity, current)))
            }
            None => Vec::new(),
        };
        self.install(DependencyKey::shape(token), generation, [])?;
        self.shape_generations.insert(identity, generation);
        Ok(invalidated)
    }

    pub fn publish_prototype(
        &mut self,
        token: PrototypeDependencyToken,
    ) -> Result<Vec<DependencyKey>, DependencyError> {
        let identity = token.identity();
        let generation = token.generation();
        let invalidated = match self.prototype_generations.get(&identity).copied() {
            Some(current) if generation < current => {
                return Err(DependencyError::VersionRegression)
            }
            Some(current) if generation == current => {
                let key = DependencyKey::prototype(token);
                if !self.nodes.get(&key).is_some_and(|node| node.valid) {
                    return Err(DependencyError::StaleDependency);
                }
                return Ok(Vec::new());
            }
            Some(current) => self.invalidate(DependencyKey::prototype(
                PrototypeDependencyToken::new(identity, current),
            )),
            None => Vec::new(),
        };
        self.install(DependencyKey::prototype(token), generation, [])?;
        self.prototype_generations.insert(identity, generation);
        Ok(invalidated)
    }

    pub fn install_shape_specialization(
        &mut self,
        key: DependencyKey,
        version: u64,
        observations: &[ShapeObservation],
        polymorphic_limit: usize,
    ) -> Result<(), DependencyError> {
        if observations.len() > polymorphic_limit {
            return Err(DependencyError::DependencyLimit);
        }
        let dependencies = observations.iter().flat_map(|observation| {
            [
                DependencyKey::shape(observation.shape()),
                DependencyKey::prototype(observation.prototype()),
            ]
        });
        self.install(key, version, dependencies)
    }

    pub fn install(
        &mut self,
        key: DependencyKey,
        version: u64,
        dependencies: impl IntoIterator<Item = DependencyKey>,
    ) -> Result<(), DependencyError> {
        if self
            .nodes
            .get(&key)
            .is_some_and(|node| version < node.version)
        {
            return Err(DependencyError::VersionRegression);
        }
        let dependencies = dependencies.into_iter().collect::<BTreeSet<_>>();
        if dependencies
            .iter()
            .any(|dependency| !self.nodes.get(dependency).is_some_and(|node| node.valid))
        {
            return Err(DependencyError::StaleDependency);
        }
        if let Some(old) = self.nodes.get(&key) {
            for dependency in &old.dependencies {
                if let Some(dependents) = self.reverse.get_mut(dependency) {
                    dependents.remove(&key);
                }
            }
        }
        for dependency in &dependencies {
            self.reverse.entry(*dependency).or_default().insert(key);
        }
        self.nodes.insert(
            key,
            DependencyNode {
                version,
                valid: true,
                dependencies,
            },
        );
        Ok(())
    }

    pub fn validate_install(
        &self,
        key: DependencyKey,
        version: u64,
        dependencies: &[(DependencyKey, u64)],
    ) -> bool {
        self.nodes
            .get(&key)
            .is_some_and(|node| node.valid && node.version == version)
            && dependencies.iter().all(|(dependency, expected)| {
                self.nodes
                    .get(dependency)
                    .is_some_and(|node| node.valid && node.version == *expected)
            })
    }

    pub fn invalidate(&mut self, root: DependencyKey) -> Vec<DependencyKey> {
        if !self.nodes.contains_key(&root) && !self.reverse.contains_key(&root) {
            return Vec::new();
        }
        let mut queue = VecDeque::from([root]);
        let mut invalidated = BTreeSet::new();
        while let Some(key) = queue.pop_front() {
            if !invalidated.insert(key) {
                continue;
            }
            if let Some(node) = self.nodes.get_mut(&key) {
                node.valid = false;
            }
            if let Some(dependents) = self.reverse.get(&key) {
                queue.extend(dependents.iter().copied());
            }
        }
        invalidated.into_iter().collect()
    }
}

#[cfg(test)]
mod shape_tests {
    use super::*;
    use crate::runtime::{
        PropertyAttributes, PrototypeDependencyToken, ShapeObservation, ShapeToken,
    };

    fn observation(shape_generation: u64, prototype_generation: u64) -> ShapeObservation {
        ShapeObservation::new(
            ShapeToken::new(10, shape_generation),
            PrototypeDependencyToken::new(20, prototype_generation),
            24,
            PropertyAttributes::WRITABLE,
            crate::runtime::ObservedType::Int32,
        )
    }

    #[test]
    fn shape_and_prototype_generation_changes_recursively_invalidate_dependents() {
        let mut graph = DependencyGraph::default();
        graph.publish_shape(ShapeToken::new(10, 1)).unwrap();
        graph
            .publish_prototype(PrototypeDependencyToken::new(20, 1))
            .unwrap();
        let optimized = DependencyKey::function(FunctionKey::new(1, 1));
        graph
            .install_shape_specialization(optimized, 1, &[observation(1, 1)], 2)
            .unwrap();
        let caller = DependencyKey::function(FunctionKey::new(2, 1));
        graph.install(caller, 1, [optimized]).unwrap();

        let invalidated = graph.publish_shape(ShapeToken::new(10, 2)).unwrap();
        assert!(invalidated.contains(&optimized));
        assert!(invalidated.contains(&caller));
        assert!(!graph.validate_install(optimized, 1, &[]));

        assert_eq!(
            graph.install_shape_specialization(
                DependencyKey::function(FunctionKey::new(3, 1)),
                1,
                &[observation(1, 1)],
                2,
            ),
            Err(DependencyError::StaleDependency)
        );
    }

    #[test]
    fn prototype_generation_change_invalidates_shape_specialization() {
        let mut graph = DependencyGraph::default();
        graph.publish_shape(ShapeToken::new(10, 1)).unwrap();
        graph
            .publish_prototype(PrototypeDependencyToken::new(20, 1))
            .unwrap();
        let optimized = DependencyKey::function(FunctionKey::new(4, 1));
        graph
            .install_shape_specialization(optimized, 1, &[observation(1, 1)], 1)
            .unwrap();

        let invalidated = graph
            .publish_prototype(PrototypeDependencyToken::new(20, 2))
            .unwrap();
        assert!(invalidated.contains(&optimized));
    }

    #[test]
    fn polymorphic_shape_dependencies_are_bounded() {
        let mut graph = DependencyGraph::default();
        graph.publish_shape(ShapeToken::new(10, 1)).unwrap();
        graph.publish_shape(ShapeToken::new(11, 1)).unwrap();
        graph
            .publish_prototype(PrototypeDependencyToken::new(20, 1))
            .unwrap();
        let observations = [
            observation(1, 1),
            ShapeObservation::new(
                ShapeToken::new(11, 1),
                PrototypeDependencyToken::new(20, 1),
                32,
                PropertyAttributes::WRITABLE,
                crate::runtime::ObservedType::Int32,
            ),
        ];

        assert_eq!(
            graph.install_shape_specialization(
                DependencyKey::function(FunctionKey::new(5, 1)),
                1,
                &observations,
                1,
            ),
            Err(DependencyError::DependencyLimit)
        );
    }
}
