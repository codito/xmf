use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::debug;

#[derive(Clone)]
pub struct Cache<K, V>
where
    K: Eq + Hash + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    inner: Arc<Mutex<HashMap<K, V>>>,
}

impl<K, V> Cache<K, V>
where
    K: Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub async fn get(&self, key: &K) -> Option<V> {
        let cache = self.inner.lock().await;
        let value = cache.get(key).cloned();
        if value.is_some() {
            debug!("Cache HIT");
        } else {
            debug!("Cache MISS");
        }
        value
    }

    pub async fn put(&self, key: K, value: V) {
        let mut cache = self.inner.lock().await;
        debug!("Cache PUT");
        cache.insert(key, value);
    }
}

impl<K, V> Default for Cache<K, V>
where
    K: Eq + Hash + Send + Sync,
    V: Clone + Send + Sync,
{
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_get_put() {
        let cache = Cache::<String, i32>::new();

        // Initially, cache is empty
        assert!(cache.get(&"key1".to_string()).await.is_none());

        // Put a value
        cache.put("key1".to_string(), 123).await;

        // Get the value
        assert_eq!(cache.get(&"key1".to_string()).await, Some(123));

        // Get a non-existent key
        assert!(cache.get(&"key2".to_string()).await.is_none());
    }
}
