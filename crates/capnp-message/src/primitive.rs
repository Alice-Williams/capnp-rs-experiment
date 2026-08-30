//! Low-level and dynamic readers for Cap'n Proto data-section values.
//!
//! The representation follows the pinned C++ `layout.h` field accessors and
//! the encoding specification: scalar fields are little-endian, absent data is
//! all-zero, and schema defaults are XORed with wire bits. Enum ordinals remain
//! representable when a newer sender uses an enumerant unknown to the reader.
//!
//! This module only reads already-bounded immutable data sections. Struct
//! evolution, pointer defaults, lists, reflection descriptors, and generated
//! enum types belong to later milestones.

use core::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimitiveError {
    DataSectionNotWordAligned {
        bytes: usize,
    },
    OffsetOverflow,
    DefaultTypeMismatch {
        expected: PrimitiveType,
        actual: PrimitiveType,
    },
}

impl fmt::Display for PrimitiveError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for PrimitiveError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrimitiveType {
    Void,
    Bool,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
    Enum,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PrimitiveValue {
    Void,
    Bool(bool),
    Int8(i8),
    Int16(i16),
    Int32(i32),
    Int64(i64),
    UInt8(u8),
    UInt16(u16),
    UInt32(u32),
    UInt64(u64),
    Float32(f32),
    Float64(f64),
    Enum(u16),
}

impl PrimitiveValue {
    pub const fn primitive_type(self) -> PrimitiveType {
        match self {
            Self::Void => PrimitiveType::Void,
            Self::Bool(_) => PrimitiveType::Bool,
            Self::Int8(_) => PrimitiveType::Int8,
            Self::Int16(_) => PrimitiveType::Int16,
            Self::Int32(_) => PrimitiveType::Int32,
            Self::Int64(_) => PrimitiveType::Int64,
            Self::UInt8(_) => PrimitiveType::UInt8,
            Self::UInt16(_) => PrimitiveType::UInt16,
            Self::UInt32(_) => PrimitiveType::UInt32,
            Self::UInt64(_) => PrimitiveType::UInt64,
            Self::Float32(_) => PrimitiveType::Float32,
            Self::Float64(_) => PrimitiveType::Float64,
            Self::Enum(_) => PrimitiveType::Enum,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnumValue<E> {
    Known(E),
    Unknown(u16),
}

/// An immutable, zero-copy view of a struct's word-aligned data section.
///
/// ```
/// use capnp_message::DataSection;
/// let wire = [0x34, 0x12, 0, 0, 0, 0, 0, 0];
/// let data = DataSection::new(&wire)?;
/// assert_eq!(data.read_u16(0, 0)?, 0x1234);
/// # Ok::<(), capnp_message::PrimitiveError>(())
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DataSection<'a> {
    bytes: &'a [u8],
}

impl<'a> DataSection<'a> {
    pub fn new(bytes: &'a [u8]) -> Result<Self, PrimitiveError> {
        if bytes.len() % 8 != 0 {
            return Err(PrimitiveError::DataSectionNotWordAligned { bytes: bytes.len() });
        }
        Ok(Self { bytes })
    }

    pub const fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }

    pub const fn read_void(self) {}

    pub fn read_bool(self, bit_offset: u32, default: bool) -> Result<bool, PrimitiveError> {
        let byte_offset =
            usize::try_from(bit_offset / 8).map_err(|_| PrimitiveError::OffsetOverflow)?;
        let bit = u8::try_from(bit_offset % 8).map_err(|_| PrimitiveError::OffsetOverflow)?;
        let wire = self
            .bytes
            .get(byte_offset)
            .copied()
            .is_some_and(|byte| byte & (1 << bit) != 0);
        Ok(wire ^ default)
    }

    pub fn read_u8(self, offset: u32, default: u8) -> Result<u8, PrimitiveError> {
        Ok(u8::from_le_bytes(self.wire_bytes(offset)?) ^ default)
    }

    pub fn read_i8(self, offset: u32, default: i8) -> Result<i8, PrimitiveError> {
        Ok((self.read_u8(offset, default as u8)?) as i8)
    }

    pub fn read_u16(self, offset: u32, default: u16) -> Result<u16, PrimitiveError> {
        Ok(u16::from_le_bytes(self.wire_bytes(offset)?) ^ default)
    }

    pub fn read_i16(self, offset: u32, default: i16) -> Result<i16, PrimitiveError> {
        Ok((self.read_u16(offset, default as u16)?) as i16)
    }

    pub fn read_u32(self, offset: u32, default: u32) -> Result<u32, PrimitiveError> {
        Ok(u32::from_le_bytes(self.wire_bytes(offset)?) ^ default)
    }

    pub fn read_i32(self, offset: u32, default: i32) -> Result<i32, PrimitiveError> {
        Ok((self.read_u32(offset, default as u32)?) as i32)
    }

    pub fn read_u64(self, offset: u32, default: u64) -> Result<u64, PrimitiveError> {
        Ok(u64::from_le_bytes(self.wire_bytes(offset)?) ^ default)
    }

    pub fn read_i64(self, offset: u32, default: i64) -> Result<i64, PrimitiveError> {
        Ok((self.read_u64(offset, default as u64)?) as i64)
    }

    pub fn read_f32(self, offset: u32, default: f32) -> Result<f32, PrimitiveError> {
        Ok(f32::from_bits(self.read_u32(offset, default.to_bits())?))
    }

    pub fn read_f64(self, offset: u32, default: f64) -> Result<f64, PrimitiveError> {
        Ok(f64::from_bits(self.read_u64(offset, default.to_bits())?))
    }

    pub fn read_enum<E: TryFrom<u16>>(
        self,
        offset: u32,
        default_ordinal: u16,
    ) -> Result<EnumValue<E>, PrimitiveError> {
        let ordinal = self.read_u16(offset, default_ordinal)?;
        Ok(match E::try_from(ordinal) {
            Ok(value) => EnumValue::Known(value),
            Err(_) => EnumValue::Unknown(ordinal),
        })
    }

    pub fn read_dynamic(
        self,
        primitive_type: PrimitiveType,
        offset: u32,
        default: PrimitiveValue,
    ) -> Result<PrimitiveValue, PrimitiveError> {
        let actual = default.primitive_type();
        if primitive_type != actual {
            return Err(PrimitiveError::DefaultTypeMismatch {
                expected: primitive_type,
                actual,
            });
        }
        Ok(match default {
            PrimitiveValue::Void => PrimitiveValue::Void,
            PrimitiveValue::Bool(value) => PrimitiveValue::Bool(self.read_bool(offset, value)?),
            PrimitiveValue::Int8(value) => PrimitiveValue::Int8(self.read_i8(offset, value)?),
            PrimitiveValue::Int16(value) => PrimitiveValue::Int16(self.read_i16(offset, value)?),
            PrimitiveValue::Int32(value) => PrimitiveValue::Int32(self.read_i32(offset, value)?),
            PrimitiveValue::Int64(value) => PrimitiveValue::Int64(self.read_i64(offset, value)?),
            PrimitiveValue::UInt8(value) => PrimitiveValue::UInt8(self.read_u8(offset, value)?),
            PrimitiveValue::UInt16(value) => PrimitiveValue::UInt16(self.read_u16(offset, value)?),
            PrimitiveValue::UInt32(value) => PrimitiveValue::UInt32(self.read_u32(offset, value)?),
            PrimitiveValue::UInt64(value) => PrimitiveValue::UInt64(self.read_u64(offset, value)?),
            PrimitiveValue::Float32(value) => {
                PrimitiveValue::Float32(self.read_f32(offset, value)?)
            }
            PrimitiveValue::Float64(value) => {
                PrimitiveValue::Float64(self.read_f64(offset, value)?)
            }
            PrimitiveValue::Enum(value) => PrimitiveValue::Enum(self.read_u16(offset, value)?),
        })
    }

    fn wire_bytes<const N: usize>(self, offset: u32) -> Result<[u8; N], PrimitiveError> {
        let offset = usize::try_from(offset).map_err(|_| PrimitiveError::OffsetOverflow)?;
        let start = offset
            .checked_mul(N)
            .ok_or(PrimitiveError::OffsetOverflow)?;
        let end = start.checked_add(N).ok_or(PrimitiveError::OffsetOverflow)?;
        let mut result = [0; N];
        if let Some(bytes) = self.bytes.get(start..end) {
            result.copy_from_slice(bytes);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TinyEnum {
        Zero,
        One,
    }

    impl TryFrom<u16> for TinyEnum {
        type Error = ();

        fn try_from(value: u16) -> Result<Self, Self::Error> {
            match value {
                0 => Ok(Self::Zero),
                1 => Ok(Self::One),
                _ => Err(()),
            }
        }
    }

    #[test]
    fn defaults_are_xored_and_missing_fields_read_as_the_schema_default() {
        let bytes = [0x01, 0xff, 0x34, 0x12, 0, 0, 0, 0];
        let data = DataSection::new(&bytes).expect("one word is aligned");
        assert_eq!(data.read_bool(0, true), Ok(false));
        assert_eq!(data.read_bool(1, true), Ok(true));
        assert_eq!(data.read_u8(1, 0x0f), Ok(0xf0));
        assert_eq!(data.read_u16(1, 0x00ff), Ok(0x12cb));
        assert_eq!(data.read_u64(1, 123_456), Ok(123_456));
        assert_eq!(
            data.read_f32(2, -0.0).map(f32::to_bits),
            Ok((-0.0f32).to_bits())
        );
    }

    #[test]
    fn unknown_enum_ordinals_are_preserved() {
        let bytes = [7, 0, 0, 0, 0, 0, 0, 0];
        let data = DataSection::new(&bytes).expect("one word is aligned");
        assert_eq!(data.read_enum::<TinyEnum>(0, 0), Ok(EnumValue::Unknown(7)));
        assert_eq!(
            data.read_enum::<TinyEnum>(0, 6),
            Ok(EnumValue::Known(TinyEnum::One))
        );
    }

    #[test]
    fn bool_offsets_use_least_significant_bit_first_across_bytes() {
        let bytes = [0b1000_0001, 0b0000_0001, 0, 0, 0, 0, 0, 0];
        let data = DataSection::new(&bytes).expect("one word is aligned");
        assert_eq!(data.read_bool(0, false), Ok(true));
        assert_eq!(data.read_bool(1, false), Ok(false));
        assert_eq!(data.read_bool(7, false), Ok(true));
        assert_eq!(data.read_bool(8, false), Ok(true));
        assert_eq!(data.read_bool(9, true), Ok(true));
    }

    #[test]
    fn randomized_u64_default_xor_is_exact() {
        let mut wire = 0x1234_5678_9abc_def0u64;
        let mut default = 0xfedc_ba98_7654_3210u64;
        for _ in 0..10_000 {
            wire ^= wire << 13;
            wire ^= wire >> 7;
            wire ^= wire << 17;
            default = default.rotate_left(11) ^ wire;
            let bytes = wire.to_le_bytes();
            let data = DataSection::new(&bytes).expect("one word is aligned");
            assert_eq!(data.read_u64(0, default), Ok(wire ^ default));
        }
    }

    #[test]
    fn dynamic_reads_cover_every_primitive_kind() {
        let bytes = [0xff; 8];
        let data = DataSection::new(&bytes).expect("one word is aligned");
        let cases = [
            (PrimitiveType::Void, PrimitiveValue::Void),
            (PrimitiveType::Bool, PrimitiveValue::Bool(false)),
            (PrimitiveType::Int8, PrimitiveValue::Int8(0)),
            (PrimitiveType::Int16, PrimitiveValue::Int16(0)),
            (PrimitiveType::Int32, PrimitiveValue::Int32(0)),
            (PrimitiveType::Int64, PrimitiveValue::Int64(0)),
            (PrimitiveType::UInt8, PrimitiveValue::UInt8(0)),
            (PrimitiveType::UInt16, PrimitiveValue::UInt16(0)),
            (PrimitiveType::UInt32, PrimitiveValue::UInt32(0)),
            (PrimitiveType::UInt64, PrimitiveValue::UInt64(0)),
            (PrimitiveType::Float32, PrimitiveValue::Float32(0.0)),
            (PrimitiveType::Float64, PrimitiveValue::Float64(0.0)),
            (PrimitiveType::Enum, PrimitiveValue::Enum(0)),
        ];
        for (primitive_type, default) in cases {
            assert!(data.read_dynamic(primitive_type, 0, default).is_ok());
        }
        assert!(matches!(
            data.read_dynamic(PrimitiveType::UInt32, 0, PrimitiveValue::Int32(0)),
            Err(PrimitiveError::DefaultTypeMismatch { .. })
        ));
    }
}
