use super::{KeyEncodeError, Result, serializer::Encoder};
use serde::{
    Serialize,
    ser::{
        SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
        SerializeTupleStruct, SerializeTupleVariant,
    },
};

pub(super) struct Sequence<'a>(pub &'a mut Encoder);
macro_rules! sequence {
    ($trait:ident, $method:ident) => {
        impl $trait for Sequence<'_> {
            type Ok = ();
            type Error = KeyEncodeError;
            fn $method<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
                value.serialize(&mut *self.0)
            }
            fn end(self) -> Result<()> {
                self.0.tag(0xff);
                Ok(())
            }
        }
    };
}
sequence!(SerializeSeq, serialize_element);
sequence!(SerializeTuple, serialize_element);
sequence!(SerializeTupleStruct, serialize_field);
sequence!(SerializeTupleVariant, serialize_field);

pub(super) struct Mapping<'a> {
    encoder: &'a mut Encoder,
    entries: Vec<(Vec<u8>, Vec<u8>)>,
    pending: Option<Vec<u8>>,
}
impl<'a> Mapping<'a> {
    pub fn new(encoder: &'a mut Encoder) -> Self {
        Self {
            encoder,
            entries: Vec::new(),
            pending: None,
        }
    }
    fn field<T: Serialize + ?Sized>(&mut self, key: &'static str, value: &T) -> Result<()> {
        self.entries
            .push((Encoder::encode(key)?, Encoder::encode(value)?));
        Ok(())
    }
    fn finish(mut self) -> Result<()> {
        if self.pending.is_some() {
            return Err(KeyEncodeError::MissingMapValue);
        }
        self.entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        if self.entries.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(KeyEncodeError::DuplicateMapKey);
        }
        self.encoder
            .bytes
            .extend_from_slice(&(self.entries.len() as u64).to_le_bytes());
        for (key, value) in self.entries {
            self.encoder.bytes.extend_from_slice(&key);
            self.encoder.bytes.extend_from_slice(&value);
        }
        Ok(())
    }
}
impl SerializeMap for Mapping<'_> {
    type Ok = ();
    type Error = KeyEncodeError;
    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<()> {
        if self.pending.is_some() {
            return Err(KeyEncodeError::MissingMapValue);
        }
        self.pending = Some(Encoder::encode(key)?);
        Ok(())
    }
    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        let key = self.pending.take().ok_or(KeyEncodeError::MissingMapKey)?;
        self.entries.push((key, Encoder::encode(value)?));
        Ok(())
    }
    fn end(self) -> Result<()> {
        self.finish()
    }
}
macro_rules! structure {
    ($trait:ident) => {
        impl $trait for Mapping<'_> {
            type Ok = ();
            type Error = KeyEncodeError;
            fn serialize_field<T: Serialize + ?Sized>(
                &mut self,
                key: &'static str,
                value: &T,
            ) -> Result<()> {
                self.field(key, value)
            }
            fn end(self) -> Result<()> {
                self.finish()
            }
        }
    };
}
structure!(SerializeStruct);
structure!(SerializeStructVariant);
