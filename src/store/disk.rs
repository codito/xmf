use crate::core::cache::KeyValueCollection;
use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};
use tracing::debug;

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    value: Vec<u8>,
    expires_at: Option<SystemTime>,
}

/// Backend marker written at the store root. A root that is non-empty but
/// carries no (or a mismatched) marker belongs to a foreign backend (e.g.
/// the previous fjall layout) and is wiped on open: the cache is purely
/// derived data and refetches itself.
const BACKEND_MARKER: &str = ".backend";
const BACKEND_VERSION: &str = "files-v1";

/// Counter to keep temp filenames unique within this process.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Hex-encode a key so it is always a safe single path segment.
fn encode_key(key: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(key.len() * 2);
    for byte in key {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

pub struct DiskStore {
    root: std::path::PathBuf,
}

impl DiskStore {
    pub fn new(path: &std::path::Path) -> Result<Self> {
        let start = Instant::now();
        std::fs::create_dir_all(path)
            .with_context(|| format!("Failed to create cache directory: {}", path.display()))?;

        let marker = path.join(BACKEND_MARKER);
        let foreign = match std::fs::read_to_string(&marker) {
            Ok(version) => version != BACKEND_VERSION,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                path.read_dir()?.next().is_some_and(|r| r.is_ok())
            }
            Err(e) => return Err(e.into()),
        };
        if foreign {
            debug!("Cache directory holds a foreign backend, starting fresh");
            std::fs::remove_dir_all(path)?;
            std::fs::create_dir_all(path)?;
        }
        if std::fs::read_to_string(&marker).is_err() {
            std::fs::write(&marker, BACKEND_VERSION)?;
        }

        debug!(
            "Opened file cache at {} in {:?}",
            path.display(),
            start.elapsed()
        );
        Ok(Self {
            root: path.to_path_buf(),
        })
    }

    pub fn get_collection(&self, name: &str) -> Result<DiskCollection> {
        anyhow::ensure!(
            !name.is_empty() && !name.contains('/') && !name.contains(".."),
            "Invalid collection name: {name}"
        );
        let dir = self.root.join(name);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create collection: {name}"))?;
        Ok(DiskCollection { dir })
    }

    pub fn persist(&self) -> Result<()> {
        // No-op: every put is atomically renamed into place, so there is
        // nothing to flush. Kept so the backend stays swappable.
        Ok(())
    }

    pub fn clear(&self) -> Result<()> {
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                std::fs::remove_dir_all(&path)?;
            }
        }
        Ok(())
    }
}

pub struct DiskCollection {
    dir: std::path::PathBuf,
}

impl DiskCollection {
    fn path_for(&self, key: &[u8]) -> std::path::PathBuf {
        self.dir.join(encode_key(key))
    }

