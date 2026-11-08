use alloc::boxed::Box;
use core::{
    any::{Any, TypeId},
    hash::Hash,
    ops::{Deref, DerefMut},
};

use bevy_platform::{
    collections::HashMap,
    hash::{Hashed, NoOpHash, PassHash},
};
use indexmap::map::IndexMap;

/// The [`hash_map::Entry`][bevy_platform::collections::hash_map::Entry] type for [`TypeIdHashMap`].
pub use bevy_platform::collections::hash_map::Entry as TypeIdHashMapEntry;

/// The [`Entry`][indexmap::map::Entry] type for [`TypeIdIndexMap`].
pub use indexmap::map::Entry as TypeIdIndexMapEntry;

/// A [`HashMap`] pre-configured to use [`Hashed`] keys and [`PassHash`] passthrough hashing.
/// Iteration order only depends on the order of insertions and deletions.
pub type PreHashMap<K, V> = HashMap<Hashed<K>, V, PassHash>;

/// Extension methods intended to add functionality to [`PreHashMap`].
pub trait PreHashMapExt<K, V> {
    /// Tries to get or insert the value for the given `key` using the pre-computed hash first.
    /// If the [`PreHashMap`] does not already contain the `key`, it will clone it and insert
    /// the value returned by `func`.
    fn get_or_insert_with<F: FnOnce() -> V>(&mut self, key: &Hashed<K>, func: F) -> &mut V;
}

impl<K: Hash + Eq + PartialEq + Clone, V> PreHashMapExt<K, V> for PreHashMap<K, V> {
    #[inline]
    fn get_or_insert_with<F: FnOnce() -> V>(&mut self, key: &Hashed<K>, func: F) -> &mut V {
        use bevy_platform::collections::hash_map::RawEntryMut;
        let entry = self
            .raw_entry_mut()
            .from_key_hashed_nocheck(key.hash(), key);
        match entry {
            RawEntryMut::Occupied(entry) => entry.into_mut(),
            RawEntryMut::Vacant(entry) => {
                let (_, value) = entry.insert_hashed_nocheck(key.hash(), key.clone(), func());
                value
            }
        }
    }
}

/// A specialized hash map type with a key of [`TypeId`].
pub type TypeIdHashMap<V> = HashMap<TypeId, V, NoOpHash>;

/// A specialized index map type with a key of [`TypeId`].
/// Iteration order only depends on the order of insertions and deletions.
pub type TypeIdIndexMap<V> = IndexMap<TypeId, V, NoOpHash>;

/// Extension trait to make use of [`TypeIdIndexMap`] more ergonomic.
///
/// Each function on this trait is a trivial wrapper for a map function,
/// replacing a `TypeId` key with a generic parameter `T`.
///
/// # Examples
///
/// ```rust
/// # use std::any::TypeId;
/// # use bevy_utils::TypeIdIndexMap;
/// use bevy_utils::TypeIdMapExt;
///
/// struct MyType;
///
/// // Using the built-in map functions requires manually looking up `TypeId`s.
/// let mut map = TypeIdIndexMap::default();
/// map.insert(TypeId::of::<MyType>(), 7);
/// assert_eq!(map.get(&TypeId::of::<MyType>()), Some(&7));
///
/// // Using `TypeIdMapExt` functions does the lookup for you.
/// map.insert_type::<MyType>(7);
/// assert_eq!(map.get_type::<MyType>(), Some(&7));
/// ```
pub trait TypeIdMapExt<V> {
    /// Inserts a value for the type `T`.
    ///
    /// If the map did not previously contain this key then [`None`] is returned,
    /// otherwise the value for this key is updated and the old value returned.
    fn insert_type<T: ?Sized + 'static>(&mut self, v: V) -> Option<V>;

    /// Returns a reference to the value for type `T`, if one exists.
    fn get_type<T: ?Sized + 'static>(&self) -> Option<&V>;

    /// Returns a mutable reference to the value for type `T`, if one exists.
    fn get_type_mut<T: ?Sized + 'static>(&mut self) -> Option<&mut V>;

    /// Removes type `T` from the map, returning the value for this
    /// key if it was previously present.
    fn remove_type<T: ?Sized + 'static>(&mut self) -> Option<V>;

    /// Gets the type `T`'s entry in the map for in-place manipulation.
    fn entry_type<T: ?Sized + 'static>(&mut self) -> TypeIdIndexMapEntry<'_, TypeId, V>;
}

impl<V> TypeIdMapExt<V> for TypeIdIndexMap<V> {
    #[inline]
    fn insert_type<T: ?Sized + 'static>(&mut self, v: V) -> Option<V> {
        self.insert(TypeId::of::<T>(), v)
    }

    #[inline]
    fn get_type<T: ?Sized + 'static>(&self) -> Option<&V> {
        self.get(&TypeId::of::<T>())
    }

    #[inline]
    fn get_type_mut<T: ?Sized + 'static>(&mut self) -> Option<&mut V> {
        self.get_mut(&TypeId::of::<T>())
    }

    #[inline]
    fn remove_type<T: ?Sized + 'static>(&mut self) -> Option<V> {
        self.shift_remove(&TypeId::of::<T>())
    }

    #[inline]
    fn entry_type<T: ?Sized + 'static>(&mut self) -> TypeIdIndexMapEntry<'_, TypeId, V> {
        self.entry(TypeId::of::<T>())
    }
}

