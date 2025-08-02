use crate::core::cache::KeyValueCollection;
use anyhow::Result;
use async_trait::async_trait;
use fjall::PartitionHandle;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime};
use tracing::debug;

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    value: Vec<u8>,
    expires_at: Option<SystemTime>,
}

pub struct DiskCollection {
    partition: PartitionHandle,
}

impl DiskCollection {
    pub fn new(partition: PartitionHandle) -> Self {
        Self { partition }
    }
}

#[async_trait]
impl KeyValueCollection for DiskCollection {
    async fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let res: Result<Option<Vec<u8>>> = (|| {
            if let Some(value) = self.partition.get(key)? {
                let entry: CacheEntry = serde_json::from_slice(&value)?;
                if let Some(expires_at) = entry.expires_at {
                    if SystemTime::now() > expires_at {
                        debug!(
                            "Cache entry expired for key: {:?}",
                            String::from_utf8_lossy(key)
                        );
                        self.partition.remove(key)?;
                        return Ok(None);
                    }
                }
                debug!("Cache HIT for key: {:?}", String::from_utf8_lossy(key));
                return Ok(Some(entry.value));
            }
            debug!("Cache MISS for key: {:?}", String::from_utf8_lossy(key));
            Ok(None)
        })();

        match res {
            Ok(val) => val,
            Err(e) => {
                debug!("DiskCollection get error: {}", e);
                None
            }
        }
    }

    async fn put(&self, key: &[u8], value: &[u8], ttl: Option<Duration>) {
        let res: Result<()> = (|| {
            let expires_at = ttl.map(|d| SystemTime::now() + d);
            let entry = CacheEntry {
                value: value.to_vec(),
                expires_at,
            };
            self.partition.insert(key, serde_json::to_vec(&entry)?)?;
            debug!("Cache PUT for key: {:?}", String::from_utf8_lossy(key));
            Ok(())
        })();
        if let Err(e) = res {
            debug!("DiskCollection put error: {}", e);
        }
    }

    async fn remove(&self, key: &[u8]) {
        if let Err(e) = self.partition.remove(key) {
            debug!("DiskCollection remove error: {}", e);
        }
    }

    async fn clear(&self) {
        let res: Result<()> = (|| {
            let keys: Vec<_> = self
                .partition
                .iter()
                .keys()
                .collect::<std::result::Result<_, _>>()?;
            for key in keys {
                self.partition.remove(key)?;
            }
            Ok(())
        })();

        if let Err(e) = res {
            debug!("DiskCollection clear error: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fjall::{Config, Keyspace, PartitionCreateOptions};
    use tempfile::tempdir;
    use tokio::time::sleep;

    fn create_test_collection() -> (DiskCollection, Arc<Keyspace>) {
        let dir = tempdir().unwrap();
        let keyspace = Arc::new(Config::new(dir.path()).open().unwrap());
        let partition = keyspace
            .open_partition("test", PartitionCreateOptions::default())
            .unwrap();
        (DiskCollection::new(partition), keyspace)
    }

    #[tokio::test]
    async fn test_disk_cache_get_put() {
        let (cache, _keyspace) = create_test_collection();

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
    async fn test_disk_cache_ttl_expiration() {
        let (cache, _keyspace) = create_test_collection();

        // Put value with 10ms TTL
        cache
            .put(
                "key1".as_bytes(),
                &123i32.to_be_bytes(),
                Some(Duration::from_millis(10)),
            )
            .await;
        assert_eq!(
            cache.get("key1".as_bytes()).await,
            Some(123i32.to_be_bytes().to_vec())
        );

        // Wait for TTL expiration
        sleep(Duration::from_millis(20)).await;
        assert!(cache.get("key1".as_bytes()).await.is_none());
    }

    #[tokio::test]
    async fn test_disk_cache_remove() {
        let (cache, _keyspace) = create_test_collection();

        cache
            .put("key1".as_bytes(), &123i32.to_be_bytes(), None)
            .await;
        assert_eq!(
            cache.get("key1".as_bytes()).await,
            Some(123i32.to_be_bytes().to_vec())
        );

        cache.remove("key1".as_bytes()).await;
        assert!(cache.get("key1".as_bytes()).await.is_none());
    }

    #[tokio::test]
    async fn test_disk_cache_clear() {
        let (cache, _keyspace) = create_test_collection();

        cache
            .put("key1".as_bytes(), &123i32.to_be_bytes(), None)
            .await;
        cache
            .put("key2".as_bytes(), &456i32.to_be_bytes(), None)
            .await;

        cache.clear().await;

        assert!(cache.get("key1".as_bytes()).await.is_none());
        assert!(cache.get("key2".as_bytes()).await.is_none());
    }
}
