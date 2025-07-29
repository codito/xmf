use async_trait::async_trait;
use std::hash::Hash;
use std::time::Duration;

/// Trait representing a cache with key-based access and TTL support
#[async_trait]
pub trait Cache<K, V>: Send + Sync
where
    K: Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    /// Retrieves a value from the cache if present and not expired
    async fn get(&self, key: &K) -> Option<V>;

    /// Stores a value in cache with specified TTL (None = no expiration)
    async fn put(&self, key: K, value: V, ttl: Option<Duration>);

    /// Removes an entry from the cache
    async fn remove(&self, key: &K);

    /// Clears all entries from the cache
    async fn clear(&self);
}