/// Extension trait to make use of [`TypeIdHashMap`] more ergonomic.
pub trait TypeIdHashMapExt<V> {
    /// Inserts a value for the type `T`.
    ///
    /// If the map did not previously contain this key then [`None`] is returned,
    /// otherwise the value for this key is updated and the old value returned.
    fn insert_type<T: ?Sized + 'static>(&mut self, v: V) -> Option<V>;

    /// Returns a reference to the value for type `T`, if one exists.
    fn get_type<T: ?Sized + 'static>(&self) -> Option<&V>;

    /// Returns a mutable reference to the value for type `T`, if one exists.
    fn get_type_mut<T: ?Sized + 'static>(&mut self) -> Option<&mut V>;

    /// Removes type `T` from the map, returning the value for this
    /// key if it was previously present.
    fn remove_type<T: ?Sized + 'static>(&mut self) -> Option<V>;

    /// Gets the type `T`'s entry in the map for in-place manipulation.
    fn entry_type<T: ?Sized + 'static>(&mut self) -> TypeIdHashMapEntry<'_, TypeId, V, NoOpHash>;
}

impl<V> TypeIdHashMapExt<V> for TypeIdHashMap<V> {
    #[inline]
    fn insert_type<T: ?Sized + 'static>(&mut self, v: V) -> Option<V> {
        self.insert(TypeId::of::<T>(), v)
    }

    #[inline]
    fn get_type<T: ?Sized + 'static>(&self) -> Option<&V> {
        self.get(&TypeId::of::<T>())
    }

    #[inline]
    fn get_type_mut<T: ?Sized + 'static>(&mut self) -> Option<&mut V> {
        self.get_mut(&TypeId::of::<T>())
    }

    #[inline]
    fn remove_type<T: ?Sized + 'static>(&mut self) -> Option<V> {
        self.remove(&TypeId::of::<T>())
    }

    #[inline]
    fn entry_type<T: ?Sized + 'static>(&mut self) -> TypeIdHashMapEntry<'_, TypeId, V, NoOpHash> {
        self.entry(TypeId::of::<T>())
    }
}

/// A map that can store a single value of any type (that implements [`Any`]).
#[derive(Default)]
pub struct AnyMap(TypeIdHashMap<Box<dyn Any>>);

impl AnyMap {
    /// Creates an empty map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns whether this map is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the number of values in this map.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Clears the map, removing all values.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    /// Inserts a value of type `T`.
    ///
    /// If the map did not have this type present, [`None`] is returned.
    ///
    /// If the map did have this type present, the value is updated, and the old value is returned.
    pub fn insert<T: Any>(&mut self, value: T) -> Option<Box<T>> {
        self.0
            .insert(TypeId::of::<T>(), Box::new(value))
            .map(|value| value.downcast().unwrap())
    }

    /// Returns `true` if the map contains a value of type `T`.
    pub fn contains_type<T: Any>(&self) -> bool {
        self.0.contains_key(&TypeId::of::<T>())
    }

    /// Gets the current value of type `T`, if present.
    pub fn get<T: Any>(&self) -> Option<&T> {
        self.0
            .get(&TypeId::of::<T>())
            .map(|value| value.downcast_ref().unwrap())
    }

    /// Gets the current value of type `T` mutably, if present.
    pub fn get_mut<T: Any>(&mut self) -> Option<&mut T> {
        self.0
            .get_mut(&TypeId::of::<T>())
            .map(|value| value.downcast_mut().unwrap())
    }

    /// Removes the value of type `T` from this map, if present.
    pub fn remove<T: Any>(&mut self) -> Option<Box<T>> {
        self.0
            .remove(&TypeId::of::<T>())
            .map(|value| value.downcast().unwrap())
    }

    /// Returns an iterator over all values in this map.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = &dyn Any> {
        self.0.values().map(Deref::deref)
    }

    /// Returns an iterator over all values (mutably) in this map.
    pub fn iter_mut(&mut self) -> impl ExactSizeIterator<Item = &mut dyn Any> {
        self.0.values_mut().map(DerefMut::deref_mut)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use static_assertions::assert_impl_all;

    // Check that the HashMaps are Clone if the key/values are Clone
    assert_impl_all!(PreHashMap::<u64, usize>: Clone);

    #[test]
    fn fast_typeid_hash() {
        struct Hasher;

        impl core::hash::Hasher for Hasher {
            fn finish(&self) -> u64 {
                0
            }
            fn write(&mut self, _: &[u8]) {
                panic!("Hashing of core::any::TypeId changed");
            }
            fn write_u64(&mut self, _: u64) {}
        }

        Hash::hash(&TypeId::of::<()>(), &mut Hasher);
    }

    crate::cfg::alloc! {
        #[test]
        fn stable_hash_within_same_program_execution() {
            use alloc::vec::Vec;

            let mut map_1 = <HashMap<_, _>>::default();
            let mut map_2 = <HashMap<_, _>>::default();
            for i in 1..10 {
                map_1.insert(i, i);
                map_2.insert(i, i);
            }
            assert_eq!(
                map_1.iter().collect::<Vec<_>>(),
                map_2.iter().collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn any_map_gets_value() {
        let mut map = AnyMap::new();
        assert!(map.insert(17u32).is_none());
        assert!(map.insert(10i8).is_none());

        assert_eq!(map.insert(21u32), Some(Box::new(17u32)));

        assert_eq!(map.get::<u32>(), Some(&21u32));
        assert_eq!(map.get::<i8>(), Some(&10i8));

        assert_eq!(map.remove::<i8>(), Some(Box::new(10i8)));
    }
}
