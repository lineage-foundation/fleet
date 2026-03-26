//! A bounded hash set that evicts the oldest entries when capacity is reached.

use std::collections::{HashSet, VecDeque};
use std::hash::Hash;

/// A bounded set that evicts the oldest entries when capacity is reached.
///
/// Combines a [`HashSet`] for O(1) lookups with a [`VecDeque`] to track
/// insertion order, enabling FIFO eviction of the oldest entry when the
/// set is full.
#[derive(Debug, Clone)]
pub struct BoundedHashSet<T> {
    set: HashSet<T>,
    order: VecDeque<T>,
    capacity: usize,
}

impl<T: Eq + Hash + Copy> BoundedHashSet<T> {
    /// Creates a new `BoundedHashSet` with the given maximum capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            set: HashSet::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Returns `true` if the set contains the value.
    pub fn contains(&self, value: &T) -> bool {
        self.set.contains(value)
    }

    /// Inserts a value into the set.
    ///
    /// Returns `true` if the value was newly inserted, `false` if it was already present.
    /// When the set is at capacity, the oldest entry is evicted before the new one is added.
    pub fn insert(&mut self, value: T) -> bool {
        if !self.set.insert(value) {
            return false;
        }
        if self.order.len() >= self.capacity {
            if let Some(oldest) = self.order.pop_front() {
                self.set.remove(&oldest);
            }
        }
        self.order.push_back(value);
        true
    }

    /// Returns the number of elements in the set.
    pub fn len(&self) -> usize {
        self.set.len()
    }

    /// Returns `true` if the set contains no elements.
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::BoundedHashSet;

    #[test]
    fn insert_and_contains() {
        let mut cache = BoundedHashSet::new(10);
        assert!(cache.insert(1u64));
        assert!(cache.insert(2));
        assert!(cache.contains(&1));
        assert!(cache.contains(&2));
        assert!(!cache.contains(&3));
    }

    #[test]
    fn duplicate_insert_returns_false() {
        let mut cache = BoundedHashSet::new(10);
        assert!(cache.insert(42u64));
        assert!(!cache.insert(42));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.order.len(), 1);
    }

    #[test]
    fn evicts_oldest_when_full() {
        let mut cache = BoundedHashSet::new(3);
        cache.insert(1u64);
        cache.insert(2);
        cache.insert(3);

        // Cache is full, inserting a 4th should evict 1
        cache.insert(4);
        assert!(!cache.contains(&1));
        assert!(cache.contains(&2));
        assert!(cache.contains(&3));
        assert!(cache.contains(&4));
        assert_eq!(cache.len(), 3);
        assert_eq!(cache.order.len(), 3);
    }

    #[test]
    fn eviction_order_is_fifo() {
        let mut cache = BoundedHashSet::new(3);
        cache.insert(10u64);
        cache.insert(20);
        cache.insert(30);

        // Each new insert evicts the oldest remaining entry
        cache.insert(40);
        assert!(!cache.contains(&10));

        cache.insert(50);
        assert!(!cache.contains(&20));

        cache.insert(60);
        assert!(!cache.contains(&30));

        assert!(cache.contains(&40));
        assert!(cache.contains(&50));
        assert!(cache.contains(&60));
    }

    #[test]
    fn capacity_one() {
        let mut cache = BoundedHashSet::new(1);
        cache.insert(1u64);
        assert!(cache.contains(&1));

        cache.insert(2);
        assert!(!cache.contains(&1));
        assert!(cache.contains(&2));
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.order.len(), 1);
    }

    #[test]
    fn set_and_deque_stay_in_sync() {
        let mut cache = BoundedHashSet::new(5);
        for i in 0u64..100 {
            cache.insert(i);
            assert_eq!(cache.len(), cache.order.len());
            assert!(cache.len() <= 5);
        }
        // Only the last 5 should remain
        for i in 95..100 {
            assert!(cache.contains(&i));
        }
        for i in 0..95 {
            assert!(!cache.contains(&i));
        }
    }

    #[test]
    fn works_with_different_types() {
        let mut cache = BoundedHashSet::new(2);
        cache.insert('a');
        cache.insert('b');
        cache.insert('c');
        assert!(!cache.contains(&'a'));
        assert!(cache.contains(&'b'));
        assert!(cache.contains(&'c'));

        let mut cache = BoundedHashSet::new(2);
        cache.insert(1i32);
        cache.insert(2i32);
        cache.insert(3i32);
        assert!(!cache.contains(&1));
        assert!(cache.contains(&2));
        assert!(cache.contains(&3));
    }

    #[test]
    fn is_empty_and_len() {
        let mut cache = BoundedHashSet::<u64>::new(5);
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);

        cache.insert(1);
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
    }
}
