use crate::{
    Config, Error, ExtraOnDiskData, InMemoryCache, ManagedDirectory, QuickCache, Result, Store,
    StoreDefaults, defaults, error::IoContext,
};
use std::{fs, io, path::PathBuf};

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
#[must_use = "a builder must be opened to create a store"]
pub struct StoreBuilder<I = QuickCache, D = ManagedDirectory> {
    path: Option<PathBuf>,
    config: Config,
    config_path: Option<PathBuf>,
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
            config_path: None,
            memory: None,
            extra: None,
        }
    }
    /// Use application defaults and the platform's standard config/cache directories.
    ///
    /// Nonempty environment overrides replace the complete application directory.
    /// At [`Self::open`], `config.ncl` in the selected config directory overrides
    /// [`StoreDefaults::config`]. Omitted fields retain the application defaults.
    /// An absent file uses defaults without invoking Nickel. Other configuration
    /// errors are returned. No config file or config directory is created.
    ///
    /// Subsequent calls to [`Self::path`] or [`Self::config`] override these choices.
    /// This constructor leaves the standard [`Default`] implementation unchanged.
    ///
    /// ```no_run
    /// use compcache::{Config, Store, StoreBuilder, StoreDefaults};
    /// struct App;
    /// impl StoreDefaults for App {
    ///     const APP_NAME: &'static str = "my-compiler";
    ///     const CACHE_DIR_ENV: Option<&'static str> = Some("MY_COMPILER_CACHE_DIR");
    ///     const CONFIG_DIR_ENV: Option<&'static str> = Some("MY_COMPILER_CONFIG_DIR");
    ///     fn config() -> Config {
    ///         let mut config = Config::default();
    ///         config.disk.prune_trigger_bytes = 8 << 30;
    ///         config
    ///     }
    /// }
    /// let store: Store = StoreBuilder::with_defaults::<App>()?.open()?;
    /// # Ok::<(), compcache::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns a configuration error for an invalid app/environment name or an unavailable platform directory.
    pub fn with_defaults<T: StoreDefaults>() -> Result<Self> {
        defaults::validate_app_name(T::APP_NAME)?;
        let cache_dir =
            defaults::directory(T::APP_NAME, T::CACHE_DIR_ENV, dirs::cache_dir, "cache_dir")?;
        let config_dir = defaults::directory(
            T::APP_NAME,
            T::CONFIG_DIR_ENV,
            dirs::config_dir,
            "config_dir",
        )?;
        Ok(Self {
            path: Some(cache_dir),
            config: T::config(),
            config_path: Some(config_dir.join("config.ncl")),
            memory: None,
            extra: None,
        })
    }
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }
    pub fn config(mut self, config: Config) -> Self {
        self.config = config;
        self.config_path = None;
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
    /// Validate configuration, initialize the backends and recover interrupted work.
    ///
    /// # Errors
    ///
    /// Returns configuration, backend initialization, filesystem or SQLite errors. Existing invalid configuration files are reported rather than replaced by defaults.
    pub fn open(mut self) -> Result<Store<I, D>> {
        if let Some(path) = &self.config_path {
            // Windows reports a child of a regular file as NotFound. Reject an
            // invalid configured directory before considering a missing file.
            if let Some(parent) = path.parent() {
                match fs::metadata(parent) {
                    Ok(metadata) if !metadata.is_dir() => {
                        return Err(io::Error::new(
                            io::ErrorKind::NotADirectory,
                            "configuration directory is not a directory",
                        ))
                        .at("inspect configuration directory", parent);
                    }
                    Err(error) if error.kind() != io::ErrorKind::NotFound => {
                        return Err(error).at("inspect configuration directory", parent);
                    }
                    _ => {}
                }
            }
            match fs::metadata(path) {
                Ok(_) => {
                    self.config = Config::from_nickel_with_defaults(path, self.config)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error).at("inspect configuration file", path),
            }
        }
        self.config.validate()?;
        let root = self.path.ok_or(Error::Config {
            field: "path",
            reason: "a cache directory is required",
        })?;
        crate::filesystem::make_directory(&root)?;
        // Disabling admission must not discard caller context/materialization hooks.
        let memory = match self.memory {
            Some(memory) => memory,
            None => I::from_config(&self.config.memory)?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Bypass, NoExtraData};
    use std::process::Command;

    struct App;
    impl StoreDefaults for App {
        const APP_NAME: &'static str = "test-compiler";
        const CONFIG_DIR_ENV: Option<&'static str> = Some("TEST_COMPILER_CONFIG_DIR");
        const CACHE_DIR_ENV: Option<&'static str> = Some("TEST_COMPILER_CACHE_DIR");

        fn config() -> Config {
            let mut config = Config::default();
            config.disk.prune_trigger_bytes = 123_456;
            config.disk.io_backoff.max_elapsed_ms = 321;
            config
        }
    }

    // Environment changes are confined to child processes, without unsafe set_var.
    #[test]
    #[ignore = "invoked in a child process with an isolated environment"]
    fn defaults_worker() {
        let expected_cache = PathBuf::from(std::env::var_os("EXPECTED_CACHE").unwrap());
        let expected_config = PathBuf::from(std::env::var_os("EXPECTED_CONFIG").unwrap());
        let mode = std::env::var("DEFAULTS_TEST_MODE").unwrap();
        let mut builder = StoreBuilder::<Bypass, NoExtraData>::with_defaults::<App>().unwrap();
        assert_eq!(builder.path.as_ref(), Some(&expected_cache));
        assert_eq!(
            builder.config_path,
            Some(expected_config.join("config.ncl"))
        );
        assert!(!expected_cache.exists());

        if mode == "explicit" {
            builder = builder
                .path(expected_cache.join("explicit"))
                .config(Config::default());
        }
        let result = builder.open();
        if mode == "invalid" {
            assert!(matches!(result, Err(Error::Nickel { .. })));
            assert!(!expected_cache.exists());
            return;
        }
        if mode == "io-error" {
            assert!(matches!(result, Err(Error::Io { .. })));
            return;
        }
        let store = result.unwrap();
        if mode == "explicit" {
            assert_eq!(
                store.root(),
                fs::canonicalize(expected_cache.join("explicit")).unwrap()
            );
            assert_eq!(
                store.config.disk.prune_trigger_bytes,
                Config::default().disk.prune_trigger_bytes
            );
        } else {
            assert_eq!(store.root(), fs::canonicalize(expected_cache).unwrap());
            assert_eq!(store.config.disk.prune_trigger_bytes, 123_456);
            assert_eq!(store.config.disk.io_backoff.max_elapsed_ms, 321);
            if mode == "partial" {
                assert!(!store.config.memory.enabled);
                assert!(!store.config.disk.io_backoff.jitter);
            }
        }
    }

    #[test]
    fn application_defaults_paths_and_overrides() {
        if cfg!(target_os = "linux") {
            test_defaults(&["xdg", "home"]);
        }
        test_defaults(&["overrides", "io-error", "explicit"]);
    }

    #[test]
    fn application_defaults_config_loading() {
        test_defaults(&["partial", "invalid"]);
    }

    fn test_defaults(modes: &[&str]) {
        for &mode in modes {
            let root = tempfile::tempdir().unwrap();
            let mut command = Command::new(std::env::current_exe().unwrap());
            command
                .args([
                    "--exact",
                    "builder::tests::defaults_worker",
                    "--ignored",
                    "--nocapture",
                ])
                .env("DEFAULTS_TEST_MODE", mode)
                .env("HOME", root.path().join("home"))
                .env("XDG_CACHE_HOME", root.path().join("xdg-cache"))
                .env("XDG_CONFIG_HOME", root.path().join("xdg-config"))
                .env_remove("TEST_COMPILER_CACHE_DIR")
                .env_remove("TEST_COMPILER_CONFIG_DIR");
            let (cache, config) = match mode {
                "xdg" => (
                    root.path().join("xdg-cache/test-compiler"),
                    root.path().join("xdg-config/test-compiler"),
                ),
                "home" => {
                    command
                        .env("XDG_CACHE_HOME", "relative")
                        .env("XDG_CONFIG_HOME", "")
                        .env("TEST_COMPILER_CACHE_DIR", "")
                        .env("TEST_COMPILER_CONFIG_DIR", "");
                    (
                        root.path().join("home/.cache/test-compiler"),
                        root.path().join("home/.config/test-compiler"),
                    )
                }
                _ => {
                    let cache = root.path().join("custom-cache");
                    let config = root.path().join("custom-config");
                    command
                        .env("TEST_COMPILER_CACHE_DIR", &cache)
                        .env("TEST_COMPILER_CONFIG_DIR", &config);
                    (cache, config)
                }
            };
            match mode {
                "partial" | "invalid" | "explicit" => {
                    fs::create_dir_all(&config).unwrap();
                    let contents = if mode == "partial" {
                        "{ memory.enabled = false, disk.io_backoff.jitter = false }"
                    } else {
                        "invalid Nickel syntax {{"
                    };
                    fs::write(config.join("config.ncl"), contents).unwrap();
                }
                "io-error" => {
                    // A file used as a directory must not be treated as a missing config.
                    fs::write(&config, "not a directory").unwrap();
                }
                _ => {
                    command.env("PATH", ""); // No Nickel needed for a missing file.
                }
            }
            let output = command
                .env("EXPECTED_CACHE", &cache)
                .env("EXPECTED_CONFIG", &config)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{mode}: {}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if matches!(mode, "xdg" | "home" | "overrides") {
                assert!(!config.exists());
            }
        }
    }
}
