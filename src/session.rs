use std::collections::{HashMap, VecDeque};

/// A HashMap with a maximum size. When full, the oldest entry is evicted.
pub struct BoundedMap<V> {
    map: HashMap<String, V>,
    order: VecDeque<String>,
    max_size: usize,
}

impl<V> BoundedMap<V> {
    pub fn new(max_size: usize) -> Self {
        BoundedMap {
            map: HashMap::new(),
            order: VecDeque::new(),
            max_size,
        }
    }

    pub fn insert(&mut self, key: String, value: V) {
        if self.map.len() >= self.max_size && !self.map.contains_key(&key) {
            if let Some(oldest) = self.order.pop_front() {
                self.map.remove(&oldest);
            }
        }
        if !self.map.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.map.insert(key, value);
    }

    pub fn get(&self, key: &str) -> Option<&V> {
        self.map.get(key)
    }

    pub fn remove(&mut self, key: &str) -> Option<V> {
        if let Some(v) = self.map.remove(key) {
            self.order.retain(|k| k != key);
            Some(v)
        } else {
            None
        }
    }

    /// Find the last value whose key starts with the given prefix.
    /// Iterates from newest to oldest for fast lookups (typically O(1)).
    pub fn find_last_by_prefix(&self, prefix: &str) -> Option<&V> {
        for key in self.order.iter().rev() {
            if key.starts_with(prefix) {
                return self.map.get(key);
            }
        }
        None
    }
}
