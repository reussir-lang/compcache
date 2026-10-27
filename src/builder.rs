use crate::{
    Config, Error, ExtraOnDiskData, InMemoryCache, ManagedDirectory, QuickCache, Result, Store,
};
use std::path::PathBuf;

/// Choose strategies through types, then provide a path and configuration.
///
/// ```no_run
/// use compcache::{Bypass, Config, NoExtraData, StoreBuilder};
/// let store = StoreBuilder::<Bypass, NoExtraData>::new()
///     .path("/tmp/compiler-cache")
///     .config(Config::from_nickel("config/default.ncl")?)
///     .open()?;
/// # Ok::<(), compcache::Error>(())
/// ```
pub struct StoreBuilder<I = QuickCache, D = ManagedDirectory> {
    path: Option<PathBuf>,
    config: Config,
    memory: Option<I>,
    extra: Option<D>,
}
impl<I, D> Default for StoreBuilder<I, D> {
    fn default() -> Self {
        Self::new()
    }
}
impl<I, D> StoreBuilder<I, D> {
    pub fn new() -> Self {
        Self {
            path: None,
            config: Config::default(),
            memory: None,
            extra: None,
        }
    }
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }
    pub fn config(mut self, config: Config) -> Self {
        self.config = config;
        self
    }
    pub fn in_memory(mut self, memory: I) -> Self {
        self.memory = Some(memory);
        self
    }
    pub fn extra_on_disk(mut self, extra: D) -> Self {
        self.extra = Some(extra);
        self
    }
}
impl<I: InMemoryCache, D: ExtraOnDiskData> StoreBuilder<I, D> {
    pub fn open(self) -> Result<Store<I, D>> {
        self.config.validate()?;
        let root = self.path.ok_or(Error::Config {
            field: "path",
            reason: "a cache directory is required",
        })?;
        crate::filesystem::make_directory(&root)?;
        let memory = if self.config.memory.enabled {
            Some(match self.memory {
                Some(memory) => memory,
                None => I::from_config(&self.config.memory)?,
            })
        } else {
            None
        };
        let extra = if self.config.disk.directories {
            match self.extra {
                Some(extra) => Some(extra),
                None => D::from_config(root.join("objects"), &self.config.disk)?,
            }
        } else {
            None
        };
        Store::open_components(&root, self.config, memory, extra)
    }
}
