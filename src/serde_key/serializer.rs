use super::{
    KeyEncodeError, Result,
    compound::{Mapping, Sequence},
};
use serde::{Serialize, Serializer};

#[derive(Default)]
pub(super) struct Encoder {
    pub bytes: Vec<u8>,
}
impl Encoder {
    pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
        let mut encoder = Self::default();
        value.serialize(&mut encoder)?;
        Ok(encoder.bytes)
    }
    pub fn tag(&mut self, tag: u8) {
        self.bytes.push(tag);
    }
    pub fn frame(&mut self, bytes: &[u8]) {
        self.bytes
            .extend_from_slice(&(bytes.len() as u64).to_le_bytes());
        self.bytes.extend_from_slice(bytes);
    }
    fn sequence(&mut self, tag: u8) -> Sequence<'_> {
        self.tag(tag);
        Sequence(self)
    }
    fn mapping(&mut self, tag: u8) -> Mapping<'_> {
        self.tag(tag);
        Mapping::new(self)
    }
}
macro_rules! integer {
    ($method:ident, $type:ty, $tag:literal) => {
        fn $method(self, value: $type) -> Result<()> {
            self.tag($tag);
            self.bytes.extend_from_slice(&value.to_le_bytes());
            Ok(())
        }
    };
}
impl<'a> Serializer for &'a mut Encoder {
    type Ok = ();
    type Error = KeyEncodeError;
    type SerializeSeq = Sequence<'a>;
    type SerializeTuple = Sequence<'a>;
    type SerializeTupleStruct = Sequence<'a>;
    type SerializeTupleVariant = Sequence<'a>;
    type SerializeMap = Mapping<'a>;
    type SerializeStruct = Mapping<'a>;
    type SerializeStructVariant = Mapping<'a>;

    fn serialize_bool(self, value: bool) -> Result<()> {
        self.tag(0x01);
        self.bytes.push(u8::from(value));
        Ok(())
    }
    integer!(serialize_i8, i8, 0x02);
    integer!(serialize_i16, i16, 0x03);
    integer!(serialize_i32, i32, 0x04);
    integer!(serialize_i64, i64, 0x05);
    integer!(serialize_i128, i128, 0x06);
    integer!(serialize_u8, u8, 0x07);
    integer!(serialize_u16, u16, 0x08);
    integer!(serialize_u32, u32, 0x09);
    integer!(serialize_u64, u64, 0x0a);
    integer!(serialize_u128, u128, 0x0b);
    fn serialize_f32(self, value: f32) -> Result<()> {
        self.tag(0x0c);
        self.bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        Ok(())
    }
    fn serialize_f64(self, value: f64) -> Result<()> {
        self.tag(0x0d);
        self.bytes.extend_from_slice(&value.to_bits().to_le_bytes());
        Ok(())
    }
    fn serialize_char(self, value: char) -> Result<()> {
        self.tag(0x0e);
        self.bytes.extend_from_slice(&(value as u32).to_le_bytes());
        Ok(())
    }
    fn serialize_str(self, value: &str) -> Result<()> {
        self.tag(0x0f);
        self.frame(value.as_bytes());
        Ok(())
    }
    fn serialize_bytes(self, value: &[u8]) -> Result<()> {
        self.tag(0x10);
        self.frame(value);
        Ok(())
    }
    fn serialize_none(self) -> Result<()> {
        self.tag(0x11);
        Ok(())
    }
    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<()> {
        self.tag(0x12);
        value.serialize(self)
    }
    fn serialize_unit(self) -> Result<()> {
        self.tag(0x13);
        Ok(())
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<()> {
        self.tag(0x14);
        Ok(())
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, variant: &'static str) -> Result<()> {
        self.tag(0x15);
        self.frame(variant.as_bytes());
        Ok(())
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> Result<()> {
        self.tag(0x16);
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<()> {
        self.tag(0x17);
        self.frame(variant.as_bytes());
        value.serialize(self)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq> {
        Ok(self.sequence(0x20))
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple> {
        Ok(self.sequence(0x21))
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        Ok(self.sequence(0x22))
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        self.tag(0x23);
        self.frame(variant.as_bytes());
        Ok(Sequence(self))
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap> {
        Ok(self.mapping(0x24))
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeStruct> {
        Ok(self.mapping(0x25))
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant> {
        self.tag(0x26);
        self.frame(variant.as_bytes());
        Ok(Mapping::new(self))
    }
    fn is_human_readable(&self) -> bool {
        false
    }
}
