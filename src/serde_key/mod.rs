//! Versioned serde key encoding. Encoding changes require a new domain prefix.
mod compound;
mod serializer;

use crate::Key;
use serde::Serialize;
use serializer::Encoder;

/// A failure while deriving identity from a serde value.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum KeyEncodeError {
    #[error("key serialization failed: {0}")]
    Serialize(String),
    #[error("serialized map contains duplicate keys")]
    DuplicateMapKey,
    #[error("serialized map value has no corresponding key")]
    MissingMapKey,
    #[error("serialized map key has no corresponding value")]
    MissingMapValue,
}
impl serde::ser::Error for KeyEncodeError {
    fn custom<T: std::fmt::Display>(message: T) -> Self {
        Self::Serialize(message.to_string())
    }
}
pub(super) type Result<T> = std::result::Result<T, KeyEncodeError>;

impl Key {
    /// Derive a key from any serde value using a deterministic binary encoding.
    ///
    /// `namespace` identifies the recipe's meaning/version, e.g. `bytecode-v1`.
    /// Include compiler version, target, input digests and relevant options.
    /// Maps and named fields are sorted; sequences retain order. Floating-point
    /// bits and serde value kinds are preserved. `Serialize` runs in binary mode.
    /// Custom serializers must be deterministic; unordered sets need caller-side
    /// ordering because serde represents them as sequences. Identity covers the
    /// serialized projection, including any serde renames/skipped fields.
    ///
    /// This allocates for encoding and map sorting; compute once for hot lookups.
    /// The existing `SchemaKey` encoding and its digests remain unchanged.
    ///
    /// ```
    /// use compcache::Key;
    /// use serde::Serialize;
    /// #[derive(Serialize)]
    /// struct UserKey<'a> {
    ///     compiler: &'a str,
    ///     target: &'a str,
    ///     inputs: &'a [[u8; 32]],
    ///     optimization: u8,
    /// }
    /// let recipe = UserKey { compiler: "v1", target: "x86_64", inputs: &[], optimization: 2 };
    /// let key = Key::from_serde("bytecode-v1", &recipe)?;
    /// # Ok::<(), compcache::Error>(())
    /// ```
    ///
    /// # Errors
    ///
    /// Returns `Error::KeyEncode` for failed serialization or malformed/duplicate map entries.
    pub fn from_serde<T: Serialize + ?Sized>(namespace: &str, recipe: &T) -> crate::Result<Self> {
        let encoded = Encoder::encode(recipe)?;
        let mut hash = blake3::Hasher::new();
        hash.update(b"compcache/serde-key/v1\0");
        hash.update(&(namespace.len() as u64).to_le_bytes());
        hash.update(namespace.as_bytes());
        hash.update(&encoded);
        Ok(Self(*hash.finalize().as_bytes()))
    }
}
