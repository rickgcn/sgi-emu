//! Inline collections used for bounded hot-path state.

use core::fmt;

use smallvec::{Array, SmallVec};

pub(crate) struct InlineMap<A>(SmallVec<A>)
where
    A: Array;

impl<A> Clone for InlineMap<A>
where
    A: Array,
    SmallVec<A>: Clone,
{
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<A> fmt::Debug for InlineMap<A>
where
    A: Array,
    SmallVec<A>: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<A> PartialEq for InlineMap<A>
where
    A: Array,
    SmallVec<A>: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<A> Eq for InlineMap<A>
where
    A: Array,
    SmallVec<A>: Eq,
{
}

impl<A> InlineMap<A>
where
    A: Array,
{
    pub(crate) fn new() -> Self {
        Self(SmallVec::new())
    }

    pub(crate) fn insert<K, V>(&mut self, key: K, value: V) -> Option<V>
    where
        A: Array<Item = (K, V)>,
        K: Eq,
    {
        if let Some((_, current)) = self.0.iter_mut().find(|(current, _)| *current == key) {
            return Some(core::mem::replace(current, value));
        }
        self.0.push((key, value));
        None
    }

    pub(crate) fn remove<K, V>(&mut self, key: &K) -> Option<V>
    where
        A: Array<Item = (K, V)>,
        K: Eq,
    {
        let index = self.0.iter().position(|(current, _)| current == key)?;
        Some(self.0.swap_remove(index).1)
    }

    pub(crate) fn iter<'a, K: 'a, V: 'a>(&'a self) -> impl Iterator<Item = (&'a K, &'a V)>
    where
        A: Array<Item = (K, V)>,
    {
        self.0.iter().map(|(key, value)| (key, value))
    }

    pub(crate) fn drain_keys<'a, K: 'a, V: 'a>(&'a mut self) -> impl Iterator<Item = K> + 'a
    where
        A: Array<Item = (K, V)>,
    {
        self.0.drain(..).map(|(key, _)| key)
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    #[cfg(test)]
    pub(crate) fn spilled(&self) -> bool {
        self.0.spilled()
    }
}

pub(crate) struct InlineSet<A>(SmallVec<A>)
where
    A: Array;

impl<A> Clone for InlineSet<A>
where
    A: Array,
    SmallVec<A>: Clone,
{
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<A> fmt::Debug for InlineSet<A>
where
    A: Array,
    SmallVec<A>: fmt::Debug,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl<A> PartialEq for InlineSet<A>
where
    A: Array,
    SmallVec<A>: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl<A> Eq for InlineSet<A>
where
    A: Array,
    SmallVec<A>: Eq,
{
}

impl<A> InlineSet<A>
where
    A: Array,
{
    pub(crate) fn new() -> Self {
        Self(SmallVec::new())
    }

    pub(crate) fn insert<K>(&mut self, value: K) -> bool
    where
        A: Array<Item = K>,
        K: Eq,
    {
        if self.0.contains(&value) {
            return false;
        }
        self.0.push(value);
        true
    }

    pub(crate) fn remove<K>(&mut self, value: &K) -> bool
    where
        A: Array<Item = K>,
        K: Eq,
    {
        let Some(index) = self.0.iter().position(|current| current == value) else {
            return false;
        };
        self.0.swap_remove(index);
        true
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    #[cfg(test)]
    pub(crate) fn spilled(&self) -> bool {
        self.0.spilled()
    }
}

pub(crate) type InlineMap8<K, V> = InlineMap<[(K, V); 8]>;
pub(crate) type InlineMap16<K, V> = InlineMap<[(K, V); 16]>;
pub(crate) type InlineSet16<K> = InlineSet<[K; 16]>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_replace_drain_clear_and_spill() {
        let mut map = InlineMap8::new();
        for key in 0..9 {
            assert_eq!(map.insert(key, key * 2), None);
        }
        assert!(map.spilled());
        assert_eq!(map.insert(4, 99), Some(8));
        assert_eq!(map.remove(&4), Some(99));
        assert_eq!(map.remove(&4), None);
        assert_eq!(map.iter().count(), 8);
        assert_eq!(map.drain_keys().count(), 8);
        assert_eq!(map.iter().count(), 0);

        let mut wide = InlineMap16::new();
        for key in 0..17 {
            wide.insert(key, key);
        }
        assert!(wide.spilled());
        wide.clear();
        assert_eq!(wide.iter().count(), 0);
    }

    #[test]
    fn set_suppresses_duplicates_removes_clears_and_spills() {
        let mut set = InlineSet16::new();
        for key in 0..17 {
            assert!(set.insert(key));
        }
        assert!(set.spilled());
        assert!(!set.insert(3));
        assert!(set.remove(&3));
        assert!(!set.remove(&3));
        set.clear();
        assert!(!set.remove(&0));
    }
}
