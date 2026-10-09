//! Generational arena: dense `Vec` of slots, stable [`Id`]s, free-list reuse.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use serde::{Deserialize, Serialize};

/// Typed handle into an [`Arena<T>`]. `Copy`/`Ord`/`Hash` for any `T`; ordered by
/// `(index, generation)`.
#[derive(Serialize, Deserialize)]
#[serde(bound = "")]
pub struct Id<T> {
    pub index: u32,
    pub generation: u32,
    #[serde(skip)]
    marker: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    pub const fn new(index: u32, generation: u32) -> Self {
        Self {
            index,
            generation,
            marker: PhantomData,
        }
    }
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Id<T> {}
impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.generation == other.generation
    }
}
impl<T> Eq for Id<T> {}
impl<T> PartialOrd for Id<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl<T> Ord for Id<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.index, self.generation).cmp(&(other.index, other.generation))
    }
}
impl<T> Hash for Id<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.index.hash(state);
        self.generation.hash(state);
    }
}
impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({}v{})", self.index, self.generation)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Slot<T> {
    generation: u32,
    value: Option<T>,
}

/// Dense generational arena. Removing a slot bumps its generation, so stale ids
/// stop resolving; the slot is reused by later inserts (most recently freed first).
/// Iteration is strictly ascending by index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Arena<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: u32,
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Arena<T> {
    pub const fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            len: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            free: Vec::new(),
            len: 0,
        }
    }

    /// Number of live elements.
    pub fn len(&self) -> usize {
        self.len as usize
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn insert(&mut self, value: T) -> Id<T> {
        self.len += 1;
        if let Some(index) = self.free.pop() {
            let slot = &mut self.slots[index as usize];
            slot.value = Some(value);
            Id::new(index, slot.generation)
        } else {
            let index = u32::try_from(self.slots.len()).expect("arena exceeds u32 slots");
            self.slots.push(Slot {
                generation: 0,
                value: Some(value),
            });
            Id::new(index, 0)
        }
    }

    pub fn remove(&mut self, id: Id<T>) -> Option<T> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        let value = slot.value.take()?;
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(id.index);
        self.len -= 1;
        Some(value)
    }

    pub fn get(&self, id: Id<T>) -> Option<&T> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation == id.generation {
            slot.value.as_ref()
        } else {
            None
        }
    }

    pub fn get_mut(&mut self, id: Id<T>) -> Option<&mut T> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation == id.generation {
            slot.value.as_mut()
        } else {
            None
        }
    }

    pub fn contains(&self, id: Id<T>) -> bool {
        self.get(id).is_some()
    }

    /// Live elements in ascending index order.
    pub fn iter(&self) -> impl Iterator<Item = (Id<T>, &T)> {
        self.slots.iter().enumerate().filter_map(|(i, slot)| {
            slot.value
                .as_ref()
                .map(|v| (Id::new(i as u32, slot.generation), v))
        })
    }

    /// Live elements in ascending index order, mutable.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (Id<T>, &mut T)> {
        self.slots.iter_mut().enumerate().filter_map(|(i, slot)| {
            let generation = slot.generation;
            slot.value
                .as_mut()
                .map(|v| (Id::new(i as u32, generation), v))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deliberately not Clone/Copy/Hash/Debug.
    struct Plain(u32);

    #[test]
    fn insert_get_remove() {
        let mut a = Arena::new();
        let x = a.insert(Plain(10));
        let y = a.insert(Plain(20));
        assert_eq!(a.len(), 2);
        assert_eq!(a.get(x).unwrap().0, 10);
        a.get_mut(y).unwrap().0 = 21;
        assert_eq!(a.remove(y).unwrap().0, 21);
        assert!(a.get(y).is_none());
        assert!(a.remove(y).is_none());
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn slot_reuse_invalidates_old_id() {
        let mut a = Arena::new();
        let old = a.insert(Plain(1));
        a.remove(old);
        let new = a.insert(Plain(2));
        assert_eq!(new.index, old.index);
        assert_ne!(new.generation, old.generation);
        assert!(a.get(old).is_none());
        assert!(a.get_mut(old).is_none());
        assert!(a.remove(old).is_none());
        assert_eq!(a.get(new).unwrap().0, 2);
        assert_eq!(a.len(), 1);
    }

    #[test]
    fn iteration_is_ascending_by_index() {
        let mut a = Arena::new();
        let ids: Vec<_> = (0..6).map(|i| a.insert(Plain(i))).collect();
        // Free in non-index order, then refill: reuse order must not affect iteration.
        a.remove(ids[4]);
        a.remove(ids[1]);
        a.insert(Plain(100));
        a.insert(Plain(101));
        let indices: Vec<u32> = a.iter().map(|(id, _)| id.index).collect();
        assert_eq!(indices, vec![0, 1, 2, 3, 4, 5]);
        a.remove(ids[0]);
        let indices: Vec<u32> = a.iter_mut().map(|(id, _)| id.index).collect();
        assert_eq!(indices, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn id_is_ord_hash_copy_for_any_t() {
        fn assert_traits<X: Copy + Ord + Hash + Serialize>() {}
        assert_traits::<Id<Plain>>();
        let a: Id<Plain> = Id::new(1, 0);
        let b: Id<Plain> = Id::new(1, 1);
        let c: Id<Plain> = Id::new(2, 0);
        assert!(a < b && b < c);
    }

    #[test]
    fn serde_round_trip_preserves_ids_and_free_list() {
        let mut a: Arena<u32> = Arena::new();
        let ids: Vec<_> = (0..5).map(|i| a.insert(i * 10)).collect();
        a.remove(ids[1]);
        a.remove(ids[3]);
        let bytes = postcard::to_stdvec(&a).unwrap();
        let mut b: Arena<u32> = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(b.len(), 3);
        assert!(b.get(ids[1]).is_none());
        assert_eq!(b.get(ids[2]), Some(&20));
        // Same free list => same next ids.
        assert_eq!(a.insert(7), b.insert(7));
        assert_eq!(a.insert(8), b.insert(8));
        assert_eq!(
            postcard::to_stdvec(&a).unwrap(),
            postcard::to_stdvec(&b).unwrap()
        );
        let id_bytes = postcard::to_stdvec(&ids[2]).unwrap();
        assert_eq!(postcard::from_bytes::<Id<u32>>(&id_bytes).unwrap(), ids[2]);
    }
}
