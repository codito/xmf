pub mod disk;
pub mod memory;

use crate::core::cache::{KeyValueCollection, Store};
use anyhow::Result;
use disk::{DiskCollection, DiskStore};
use memory::MemoryCollection;
use std::{
    any::Any,
    collections::HashMap,
    sync::{Arc, RwLock},
};

/// A thread-safe key-value store that can hold multiple collections.
pub struct KeyValueStore {
    collections: RwLock<HashMap<String, Arc<dyn Any + Send + Sync>>>,
    disk_store: Option<DiskStore>,
}

impl KeyValueStore {
    pub fn with_custom_path(path: &std::path::Path) -> Self {
        Self {
            collections: RwLock::new(HashMap::new()),
            disk_store: DiskStore::new(path).ok(),
        }
    }

    #[cfg(test)]
    pub(crate) fn persist(&self) {
        if let Some(ds) = &self.disk_store {
            ds.persist().unwrap();
        }
    }

    pub fn new() -> Self {
        // We'll need access to config to get proper data path - let main handle this conditionally
        Self {
            collections: RwLock::new(HashMap::new()),
            disk_store: None,
        }
    }

    pub fn clear_persistent_cache(&self) -> Result<()> {
        if let Some(ds) = &self.disk_store {
            ds.clear()?;
            let mut collections = self.collections.write().unwrap();
            collections
                .retain(|_, collection| collection.downcast_ref::<DiskCollection>().is_none());
        }
        Ok(())
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
                    self.disk_store
                        .as_ref()
                        .and_then(|ds| ds.get_collection(name).ok())
                        .map(|collection| Arc::new(collection) as Arc<dyn Any + Send + Sync>)
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
