pub mod disk;
pub mod memory;

use crate::core::cache::{KeyValueCollection, Store};
use crate::core::config::AppConfig;
use disk::DiskCollection;
use fjall::{Keyspace, PartitionCreateOptions};
use memory::MemoryCollection;
use std::{
    any::Any,
    collections::HashMap,
    sync::{Arc, RwLock},
};

/// A thread-safe key-value store that can hold multiple collections.
pub struct KeyValueStore {
    collections: RwLock<HashMap<String, Arc<dyn Any + Send + Sync>>>,
    keyspace: Option<Arc<Keyspace>>,
}

impl KeyValueStore {
    #[cfg(test)]
    pub(crate) fn new_for_test(path: &std::path::Path) -> Self {
        let keyspace = fjall::Config::new(path).open().ok().map(Arc::new);

        Self {
            collections: RwLock::new(HashMap::new()),
            keyspace,
        }
    }

    pub fn new() -> Self {
        let keyspace = AppConfig::default_data_path()
            .ok()
            .and_then(|path| {
                let cache_dir = path.join("cache");
                fjall::Config::new(cache_dir).open().ok()
            })
            .map(Arc::new);

        Self {
            collections: RwLock::new(HashMap::new()),
            keyspace,
        }
    }
}

impl Default for KeyValueStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Store for KeyValueStore {
    fn get_collection(
        &self,
        name: &str,
        persist: bool,
        create_if_missing: bool,
    ) -> Option<Arc<dyn KeyValueCollection>> {
        if create_if_missing {
            let mut collections = self.collections.write().unwrap();
            if !collections.contains_key(name) {
                let new_collection: Option<Arc<dyn Any + Send + Sync>> = if persist {
                    self.keyspace.as_ref().and_then(|ks| {
                        ks.open_partition(name, PartitionCreateOptions::default())
                            .ok()
                            .map(|partition| {
                                Arc::new(DiskCollection::new(partition))
                                    as Arc<dyn Any + Send + Sync>
                            })
                    })
                } else {
                    Some(Arc::new(MemoryCollection::new()))
                };

                if let Some(collection) = new_collection {
                    collections.insert(name.to_string(), collection);
                } else if persist {
                    return None; // Failed to create persistent collection
                }
            }
        }

        let collections = self.collections.read().unwrap();
        collections
            .get(name)
            .cloned()
            .map(|collection| -> Arc<dyn KeyValueCollection> {
                if persist {
                    collection.downcast::<DiskCollection>().unwrap()
                } else {
                    collection.downcast::<MemoryCollection>().unwrap()
                }
            })
    }

    fn remove_collection(&self, name: &str) -> bool {
        let mut collections = self.collections.write().unwrap();
        collections.remove(name).is_some()
    }
}
