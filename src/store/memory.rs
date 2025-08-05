use crate::core::cache::KeyValueCollection;
use async_trait::async_trait;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

struct CacheValue<V> {
    value: V,
    expires_at: Option<Instant>,
}

/// In-memory cache implementation using HashMap and RwLock
pub struct MemoryCollection {
    inner: RwLock<HashMap<Vec<u8>, CacheValue<Vec<u8>>>>,
}

// ---- MemoryCollection implementation ----
impl MemoryCollection {
    /// Creates a new MemoryCache instance
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::new()),
        }
    }
}

impl Default for MemoryCollection {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl KeyValueCollection for MemoryCollection {
    async fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let cache = self.inner.read().await;
        if let Some(entry) = cache.get(key) {
            // Check if entry has expired
            if let Some(expiry) = entry.expires_at {
                if expiry < Instant::now() {
                    return None;
                }
            }
            return Some(entry.value.clone());
        }

        None
    }

    async fn put(&self, key: &[u8], value: &[u8], ttl: Option<Duration>) {
        let expires_at = ttl.map(|duration| Instant::now() + duration);
        let cache_value = CacheValue {
            value: value.into(),
            expires_at,
        };

        let mut cache = self.inner.write().await;
        cache.insert(key.into(), cache_value);
    }

    async fn remove(&self, key: &[u8]) {
        let mut cache = self.inner.write().await;
        cache.remove(key);
    }

    async fn clear(&self) {
        let mut cache = self.inner.write().await;
        cache.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::cache::Store;
    use crate::store::KeyValueStore;
    use tempfile::tempdir;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_cache_get_collection() {
        let dir = tempdir().unwrap();
        let cache = KeyValueStore::with_custom_path(dir.path());

        // Test getting a non-existent collection
        assert!(cache.get_collection("test", false, false).is_none());

        // Test creating and getting a memory-backed collection
        let mem_collection = cache.get_collection("test_mem", false, true).unwrap();
        mem_collection.put(b"mem_key", b"mem_val", None).await;
        assert_eq!(
            mem_collection.get(b"mem_key").await,
            Some(b"mem_val".to_vec())
        );

        // Test creating and getting a disk-backed collection
        let disk_collection = cache.get_collection("test_disk", true, true).unwrap();
        disk_collection.put(b"disk_key", b"disk_val", None).await;
        assert_eq!(
            disk_collection.get(b"disk_key").await,
            Some(b"disk_val".to_vec())
        );

        // Test getting an existing collection
        assert!(cache.get_collection("test_mem", false, false).is_some());
        assert!(cache.get_collection("test_disk", true, false).is_some());
    }

    #[tokio::test]
    async fn test_cache_remove_collection() {
        let dir = tempdir().unwrap();
        let cache = KeyValueStore::with_custom_path(dir.path());

        // Create a collection
        assert!(cache.get_collection("test", true, true).is_some());

        // Remove the collection
        assert!(cache.remove_collection("test"));

        // Verify the collection is removed
        assert!(cache.get_collection("test", false, false).is_none());

        // Try to remove a non-existent collection
        assert!(!cache.remove_collection("nonexistent"));
    }

    #[tokio::test]
    async fn test_collection_get_put() {
        let cache = MemoryCollection::new();

        // Initially, cache is empty
        assert!(cache.get("key1".as_bytes()).await.is_none());

        // Put a value without TTL
        cache
            .put("key1".as_bytes(), &123i32.to_be_bytes(), None)
            .await;

        // Get the value
        assert_eq!(
            cache.get("key1".as_bytes()).await,
            Some(123i32.to_be_bytes().to_vec())
        );

        // Get a non-existent key
        assert!(cache.get("key2".as_bytes()).await.is_none());
    }

    #[tokio::test]
    async fn test_collection_ttl_expiration() {
        let cache = MemoryCollection::new();

        // Put value with 10ms TTL
        cache
            .put(
                "key1".as_bytes(),
                &123u32.to_be_bytes(),
                Some(Duration::from_millis(10)),
            )
            .await;
        assert_eq!(
            cache.get("key1".as_bytes()).await,
            Some(123u32.to_be_bytes().to_vec())
        );

        // Wait for TTL expiration
        sleep(Duration::from_millis(20)).await;
        assert!(cache.get("key1".as_bytes()).await.is_none());
    }

    #[tokio::test]
    async fn test_collection_remove() {
        let cache = MemoryCollection::new();

        cache
            .put("key1".as_bytes(), &123u32.to_be_bytes(), None)
            .await;
        assert_eq!(
            cache.get("key1".as_bytes()).await,
            Some(123u32.to_be_bytes().to_vec())
        );

        cache.remove("key1".as_bytes()).await;
        assert!(cache.get("key1".as_bytes()).await.is_none());
    }

    #[tokio::test]
    async fn test_collection_clear() {
        let cache = MemoryCollection::new();

        cache
            .put("key1".as_bytes(), &123u32.to_be_bytes(), None)
            .await;
        cache
            .put("key2".as_bytes(), &456u32.to_be_bytes(), None)
            .await;

        cache.clear().await;

        assert!(cache.get("key1".as_bytes()).await.is_none());
        assert!(cache.get("key2".as_bytes()).await.is_none());
    }

    #[tokio::test]
    async fn test_disk_collection_persistence() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();

        // Create a store, add a value to a disk collection
        {
            let store = KeyValueStore::with_custom_path(&path);
            let collection = store.get_collection("persist_test", true, true).unwrap();
            collection.put(b"mykey", b"myvalue", None).await;

            // Ensure data is flushed to disk
            store.persist();
        }

        // Create another store instance with the same path
        let store2 = KeyValueStore::with_custom_path(&path);
        let collection2 = store2.get_collection("persist_test", true, true).unwrap();
        let value = collection2.get(b"mykey").await;

        assert_eq!(value, Some(b"myvalue".to_vec()));
    }
}
