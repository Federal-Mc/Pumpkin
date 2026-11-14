use std::sync::Mutex;

use pumpkin_util::math::vector2::Vector2;
use rustc_hash::{FxHashMap, FxHashSet};
use uuid::Uuid;

struct Slot<T> {
    id: i32,
    uuid: Uuid,
    value: T,
}

#[derive(Default)]
struct Inner<T> {
    chunks: FxHashMap<Vector2<i32>, Vec<Slot<T>>>,
    ids: FxHashMap<i32, Vector2<i32>>,
}

pub struct DecorStore<T> {
    inner: Mutex<Inner<T>>,
}

impl<T> Default for DecorStore<T> {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                chunks: FxHashMap::default(),
                ids: FxHashMap::default(),
            }),
        }
    }
}

impl<T: Clone> DecorStore<T> {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner<T>> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn insert(&self, chunk: Vector2<i32>, id: i32, uuid: Uuid, value: T) -> bool {
        let mut inner = self.lock();
        if inner.ids.contains_key(&id) {
            return false;
        }
        let slots = inner.chunks.entry(chunk).or_default();
        if slots.iter().any(|slot| slot.uuid == uuid) {
            return false;
        }
        slots.push(Slot { id, uuid, value });
        inner.ids.insert(id, chunk);
        true
    }

    #[must_use]
    pub fn get(&self, id: i32) -> Option<T> {
        let inner = self.lock();
        let chunk = inner.ids.get(&id)?;
        inner
            .chunks
            .get(chunk)?
            .iter()
            .find(|slot| slot.id == id)
            .map(|slot| slot.value.clone())
    }

    pub fn remove(&self, id: i32) -> Option<T> {
        let mut inner = self.lock();
        let chunk = inner.ids.remove(&id)?;
        let slots = inner.chunks.get_mut(&chunk)?;
        let index = slots.iter().position(|slot| slot.id == id)?;
        let slot = slots.swap_remove(index);
        if slots.is_empty() {
            inner.chunks.remove(&chunk);
        }
        Some(slot.value)
    }

    #[must_use]
    pub fn in_chunks<'a>(&self, chunks: impl IntoIterator<Item = &'a Vector2<i32>>) -> Vec<T> {
        let inner = self.lock();
        chunks
            .into_iter()
            .filter_map(|chunk| inner.chunks.get(chunk))
            .flatten()
            .map(|slot| slot.value.clone())
            .collect()
    }

    pub fn take_chunks(&self, chunks: &FxHashSet<Vector2<i32>>) -> Vec<T> {
        let mut inner = self.lock();
        let mut taken = Vec::new();
        for chunk in chunks {
            if let Some(slots) = inner.chunks.remove(chunk) {
                for slot in slots {
                    inner.ids.remove(&slot.id);
                    taken.push(slot.value);
                }
            }
        }
        taken
    }

    pub fn take_all(&self) -> Vec<T> {
        let mut inner = self.lock();
        inner.ids.clear();
        std::mem::take(&mut inner.chunks)
            .into_values()
            .flatten()
            .map(|slot| slot.value)
            .collect()
    }

    #[must_use]
    pub fn all(&self) -> Vec<T> {
        self.lock()
            .chunks
            .values()
            .flatten()
            .map(|slot| slot.value.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uuid(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    #[test]
    fn keeps_entries_by_chunk_and_id() {
        let store = DecorStore::default();
        let a = Vector2::new(0, 0);
        let b = Vector2::new(-1, 3);
        assert!(store.insert(a, 1, uuid(1), "a1"));
        assert!(store.insert(a, 2, uuid(2), "a2"));
        assert!(store.insert(b, 3, uuid(3), "b3"));
        assert!(!store.insert(a, 1, uuid(9), "dup id"));
        assert!(!store.insert(a, 7, uuid(2), "dup uuid"));
        assert_eq!(store.all().len(), 3);
        assert_eq!(store.get(3), Some("b3"));
        assert_eq!(store.get(4), None);

        let mut in_a = store.in_chunks(&[a, Vector2::new(5, 5)]);
        in_a.sort_unstable();
        assert_eq!(in_a, ["a1", "a2"]);

        assert_eq!(store.remove(2), Some("a2"));
        assert_eq!(store.remove(2), None);
        assert_eq!(store.get(2), None);
        assert_eq!(store.in_chunks(&[a]), ["a1"]);
    }

    #[test]
    fn takes_whole_chunks() {
        let store = DecorStore::default();
        let a = Vector2::new(2, 2);
        let b = Vector2::new(3, 2);
        store.insert(a, 1, uuid(1), 10);
        store.insert(a, 2, uuid(2), 20);
        store.insert(b, 3, uuid(3), 30);

        let mut taken = store.take_chunks(&std::iter::once(a).collect());
        taken.sort_unstable();
        assert_eq!(taken, [10, 20]);
        assert_eq!(store.get(1), None);
        assert_eq!(store.all().len(), 1);
        assert!(store.insert(a, 1, uuid(1), 11));

        let mut all = store.take_all();
        all.sort_unstable();
        assert_eq!(all, [11, 30]);
        assert!(store.all().is_empty());
        assert!(store.in_chunks(&[a, b]).is_empty());
    }
}
