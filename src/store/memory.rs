use crate::core::cache::Cache;
use async_trait::async_trait;
use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::debug;

struct CacheValue<V> {
    value: V,
    expires_at: Option<Instant>,
}

/// In-memory cache implementation using HashMap and RwLock
pub struct MemoryCache<K, V>
where
    K: Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    inner: Arc<Mutex<HashMap<K, CacheValue<V>>>>,
}

impl<K, V> MemoryCache<K, V>
where
    K: Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Creates a new MemoryCache instance
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl<K, V> Default for MemoryCache<K, V>
where
    K: Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    /// Creates a new MemoryCache instance with default settings
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl<K, V> Cache<K, V> for MemoryCache<K, V>
where
    K: Eq + Hash + Send + Sync + std::fmt::Debug + 'static,
    V: Clone + Send + Sync + 'static,
{
    async fn get(&self, key: &K) -> Option<V> {
        let cache = self.inner.lock().await;
        if let Some(entry) = cache.get(key) {
            // Check if entry has expired
            if let Some(expiry) = entry.expires_at {
                if expiry < Instant::now() {
                    debug!("Cache entry expired for key: {:?}", key);
                    return None;
                }
            }
            debug!("Cache HIT for key: {:?}", key);
            return Some(entry.value.clone());
        }
        debug!("Cache MISS for key: {:?}", key);
        None
    }

    async fn put(&self, key: K, value: V, ttl: Option<Duration>) {
        let expires_at = ttl.map(|duration| Instant::now() + duration);
        let cache_value = CacheValue { value, expires_at };

        let mut cache = self.inner.lock().await;
        debug!("Cache PUT for key: {:?}", key);
        cache.insert(key, cache_value);
    }

    async fn remove(&self, key: &K) {
        let mut cache = self.inner.lock().await;
        cache.remove(key);
        debug!("Cache REMOVE for key: {:?}", key);
    }

    async fn clear(&self) {
        let mut cache = self.inner.lock().await;
        cache.clear();
        debug!("Cache CLEAR");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_cache_get_put() {
        let cache = MemoryCache::<String, i32>::new();

        // Initially, cache is empty
        assert!(cache.get(&"key1".to_string()).await.is_none());

        // Put a value without TTL
        cache.put("key1".to_string(), 123, None).await;

        // Get the value
        assert_eq!(cache.get(&"key1".to_string()).await, Some(123));

        // Get a non-existent key
        assert!(cache.get(&"key2".to_string()).await.is_none());
    }

    #[tokio::test]
    async fn test_cache_ttl_expiration() {
        let cache = MemoryCache::<String, i32>::new();

        // Put value with 10ms TTL
        cache
            .put("key1".to_string(), 123, Some(Duration::from_millis(10)))
            .await;
        assert_eq!(cache.get(&"key1".to_string()).await, Some(123));

        // Wait for TTL expiration
        sleep(Duration::from_millis(20)).await;
        assert!(cache.get(&"key1".to_string()).await.is_none());
    }

    #[tokio::test]
    async fn test_cache_remove() {
        let cache = MemoryCache::<String, i32>::new();

        cache.put("key1".to_string(), 123, None).await;
        assert_eq!(cache.get(&"key1".to_string()).await, Some(123));

        cache.remove(&"key1".to_string()).await;
        assert!(cache.get(&"key1".to_string()).await.is_none());
    }

    #[tokio::test]
    async fn test_cache_clear() {
        let cache = MemoryCache::<String, i32>::new();

        cache.put("key1".to_string(), 123, None).await;
        cache.put("key2".to_string(), 456, None).await;

        cache.clear().await;

        assert!(cache.get(&"key1".to_string()).await.is_none());
        assert!(cache.get(&"key2".to_string()).await.is_none());
    }
}
