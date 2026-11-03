//! A small least-recently-used cache (commit details, blame, graph rows).

use std::collections::HashMap;
use std::hash::Hash;

pub struct Lru<K, V> {
    cap: usize,
    tick: u64,
    map: HashMap<K, (u64, V)>,
}

impl<K: Hash + Eq + Clone, V: Clone> Lru<K, V> {
    pub fn new(cap: usize) -> Self {
        Self { cap: cap.max(1), tick: 0, map: HashMap::new() }
    }

    pub fn get(&mut self, k: &K) -> Option<V> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(k).map(|e| {
            e.0 = t;
            e.1.clone()
        })
    }

    pub fn put(&mut self, k: K, v: V) {
        self.tick += 1;
        if self.map.len() >= self.cap && !self.map.contains_key(&k) {
            // Evict the oldest eighth at once, so a full cache isn't scanned per insert.
            let mut ages: Vec<u64> = self.map.values().map(|e| e.0).collect();
            ages.sort_unstable();
            let cut = ages[(ages.len() / 8).min(ages.len() - 1)];
            self.map.retain(|_, e| e.0 > cut);
        }
        self.map.insert(k, (self.tick, v));
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_least_recently_used_go_first() {
        let mut c = Lru::new(8);
        for i in 0..8 {
            c.put(i, i * 10);
        }
        assert_eq!(c.get(&0), Some(0));
        c.put(8, 80);
        assert!(c.len() <= 8);
        assert_eq!(c.get(&0), Some(0), "recently read survives");
        assert_eq!(c.get(&1), None, "oldest is gone");
        assert_eq!(c.get(&8), Some(80));
    }
}
