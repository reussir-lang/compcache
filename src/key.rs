/// A structured key schema.
///
/// Field names and values have the same compile-time
/// length; values borrow from the caller and need no serialization or formatting.
/// Include the compiler version, target, input digests and all relevant options.
///
/// ```
/// use compcache::{Key, SchemaKey};
/// struct UserKey<'a> { compiler: &'a str, target: &'a str, source: &'a str }
/// impl SchemaKey<3> for UserKey<'_> {
///     const KEY_FIELDS: [&'static str; 3] = ["compiler", "target", "source"];
///     fn key_value(&self) -> [&str; 3] { [self.compiler, self.target, self.source] }
/// }
/// let key = Key::new(&UserKey { compiler: "v1", target: "x86_64", source: "source-digest" });
/// ```
pub trait SchemaKey<const KEY_LEN: usize> {
    const KEY_FIELDS: [&'static str; KEY_LEN];
    fn key_value(&self) -> [&str; KEY_LEN];
}

/// Immutable BLAKE3-256 recipe identity. Compute once and reuse for hot lookups.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Key(pub(crate) [u8; 32]);
impl Key {
    /// Hash the schema and its corresponding values with unambiguous framing.
    pub fn new<const KEY_LEN: usize, K: SchemaKey<KEY_LEN> + ?Sized>(recipe: &K) -> Self {
        let mut hash = blake3::Hasher::new();
        hash.update(b"compcache/schema-key/v1\0");
        hash.update(&(KEY_LEN as u64).to_le_bytes());
        for (field, value) in K::KEY_FIELDS.into_iter().zip(recipe.key_value()) {
            frame(&mut hash, field.as_bytes());
            frame(&mut hash, value.as_bytes());
        }
        Self(*hash.finalize().as_bytes())
    }
    /// For callers with an existing stable binary recipe encoding.
    #[must_use]
    pub fn from_parts(domain: &str, recipe: &[u8]) -> Self {
        let mut hash = blake3::Hasher::new();
        frame(&mut hash, domain.as_bytes());
        hash.update(recipe);
        Self(*hash.finalize().as_bytes())
    }
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    #[must_use]
    pub fn to_hex(self) -> String {
        blake3::Hash::from_bytes(self.0).to_hex().to_string()
    }
}
fn frame(hash: &mut blake3::Hasher, bytes: &[u8]) {
    hash.update(&(bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}
impl std::fmt::Display for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&blake3::Hash::from_bytes(self.0).to_hex())
    }
}
