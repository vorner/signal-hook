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

    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        match self.find(&key) {
            Some(i) => Entry::Occupied(OccupiedEntry {
                place: &mut self.0[i],
            }),
            None => Entry::Vacant(VacantEntry { map: self, key }),
        }
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.0.iter().map(|kv| &kv.1)
    }
}

pub enum Entry<'a, K: 'a, V: 'a> {
    Occupied(OccupiedEntry<'a, K, V>),
    Vacant(VacantEntry<'a, K, V>),
}

pub struct OccupiedEntry<'a, K: 'a, V: 'a> {
    place: &'a mut (K, V),
}

impl<'a, K, V> OccupiedEntry<'a, K, V> {
    pub fn get_mut(&mut self) -> &mut V {
        &mut self.place.1
    }
}

pub struct VacantEntry<'a, K: 'a, V: 'a> {
    map: &'a mut VecMap<K, V>,
    key: K,
}

impl<'a, K, V> VacantEntry<'a, K, V> {
    pub fn insert(self, value: V) {
        self.map.0.push((self.key, value));
    }
}
