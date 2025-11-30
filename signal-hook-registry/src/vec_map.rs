use std::mem;

/// A small map backed by an unsorted vector.
///
/// Maintains key uniqueness at cost of O(n) lookup/insert/remove. Maintains insertion order
/// (`insert` calls that overwrite an existing value don't change order).
#[derive(Clone, Default)]
pub struct VecMap<K, V>(Vec<(K, V)>);

impl<K: Eq, V> VecMap<K, V> {
    pub fn new() -> Self {
        VecMap(Vec::new())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    fn find(&self, key: &K) -> Option<usize> {
        for (i, (k, _)) in self.0.iter().enumerate() {
            if k == key {
                return Some(i);
            }
        }
        None
    }

    pub fn contains(&self, key: &K) -> bool {
        self.find(key).is_some()
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        match self.find(key) {
            Some(i) => Some(&self.0[i].1),
            None => None,
        }
    }

    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        match self.find(key) {
            Some(i) => Some(&mut self.0[i].1),
            None => None,
        }
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some(old) = self.get_mut(&key) {
            return Some(mem::replace(old, value));
        }
        self.0.push((key, value));
        None
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        match self.find(key) {
            Some(i) => Some(self.0.remove(i).1),
            None => None,
        }
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.0.iter().map(|kv| &kv.1)
    }
}
