use crate::core::cache::{Cache, CacheError};
use async_trait::async_trait;
use dirs::cache_dir;
use serde::{Serialize, de::DeserializeOwned};
use sled::{Db, Tree};
use std::hash::Hash;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tracing::debug;

pub struct SledCache<K, V>
where
    K: Eq + Hash + Send + Sync + Serialize + DeserializeOwned + 'static,
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    db: Arc<Db>,
    tree: Tree,
}

impl<K, V> SledCache<K, V>
where
    K: Eq + Hash + Send + Sync + Serialize + DeserializeOwned,
    V: Clone + Send + Sync + Serialize + DeserializeOwned,
{
    pub fn new(tree_name: &str) -> Result<Self, CacheError> {
        let cache_path = cache_dir()
            .ok_or_else(|| {
                CacheError::IoError(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "Cache directory not found",
                ))
            })?
            .join("xmf")
            .join("cache");
        std::fs::create_dir_all(&cache_path)?;

        let db = Arc::new(Db::open(cache_path.join("sled_db"))?);
        let tree = db.open_tree(tree_name)?;
        Ok(Self { db, tree })
    }
}

#[async_trait]
impl<K, V> Cache<K, V> for SledCache<K, V>
where
    K: Eq + Hash + Send + Sync + Serialize + DeserializeOwned + 'static,
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    async fn get(&self, key: &K) -> Result<Option<V>, CacheError> {
        if let Ok(Some(value)) = self.tree.get(serde_json::to_vec(key)?) {
            let value: V = serde_json::from_slice(&value)?;
            debug!("Cache HIT for key: {:?}", key);
            return Ok(Some(value));
        }
        debug!("Cache MISS for key: {:?}", key);
        Ok(None)
    }

    async fn put(&self, key: K, value: V, ttl: Option<Duration>) -> Result<(), CacheError> {
        self.tree
            .insert(serde_json::to_vec(&key)?, serde_json::to_vec(&value)?)?;
        debug!("Cache PUT for key: {:?}", key);
        Ok(())
    }

    async fn remove(&self, key: &K) -> Result<(), CacheError> {
        self.tree.remove(serde_json::to_vec(key)?)?;
        debug!("Cache REMOVE for key: {:?}", key);
        Ok(())
    }

    async fn clear(&self) -> Result<(), CacheError> {
        self.tree.clear()?;
        debug!("Cache CLEAR");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_sled_cache_get_put() {
        let cache = SledCache::<String, i32>::new("test_tree").unwrap();

        // Initially, cache is empty
        assert!(cache.get(&"key1".to_string()).await.unwrap().is_none());

        // Put a value without TTL
        cache.put("key1".to_string(), 123, None).await.unwrap();

        // Get the value
        assert_eq!(cache.get(&"key1".to_string()).await.unwrap(), Some(123));

        // Get a non-existent key
        assert!(cache.get(&"key2".to_string()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_sled_cache_ttl_expiration() {
        let cache = SledCache::<String, i32>::new("test_tree_ttl").unwrap();

        // Put value with 10ms TTL
        cache
            .put("key1".to_string(), 123, Some(Duration::from_millis(10)))
            .await
            .unwrap();
        assert_eq!(cache.get(&"key1".to_string()).await.unwrap(), Some(123));

        // Wait for TTL expiration
        sleep(Duration::from_millis(20)).await;
        assert!(cache.get(&"key1".to_string()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_sled_cache_remove() {
        let cache = SledCache::<String, i32>::new("test_tree_remove").unwrap();

        cache.put("key1".to_string(), 123, None).await.unwrap();
        assert_eq!(cache.get(&"key1".to_string()).await.unwrap(), Some(123));

        cache.remove(&"key1".to_string()).await.unwrap();
        assert!(cache.get(&"key1".to_string()).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_sled_cache_clear() {
        let cache = SledCache::<String, i32>::new("test_tree_clear").unwrap();

        cache.put("key1".to_string(), 123, None).await.unwrap();
        cache.put("key2".to_string(), 456, None).await.unwrap();

        cache.clear().await.unwrap();

        assert!(cache.get(&"key1".to_string()).await.unwrap().is_none());
        assert!(cache.get(&"key2".to_string()).await.unwrap().is_none());
    }
}
