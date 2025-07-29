use crate::core::cache::Cache;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sled::{Db, Tree};
use std::fmt::Debug;
use std::hash::Hash;
use std::marker::PhantomData;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tracing::debug;

#[derive(Serialize, Deserialize)]
struct CacheEntry<V> {
    value: V,
    expires_at: Option<SystemTime>,
}

pub struct SledCache<K, V>
where
    K: Eq + Hash + Send + Sync + Serialize + DeserializeOwned + 'static + Debug,
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    db: Arc<Db>,
    tree: Tree,
    _marker: PhantomData<(K, V)>,
}

impl<K, V> SledCache<K, V>
where
    K: Eq + Hash + Send + Sync + Serialize + DeserializeOwned + Debug,
    V: Clone + Send + Sync + Serialize + DeserializeOwned,
{
    pub fn new(db_path: &Path, tree_name: &str) -> Result<Self> {
        std::fs::create_dir_all(db_path)?;

        let db = Arc::new(sled::open(db_path.join("sled_db"))?);
        let tree = db.open_tree(tree_name)?;
        Ok(Self {
            db,
            tree,
            _marker: PhantomData,
        })
    }
}

#[async_trait]
impl<K, V> Cache<K, V> for SledCache<K, V>
where
    K: Eq + Hash + Send + Sync + Serialize + DeserializeOwned + 'static + Debug,
    V: Clone + Send + Sync + Serialize + DeserializeOwned + 'static,
{
    async fn get(&self, key: &K) -> Option<V> {
        let res: Result<Option<V>> = (|| {
            if let Some(value) = self.tree.get(serde_json::to_vec(key)?)? {
                let entry: CacheEntry<V> = serde_json::from_slice(&value)?;
                if let Some(expires_at) = entry.expires_at {
                    if SystemTime::now() > expires_at {
                        debug!("Cache entry expired for key: {:?}", key);
                        self.tree.remove(serde_json::to_vec(key)?)?;
                        return Ok(None);
                    }
                }
                debug!("Cache HIT for key: {:?}", key);
                return Ok(Some(entry.value));
            }
            debug!("Cache MISS for key: {:?}", key);
            Ok(None)
        })();

        match res {
            Ok(val) => val,
            Err(e) => {
                debug!("SledCache get error: {}", e);
                None
            }
        }
    }

    async fn put(&self, key: K, value: V, ttl: Option<Duration>) {
        let res: Result<()> = (|| {
            let expires_at = ttl.map(|d| SystemTime::now() + d);
            let entry = CacheEntry { value, expires_at };
            self.tree
                .insert(serde_json::to_vec(&key)?, serde_json::to_vec(&entry)?)?;
            debug!("Cache PUT for key: {:?}", key);
            Ok(())
        })();
        if let Err(e) = res {
            debug!("SledCache put error: {}", e);
        }
    }

    async fn remove(&self, key: &K) {
        let res: Result<()> = (|| {
            Ok(self
                .tree
                .remove(serde_json::to_vec(key)?)?
                .map_or((), |_| ()))
        })();
        if let Err(e) = res {
            debug!("SledCache remove error: {}", e);
        }
    }

    async fn clear(&self) {
        if let Err(e) = self.tree.clear() {
            debug!("SledCache clear error: {}", e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use tokio::time::sleep;

    #[tokio::test]
    async fn test_sled_cache_get_put() {
        let dir = tempdir().unwrap();
        let cache = SledCache::<String, i32>::new(dir.path(), "test_tree").unwrap();

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
    async fn test_sled_cache_ttl_expiration() {
        let dir = tempdir().unwrap();
        let cache = SledCache::<String, i32>::new(dir.path(), "test_tree_ttl").unwrap();

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
    async fn test_sled_cache_remove() {
        let dir = tempdir().unwrap();
        let cache = SledCache::<String, i32>::new(dir.path(), "test_tree_remove").unwrap();

        cache.put("key1".to_string(), 123, None).await;
        assert_eq!(cache.get(&"key1".to_string()).await, Some(123));

        cache.remove(&"key1".to_string()).await;
        assert!(cache.get(&"key1".to_string()).await.is_none());
    }

    #[tokio::test]
    async fn test_sled_cache_clear() {
        let dir = tempdir().unwrap();
        let cache = SledCache::<String, i32>::new(dir.path(), "test_tree_clear").unwrap();

        cache.put("key1".to_string(), 123, None).await;
        cache.put("key2".to_string(), 456, None).await;

        cache.clear().await;

        assert!(cache.get(&"key1".to_string()).await.is_none());
        assert!(cache.get(&"key2".to_string()).await.is_none());
    }
}
