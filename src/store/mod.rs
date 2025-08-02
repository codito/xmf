// pub mod disk;
pub mod memory;

use std::{
    any::Any,
    collections::HashMap,
    sync::{Arc, RwLock},
};

// pub use disk::{DiskCache, DiskCollection};
pub use memory::MemoryCollection;

use crate::core::cache::{KeyValueCollection, Store};

/// A thread-safe key-value store that can hold multiple collections.
pub struct KeyValueStore {
    collections: RwLock<HashMap<String, Arc<dyn Any + Send + Sync>>>,
}

impl KeyValueStore {
    pub fn new() -> Self {
        Self {
            collections: RwLock::new(HashMap::new()),
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
            collections
                .entry(name.into())
                .or_insert_with(|| Arc::new(MemoryCollection::new()) as Arc<dyn Any + Send + Sync>);
        }

        let collections = self.collections.read().unwrap();
        collections
            .get(name)
            .cloned()
            .map(|collection| -> Arc<dyn KeyValueCollection> {
                if persist {
                    // collection.downcast::<DiskCollection>().unwrap()
                    collection.downcast::<MemoryCollection>().unwrap()
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
