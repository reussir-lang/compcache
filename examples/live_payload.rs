//! A client-scoped live object cache. Run with a private cache directory argument.
use compcache::{
    Error, InMemoryCache, Key, MemoryConfig, MemoryEntry, NoExtraData, Produced, QuickCache,
    Result, StoreBuilder,
};
use std::sync::Arc;

struct Client;
struct Executable {
    // Own the client until after native executable destruction.
    _client: Arc<Client>,
    program: String,
}
impl Client {
    fn compile(self: &Arc<Self>, program: &str) -> Arc<Executable> {
        Arc::new(Executable {
            _client: self.clone(),
            program: program.into(),
        })
    }
}
struct ClientMemory {
    client: Arc<Client>,
    entries: QuickCache<Arc<Executable>>,
}
impl InMemoryCache for ClientMemory {
    type Payload = Arc<Executable>;
    fn from_config(_: &MemoryConfig) -> Result<Self> {
        Err(Error::Config {
            field: "client",
            reason: "supply ClientMemory with in_memory",
        })
    }
    fn materialize(&self, bytes: Arc<[u8]>) -> Result<Self::Payload> {
        let program = String::from_utf8(bytes.to_vec()).map_err(Error::producer)?;
        // A real PJRT adapter deserializes with self.client here.
        Ok(Arc::new(Executable {
            _client: self.client.clone(),
            program,
        }))
    }
    fn weight(&self, payload: &Self::Payload) -> u64 {
        payload.program.len() as u64 + 128
    }
    fn get(&self, key: &Key) -> Option<Arc<MemoryEntry<Self::Payload>>> {
        self.entries.get(key)
    }
    fn insert(
        &self,
        key: Key,
        value: Arc<MemoryEntry<Self::Payload>>,
    ) -> Vec<Arc<MemoryEntry<Self::Payload>>> {
        self.entries.insert(key, value)
    }
    fn remove(&self, key: &Key) -> Option<Arc<MemoryEntry<Self::Payload>>> {
        self.entries.remove(key)
    }
    fn clear(&self) -> Vec<Arc<MemoryEntry<Self::Payload>>> {
        self.entries.clear()
    }
}
fn main() -> Result<()> {
    let path = std::env::args_os()
        .nth(1)
        .expect("cache directory argument");
    let client = Arc::new(Client);
    let memory = ClientMemory {
        client: client.clone(),
        entries: QuickCache::new(&MemoryConfig::default())?,
    };
    let store = StoreBuilder::<ClientMemory, NoExtraData>::new()
        .path(path)
        .in_memory(memory)
        .open()?;
    // Persistent recipes include program/options/toolchain/target, never a client pointer.
    let key = Key::from_parts("mock-executable-v1", b"add/shape=2x4/donate=false/cpu-v1");
    let result = store.get_or_insert_payload(key, || {
        let payload = client.compile("add");
        let serialized = Ok(Some(payload.program.as_bytes().to_vec()));
        // On serialization failure use Err(cache_error); for memory-only use Ok(None).
        Ok(Produced {
            payload,
            serialized,
        })
    })?;
    if let Some(error) = result.cache_error {
        eprintln!("cache write failed; executable remains usable: {error}");
    }
    store.force_evict_blob(&key)?;
    drop(store);
    println!("caller still owns executable: {}", result.payload.program);
    Ok(())
}
