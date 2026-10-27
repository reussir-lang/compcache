use compcache::{Error, Key, KeyEncodeError};
use serde::{
    Serialize, Serializer,
    ser::{Error as _, SerializeMap},
};
use std::collections::HashSet;

#[derive(Serialize)]
enum Target {
    Native,
    Cross {
        architecture: &'static str,
        width: u8,
    },
}
#[derive(Serialize)]
struct Recipe {
    compiler: &'static str,
    target: Target,
    inputs: Vec<[u8; 32]>,
    flags: Option<Vec<String>>,
}
struct OrderedMap<K, V>(Vec<(K, V)>);
impl<K: Serialize, V: Serialize> Serialize for OrderedMap<K, V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        for (key, value) in &self.0 {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}
fn key<T: Serialize + ?Sized>(value: &T) -> Key {
    Key::from_serde("test-v1", value).unwrap()
}

#[test]
fn nested_recipe_fields_and_namespace_determine_identity() {
    let mut recipe = Recipe {
        compiler: "v1",
        target: Target::Cross {
            architecture: "aarch64",
            width: 64,
        },
        inputs: vec![[1; 32], [2; 32]],
        flags: Some(vec!["-O2".into()]),
    };
    let initial = key(&recipe);
    assert_eq!(initial, key(&recipe));
    recipe.inputs[0][0] = 9;
    assert_ne!(initial, key(&recipe));
    recipe.inputs[0][0] = 1;
    recipe.target = Target::Native;
    assert_ne!(initial, key(&recipe));
    assert_ne!(key(&recipe), Key::from_serde("test-v2", &recipe).unwrap());
}

#[test]
fn map_order_is_canonical_at_every_depth_and_non_string_keys_work() {
    let left = OrderedMap(vec![
        (1i32, OrderedMap(vec![("a", vec![1u64, 2]), ("b", vec![3])])),
        (2, OrderedMap(vec![("c", vec![4])])),
    ]);
    let right = OrderedMap(vec![
        (2i32, OrderedMap(vec![("c", vec![4u64])])),
        (1, OrderedMap(vec![("b", vec![3]), ("a", vec![1, 2])])),
    ]);
    assert_eq!(key(&left), key(&right));
    assert_ne!(key(&vec![1u64, 2]), key(&vec![2u64, 1]));
    // Container length hints are not part of the identity.
    let standard: std::collections::BTreeMap<_, _> = left.0.iter().map(|(k, v)| (k, v)).collect();
    assert_eq!(key(&left), key(&standard));
}

#[test]
fn named_field_order_is_canonical_and_serialized_projection_is_respected() {
    #[derive(Serialize)]
    struct A {
        compiler: u8,
        target: &'static str,
    }
    #[derive(Serialize)]
    struct B {
        target: &'static str,
        compiler: u8,
    }
    #[derive(Serialize)]
    struct Renamed {
        #[serde(rename = "version")]
        compiler: u8,
        target: &'static str,
    }
    #[derive(Serialize)]
    struct Skipped {
        input: u8,
        #[serde(skip)]
        _diagnostics: bool,
    }
    assert_eq!(
        key(&A {
            compiler: 1,
            target: "x86_64"
        }),
        key(&B {
            target: "x86_64",
            compiler: 1
        })
    );
    assert_ne!(
        key(&A {
            compiler: 1,
            target: "x86_64"
        }),
        key(&Renamed {
            compiler: 1,
            target: "x86_64"
        })
    );
    assert_eq!(
        key(&Skipped {
            input: 1,
            _diagnostics: false
        }),
        key(&Skipped {
            input: 1,
            _diagnostics: true
        })
    );
}

struct Bytes<'a>(&'a [u8]);
impl Serialize for Bytes<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
    }
}
#[test]
fn value_kinds_variants_and_boundaries_remain_distinct() {
    #[derive(Serialize)]
    enum E {
        Unit,
        Value(u8),
        Pair(u8, bool),
        Named { value: u8 },
    }
    #[derive(Serialize)]
    struct Unit;
    #[derive(Serialize)]
    struct Newtype(u8);
    #[derive(Serialize)]
    struct Tuple(u8, bool);
    let keys = [
        key(&false),
        key(&0i8),
        key(&0i16),
        key(&0i32),
        key(&0i64),
        key(&0i128),
        key(&0u8),
        key(&0u16),
        key(&0u32),
        key(&0u64),
        key(&0u128),
        key(&0f32),
        key(&0f64),
        key(&'a'),
        key("a"),
        key(&Bytes(b"a")),
        key(&None::<()>),
        key(&Some(())),
        key(&()),
        key(&Unit),
        key(&Newtype(0)),
        key(&Tuple(0, false)),
        key(&(0u8, false)),
        key(&E::Unit),
        key(&E::Value(0)),
        key(&E::Pair(0, false)),
        key(&E::Named { value: 0 }),
        key("Unit"),
    ];
    assert_eq!(keys.len(), keys.into_iter().collect::<HashSet<_>>().len());
    assert_ne!(key(&("ab", "c")), key(&("a", "bc")));
    assert_ne!(key(&("a\0", "b")), key(&("a", "\0b")));
    assert_ne!(key(&Bytes(&[1, 2])), key(&vec![1u8, 2]));
    assert_ne!(key(&vec![1u8, 2]), key(&[1u8, 2]));
}

#[test]
fn floating_point_bits_and_full_width_integers_are_preserved() {
    assert_ne!(key(&0.0f64), key(&-0.0f64));
    assert_ne!(
        key(&f64::from_bits(0x7ff8_0000_0000_0001)),
        key(&f64::from_bits(0x7ff8_0000_0000_0002))
    );
    assert_eq!(key(&f64::INFINITY), key(&f64::INFINITY));
    assert_ne!(key(&u128::MAX), key(&(u128::MAX - 1)));
    assert_ne!(key(&i128::MIN), key(&(i128::MIN + 1)));
}

#[test]
fn binary_mode_and_typed_serialization_errors_are_exposed() {
    struct Mode;
    impl Serialize for Mode {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            assert!(!serializer.is_human_readable());
            serializer.serialize_bytes(&[1, 2])
        }
    }
    struct Fails;
    impl Serialize for Fails {
        fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
            Err(S::Error::custom("fixture failure"))
        }
    }
    assert_eq!(key(&Mode), key(&Bytes(&[1, 2])));
    assert!(
        matches!(Key::from_serde("test",&Fails),Err(Error::KeyEncode(KeyEncodeError::Serialize(message))) if message == "fixture failure")
    );
    assert!(matches!(
        Key::from_serde("test", &OrderedMap(vec![("same", 1u8), ("same", 2)])),
        Err(Error::KeyEncode(KeyEncodeError::DuplicateMapKey))
    ));
}

#[test]
fn persisted_key_vector() {
    assert_eq!(
        Key::from_serde("fixture-v1", &("rustc", 1u64, Some(true)))
            .unwrap()
            .to_hex(),
        "fcd740165508a81bd1a9df0a4f183823ab6035f20497386f58d1ecc24abe9990"
    );
}
