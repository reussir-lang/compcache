use crate::{Config, Error, Result};
use std::path::{Component, Path, PathBuf};

/// Application defaults used by [`crate::StoreBuilder::with_defaults`].
pub trait StoreDefaults {
    /// A single directory name, appended to the platform's config/cache roots.
    const APP_NAME: &'static str;

    /// Optional environment variable containing the complete config directory.
    /// Empty values are ignored; relative paths are relative to the working directory.
    const CONFIG_DIR_ENV: Option<&'static str> = None;

    /// Optional environment variable containing the complete cache directory.
    /// Empty values are ignored; no application name is appended to overrides.
    const CACHE_DIR_ENV: Option<&'static str> = None;

    /// Fallback values for an absent config file and fields omitted from it.
    #[must_use]
    fn config() -> Config {
        Config::default()
    }
}

pub(crate) fn validate_app_name(name: &str) -> Result<()> {
    let mut components = Path::new(name).components();
    if name.is_empty()
        || name.contains(['/', '\\', '\0'])
        || !matches!(components.next(), Some(Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(Error::Config {
            field: "app_name",
            reason: "must be a single normal directory name",
        });
    }
    Ok(())
}

pub(crate) fn directory(
    app_name: &str,
    variable: Option<&str>,
    platform_directory: impl FnOnce() -> Option<PathBuf>,
    field: &'static str,
) -> Result<PathBuf> {
    if let Some(variable) = variable {
        if variable.is_empty() || variable.contains(['=', '\0']) {
            return Err(Error::Config {
                field,
                reason: "environment variable name must be nonempty and contain neither '=' nor NUL",
            });
        }
        if let Some(value) = std::env::var_os(variable).filter(|value| !value.is_empty()) {
            return Ok(value.into());
        }
    }
    platform_directory()
        .map(|root| root.join(app_name))
        .ok_or(Error::Config {
            field,
            reason: "could not determine the user's platform directory; set an environment override",
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_name_cannot_escape_the_platform_directory() {
        assert!(validate_app_name("my-compiler").is_ok());
        for name in ["", ".", "..", "/tmp/cache", "a/b", "a\\b", "a\0b"] {
            assert!(matches!(
                validate_app_name(name),
                Err(Error::Config {
                    field: "app_name",
                    ..
                })
            ));
        }
    }

    #[test]
    fn missing_platform_directory_and_invalid_environment_names_are_errors() {
        assert!(matches!(
            directory("app", None, || None, "cache_dir"),
            Err(Error::Config {
                field: "cache_dir",
                ..
            })
        ));
        for name in ["", "a=b", "a\0b"] {
            assert!(directory("app", Some(name), || None, "config_dir").is_err());
        }
    }
}
