use compcache::{Key, SchemaKey};
struct UserKey<'a> {
    compiler: &'a str,
    target: &'a str,
    source: &'a str,
}
impl SchemaKey<3> for UserKey<'_> {
    const KEY_FIELDS: [&'static str; 3] = ["compiler", "target", "source"];
    fn key_value(&self) -> [&str; 3] {
        [self.compiler, self.target, self.source]
    }
}
struct OtherSchema<'a>(UserKey<'a>);
impl SchemaKey<3> for OtherSchema<'_> {
    const KEY_FIELDS: [&'static str; 3] = ["compiler", "target", "runtime"];
    fn key_value(&self) -> [&str; 3] {
        self.0.key_value()
    }
}
#[test]
fn structured_keys_derive_their_schema_and_frame_fields_without_ambiguity() {
    let a = UserKey {
        compiler: "ab",
        target: "c",
        source: "digest",
    };
    let b = UserKey {
        compiler: "a",
        target: "bc",
        source: "digest",
    };
    assert_eq!(Key::new(&a), Key::new(&a));
    assert_ne!(Key::new(&a), Key::new(&b));
    assert_ne!(Key::new(&a), Key::new(&OtherSchema(a)));
}