    fn read_entry(&self, key: &[u8]) -> Result<Option<CacheEntry>> {
        match std::fs::read(self.path_for(key)) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[async_trait]
impl KeyValueCollection for DiskCollection {
    async fn get(&self, key: &[u8]) -> Option<Vec<u8>> {
        let res: Result<Option<Vec<u8>>> = (|| {
            if let Some(entry) = self.read_entry(key)? {
                if let Some(expires_at) = entry.expires_at
                    && SystemTime::now() > expires_at
                {
                    debug!(
                        "Cache entry expired for key: {:?}",
                        String::from_utf8_lossy(key)
                    );
                    let _ = std::fs::remove_file(self.path_for(key));
                    return Ok(None);
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
            let bytes = serde_json::to_vec(&entry)?;
            let target = self.path_for(key);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let tmp = self.dir.join(format!(
                "tmp-{}-{}.part",
                std::process::id(),
                TMP_COUNTER.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(&tmp, &target)?;
            debug!("Cache PUT for key: {:?}", String::from_utf8_lossy(key));
            Ok(())
        })();
        if let Err(e) = res {
            debug!("DiskCollection put error: {}", e);
        }
    }

    async fn remove(&self, key: &[u8]) {
        match std::fs::remove_file(self.path_for(key)) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => debug!("DiskCollection remove error: {}", e),
        }
    }

    async fn clear(&self) {
        let res: Result<()> = (|| {
            for entry in std::fs::read_dir(&self.dir)? {
                let path = entry?.path();
                if path.is_file() {
                    std::fs::remove_file(&path)?;
                }
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
    use tempfile::{TempDir, tempdir};
    use tokio::time::sleep;

    fn create_test_collection() -> (DiskCollection, TempDir) {
        let dir = tempdir().unwrap();
        let store = DiskStore::new(dir.path()).unwrap();
        (store.get_collection("test").unwrap(), dir)
    }

    fn dir_size(path: &std::path::Path) -> u64 {
        let mut total = 0;
        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    total += meta.len();
                }
            }
        }
        total
    }

    #[tokio::test]
    async fn test_disk_cache_get_put() {
        let (cache, _dir) = create_test_collection();

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
        let (cache, _dir) = create_test_collection();

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
        let (cache, _dir) = create_test_collection();

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
        let (cache, _dir) = create_test_collection();

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

    #[tokio::test]
    async fn test_disk_cache_corrupt_file_is_miss() {
        let (cache, dir) = create_test_collection();

        cache.put(b"key1", b"value1", None).await;
        std::fs::write(dir.path().join("test").join(encode_key(b"key1")), b"{{{").unwrap();

        assert!(cache.get(b"key1").await.is_none());
    }

    #[tokio::test]
    async fn test_disk_cache_concurrent_puts_last_writer_wins() {
        let (cache, _dir) = create_test_collection();
        let cache = std::sync::Arc::new(cache);

        let mut handles = Vec::new();
        for i in 0..20u8 {
            let cache = cache.clone();
            handles.push(tokio::spawn(async move {
                cache.put(b"shared", &[i], None).await;
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }

        // Exactly one value survives, and it is intact (no torn writes).
        let got = cache.get(b"shared").await.unwrap();
        assert_eq!(got.len(), 1);
        assert!(got[0] < 20);
    }

    #[tokio::test]
    async fn test_disk_store_persist() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();

        // Create store, add data, and persist
        {
            let store = DiskStore::new(&path).unwrap();
            let collection = store.get_collection("test").unwrap();
            collection.put(b"key1", b"value1", None).await;
            store.persist().unwrap();
        }

        // Re-open store and check if data is still there
        {
            let store = DiskStore::new(&path).unwrap();
            let collection = store.get_collection("test").unwrap();
            assert_eq!(collection.get(b"key1").await, Some(b"value1".to_vec()));
        }
    }

    #[tokio::test]
    async fn test_disk_store_clear() {
        let dir = tempdir().unwrap();
        let store = DiskStore::new(dir.path()).unwrap();

        // Create a few collections and add data
        let collection1 = store.get_collection("test1").unwrap();
        collection1.put(b"key1", b"value1", None).await;

        let collection2 = store.get_collection("test2").unwrap();
        collection2.put(b"key2", b"value2", None).await;

        store.clear().unwrap();
        assert!(collection1.get(b"key1").await.is_none());
        assert!(collection2.get(b"key2").await.is_none());

        // Collections are usable again after a clear.
        collection1.put(b"key1", b"value1", None).await;
        assert_eq!(collection1.get(b"key1").await, Some(b"value1".to_vec()));
    }

    #[tokio::test]
    async fn test_rewrites_do_not_grow_storage() {
        let dir = tempdir().unwrap();
        let store = DiskStore::new(dir.path()).unwrap();
        let collection = store.get_collection("prices").unwrap();

        // Simulate several runs' worth of same-key overwrites.
        let value_a = vec![b'a'; 2048];
        let value_b = vec![b'b'; 2048];
        for i in 0..100 {
            collection
                .put(format!("key{i}").as_bytes(), &value_a, None)
                .await;
        }
        let after_first = dir_size(&dir.path().join("prices"));

        for i in 0..100 {
            collection
                .put(format!("key{i}").as_bytes(), &value_b, None)
                .await;
        }
        let after_rewrite = dir_size(&dir.path().join("prices"));

        assert_eq!(
            after_rewrite, after_first,
            "rewriting the same keys grew storage from {after_first} to {after_rewrite}"
        );
    }

    #[tokio::test]
    async fn test_foreign_backend_dir_starts_fresh() {
        let dir = tempdir().unwrap();

        // Simulate the previous fjall layout: no marker, foreign files.
        std::fs::write(dir.path().join("0.jnl"), b"journal-bytes").unwrap();
        std::fs::create_dir(dir.path().join("keyspaces")).unwrap();

        let store = DiskStore::new(dir.path()).unwrap();
        assert!(!dir.path().join("0.jnl").exists());
        assert!(!dir.path().join("keyspaces").exists());
        assert_eq!(
            std::fs::read_to_string(dir.path().join(BACKEND_MARKER)).unwrap(),
            BACKEND_VERSION
        );

        let collection = store.get_collection("test").unwrap();
        collection.put(b"key1", b"value1", None).await;
        assert_eq!(collection.get(b"key1").await, Some(b"value1".to_vec()));
    }
}
