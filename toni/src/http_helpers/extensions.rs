//! Type-safe storage for request-scoped data.
//!
//! The `Extensions` type provides a way for middleware to pass typed data
//! to controllers and services without coupling them to HTTP implementation details.
//!
//! # Implementation Note
//!
//! This implementation is based on the `http` crate's Extensions type
//! (https://docs.rs/http/1.3.1/http/struct.Extensions.html).
//!
//! # Examples
//!
//! ```
//! use toni::http_helpers::Extensions;
//!
//! #[derive(Clone)]
//! struct UserId(String);
//!
//! let mut ext = Extensions::new();
//! ext.insert(UserId("alice".to_string()));
//!
//! let user_id = ext.get::<UserId>().unwrap();
//! assert_eq!(user_id.0, "alice");
//! ```

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::hash::{BuildHasherDefault, Hasher};

type AnyMap = HashMap<TypeId, Box<dyn AnyClone + Send + Sync>, BuildHasherDefault<IdHasher>>;

// With TypeIds as keys, there's no need to hash them. They are already hashes
// themselves, coming from the compiler. The IdHasher just holds the u64 of
// the TypeId, and then returns it, instead of doing any bit fiddling.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("TypeId calls write_u64");
    }

    #[inline]
    fn write_u64(&mut self, id: u64) {
        self.0 = id;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// A type map for storing request-scoped data.
///
/// This allows middleware to pass typed data to controllers and services
/// without coupling them to HTTP types.
///
/// Values stored in `Extensions` must implement `Clone + Send + Sync + 'static`.
#[derive(Clone, Default)]
pub struct Extensions {
    map: AnyMap,
}

impl Extensions {
    /// Create an empty `Extensions` map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert a value into the map.
    ///
    /// If a value of this type already existed, it will be returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use toni::http_helpers::Extensions;
    ///
    /// #[derive(Clone)]
    /// struct UserId(String);
    ///
    /// let mut ext = Extensions::new();
    /// assert!(ext.insert(UserId("alice".to_string())).is_none());
    /// assert_eq!(
    ///     ext.insert(UserId("bob".to_string())).unwrap().0,
    ///     "alice"
    /// );
    /// ```
    pub fn insert<T: Clone + Send + Sync + 'static>(&mut self, val: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), Box::new(val))
            .and_then(|boxed| boxed.into_any().downcast().ok().map(|boxed| *boxed))
    }

    /// Get a reference to a value previously inserted.
    ///
    /// # Examples
    ///
    /// ```
    /// use toni::http_helpers::Extensions;
    ///
    /// #[derive(Clone)]
    /// struct UserId(String);
    ///
    /// let mut ext = Extensions::new();
    /// assert!(ext.get::<UserId>().is_none());
    ///
    /// ext.insert(UserId("alice".to_string()));
    /// assert_eq!(ext.get::<UserId>().unwrap().0, "alice");
    /// ```
    pub fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|boxed| (**boxed).as_any().downcast_ref())
    }

    /// Get a mutable reference to a value previously inserted.
    ///
    /// # Examples
    ///
    /// ```
    /// use toni::http_helpers::Extensions;
    ///
    /// #[derive(Clone)]
    /// struct Counter(i32);
    ///
    /// let mut ext = Extensions::new();
    /// ext.insert(Counter(5));
    ///
    /// ext.get_mut::<Counter>().unwrap().0 += 10;
    /// assert_eq!(ext.get::<Counter>().unwrap().0, 15);
    /// ```
    pub fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| (**boxed).as_any_mut().downcast_mut())
    }

    /// Remove a value from the map.
    ///
    /// If a value of this type existed, it will be returned.
    ///
    /// # Examples
    ///
    /// ```
    /// use toni::http_helpers::Extensions;
    ///
    /// #[derive(Clone)]
    /// struct UserId(String);
    ///
    /// let mut ext = Extensions::new();
    /// ext.insert(UserId("alice".to_string()));
    ///
    /// assert_eq!(ext.remove::<UserId>().unwrap().0, "alice");
    /// assert!(ext.get::<UserId>().is_none());
    /// ```
    pub fn remove<T: Send + Sync + 'static>(&mut self) -> Option<T> {
        self.map
            .remove(&TypeId::of::<T>())
            .and_then(|boxed| boxed.into_any().downcast().ok().map(|boxed| *boxed))
    }

    /// Clear all values from the map.
    ///
    /// # Examples
    ///
    /// ```
    /// use toni::http_helpers::Extensions;
    ///
    /// #[derive(Clone)]
    /// struct UserId(String);
    ///
    /// let mut ext = Extensions::new();
    /// ext.insert(UserId("alice".to_string()));
    /// ext.clear();
    ///
    /// assert!(ext.get::<UserId>().is_none());
    /// ```
    #[inline]
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Check if the map is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use toni::http_helpers::Extensions;
    ///
    /// #[derive(Clone)]
    /// struct UserId(String);
    ///
    /// let mut ext = Extensions::new();
    /// assert!(ext.is_empty());
    ///
    /// ext.insert(UserId("alice".to_string()));
    /// assert!(!ext.is_empty());
    /// ```
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Get the number of values in the map.
    ///
    /// # Examples
    ///
    /// ```
    /// use toni::http_helpers::Extensions;
    ///
    /// #[derive(Clone)]
    /// struct UserId(String);
    ///
    /// let mut ext = Extensions::new();
    /// assert_eq!(ext.len(), 0);
    ///
    /// ext.insert(UserId("alice".to_string()));
    /// assert_eq!(ext.len(), 1);
    /// ```
    #[inline]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

impl fmt::Debug for Extensions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Extensions").finish()
    }
}

// Internal trait to enable cloning of trait objects
trait AnyClone: Any {
    fn clone_box(&self) -> Box<dyn AnyClone + Send + Sync>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: Clone + Send + Sync + 'static> AnyClone for T {
    fn clone_box(&self) -> Box<dyn AnyClone + Send + Sync> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl Clone for Box<dyn AnyClone + Send + Sync> {
    fn clone(&self) -> Self {
        (**self).clone_box()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct MyType(i32);

    #[test]
    fn test_extensions() {
        let mut extensions = Extensions::new();

        extensions.insert(5i32);
        extensions.insert(MyType(10));

        assert_eq!(extensions.get(), Some(&5i32));
        assert_eq!(extensions.get_mut(), Some(&mut 5i32));

        // Clone now properly preserves data!
        let ext2 = extensions.clone();

        assert_eq!(extensions.remove::<i32>(), Some(5i32));
        assert!(extensions.get::<i32>().is_none());

        // Clone still has it
        assert_eq!(ext2.get(), Some(&5i32));
        assert_eq!(ext2.get(), Some(&MyType(10)));

        assert_eq!(extensions.get::<bool>(), None);
        assert_eq!(extensions.get(), Some(&MyType(10)));
    }

    #[test]
    fn test_clear() {
        let mut ext = Extensions::new();
        ext.insert(5i32);
        ext.insert("hello");

        assert_eq!(ext.len(), 2);
        ext.clear();
        assert_eq!(ext.len(), 0);
        assert!(ext.is_empty());
    }

    #[test]
    fn test_remove() {
        let mut ext = Extensions::new();
        ext.insert(5i32);

        assert_eq!(ext.remove::<i32>(), Some(5));
        assert_eq!(ext.remove::<i32>(), None);
    }
}
