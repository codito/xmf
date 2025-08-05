use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

/// Trait representing a cache store with collection management.
pub trait Store {
    /// Retrieves a collection by name, optionally creating it if missing.
    /// Returns `None` if the collection does not exist and `create_if_missing` is false.
    fn get_collection(
        &self,
        name: &str,
        persist: bool,
        create_if_missing: bool,
    ) -> Option<Arc<dyn KeyValueCollection>>;

    /// Removes a collection by name. Returns `true` if the collection was removed.
    fn remove_collection(&self, name: &str) -> bool;
}

/// Trait representing a cache with key-based access and TTL support.
#[async_trait]
pub trait KeyValueCollection: Send + Sync {
    /// Retrieves a value from the cache if present and not expired.
    async fn get(&self, key: &[u8]) -> Option<Vec<u8>>;

    /// Stores a value in cache with specified TTL (None = no expiration).
    async fn put(&self, key: &[u8], value: &[u8], ttl: Option<Duration>);

    /// Removes an entry from the cache.
    async fn remove(&self, key: &[u8]);

    /// Clears all entries from the cache.
    async fn clear(&self);
}
