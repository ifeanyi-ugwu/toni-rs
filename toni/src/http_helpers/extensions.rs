//! Type-safe storage for request-scoped data.
//!
//! The `Extensions` type provides a way for middleware to pass typed data
//! to controllers and services without coupling them to HTTP implementation details.
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

/// A type map for storing request-scoped data.
///
/// This allows middleware to pass typed data to controllers and services
/// without coupling them to HTTP types.
#[derive(Debug, Default)]
pub struct Extensions {
    map: HashMap<TypeId, Box<dyn Any + Send + Sync>>,
}

// Manual Clone implementation that creates empty Extensions
// (trait objects can't be cloned generically)
impl Clone for Extensions {
    fn clone(&self) -> Self {
        // Create empty extensions on clone
        // This is intentional: we don't want to clone the internal state
        Self::new()
    }
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
    /// let old = ext.insert(UserId("alice".to_string()));
    /// assert!(old.is_none());
    ///
    /// let old = ext.insert(UserId("bob".to_string()));
    /// assert_eq!(old.unwrap().0, "alice");
    /// ```
    pub fn insert<T: Send + Sync + 'static>(&mut self, val: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), Box::new(val))
            .and_then(|boxed| boxed.downcast().ok())
            .map(|boxed| *boxed)
    }

    /// Get a reference to a value.
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
    /// let user_id = ext.get::<UserId>().unwrap();
    /// assert_eq!(user_id.0, "alice");
    /// ```
    pub fn get<T: 'static>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_ref())
    }

    /// Get a mutable reference to a value.
    ///
    /// # Examples
    ///
    /// ```
    /// use toni::http_helpers::Extensions;
    ///
    /// #[derive(Clone)]
    /// struct Counter(usize);
    ///
    /// let mut ext = Extensions::new();
    /// ext.insert(Counter(0));
    ///
    /// if let Some(counter) = ext.get_mut::<Counter>() {
    ///     counter.0 += 1;
    /// }
    ///
    /// assert_eq!(ext.get::<Counter>().unwrap().0, 1);
    /// ```
    pub fn get_mut<T: 'static>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast_mut())
    }

    /// Remove a value from the map.
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
    /// let removed = ext.remove::<UserId>().unwrap();
    /// assert_eq!(removed.0, "alice");
    ///
    /// assert!(ext.get::<UserId>().is_none());
    /// ```
    pub fn remove<T: 'static>(&mut self) -> Option<T> {
        self.map
            .remove(&TypeId::of::<T>())
            .and_then(|boxed| boxed.downcast().ok())
            .map(|boxed| *boxed)
    }

    /// Clear all values from the map.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Check if the map is empty.
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Get the number of values stored in the map.
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct UserId(String);

    #[derive(Clone, Debug, PartialEq)]
    struct RequestId(String);

    #[test]
    fn test_insert_and_get() {
        let mut ext = Extensions::new();

        ext.insert(UserId("alice".to_string()));
        ext.insert(RequestId("req-123".to_string()));

        assert_eq!(ext.get::<UserId>().unwrap().0, "alice");
        assert_eq!(ext.get::<RequestId>().unwrap().0, "req-123");
    }

    #[test]
    fn test_insert_overwrites() {
        let mut ext = Extensions::new();

        let old = ext.insert(UserId("alice".to_string()));
        assert!(old.is_none());

        let old = ext.insert(UserId("bob".to_string()));
        assert_eq!(old.unwrap().0, "alice");

        assert_eq!(ext.get::<UserId>().unwrap().0, "bob");
    }

    #[test]
    fn test_get_mut() {
        let mut ext = Extensions::new();
        ext.insert(UserId("alice".to_string()));

        if let Some(user_id) = ext.get_mut::<UserId>() {
            user_id.0 = "bob".to_string();
        }

        assert_eq!(ext.get::<UserId>().unwrap().0, "bob");
    }

    #[test]
    fn test_remove() {
        let mut ext = Extensions::new();
        ext.insert(UserId("alice".to_string()));

        let removed = ext.remove::<UserId>().unwrap();
        assert_eq!(removed.0, "alice");

        assert!(ext.get::<UserId>().is_none());
    }

    #[test]
    fn test_len_and_is_empty() {
        let mut ext = Extensions::new();

        assert!(ext.is_empty());
        assert_eq!(ext.len(), 0);

        ext.insert(UserId("alice".to_string()));
        assert!(!ext.is_empty());
        assert_eq!(ext.len(), 1);

        ext.insert(RequestId("req-123".to_string()));
        assert_eq!(ext.len(), 2);

        ext.clear();
        assert!(ext.is_empty());
        assert_eq!(ext.len(), 0);
    }

    #[test]
    fn test_multiple_types() {
        let mut ext = Extensions::new();

        #[derive(Clone, Debug)]
        struct Name(String);

        #[derive(Clone, Debug)]
        struct Age(u32);

        #[derive(Clone, Debug)]
        struct Email(String);

        ext.insert(Name("Alice".to_string()));
        ext.insert(Age(30));
        ext.insert(Email("alice@example.com".to_string()));

        assert_eq!(ext.get::<Name>().unwrap().0, "Alice");
        assert_eq!(ext.get::<Age>().unwrap().0, 30);
        assert_eq!(ext.get::<Email>().unwrap().0, "alice@example.com");
    }
}
