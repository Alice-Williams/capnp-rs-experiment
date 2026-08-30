use crate::{WORD_BYTES, WireError, checked_range};

/// One on-wire word stored as bytes, with no native alignment requirement.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct Word([u8; WORD_BYTES]);

impl Word {
    pub const ZERO: Self = Self([0; WORD_BYTES]);

    pub const fn from_le_bytes(bytes: [u8; WORD_BYTES]) -> Self {
        Self(bytes)
    }

    pub const fn to_le_bytes(self) -> [u8; WORD_BYTES] {
        self.0
    }

    pub const fn get(self) -> u64 {
        u64::from_le_bytes(self.0)
    }

    pub const fn from_u64(value: u64) -> Self {
        Self(value.to_le_bytes())
    }

    pub fn read_from(bytes: &[u8], offset: usize) -> Result<Self, WireError> {
        Ok(Self(read_array(bytes, offset)?))
    }

    pub fn write_to(self, bytes: &mut [u8], offset: usize) -> Result<(), WireError> {
        write_array(bytes, offset, self.0)
    }
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], WireError> {
    let range = checked_range(offset, N, bytes.len())?;
    let mut value = [0; N];
    value.copy_from_slice(&bytes[range]);
    Ok(value)
}

fn write_array<const N: usize>(
    bytes: &mut [u8],
    offset: usize,
    value: [u8; N],
) -> Result<(), WireError> {
    let range = checked_range(offset, N, bytes.len())?;
    bytes[range].copy_from_slice(&value);
    Ok(())
}

macro_rules! integer_accessors {
    ($read:ident, $write:ident, $ty:ty, $size:expr) => {
        pub fn $read(bytes: &[u8], offset: usize) -> Result<$ty, WireError> {
            Ok(<$ty>::from_le_bytes(read_array::<$size>(bytes, offset)?))
        }

        pub fn $write(bytes: &mut [u8], offset: usize, value: $ty) -> Result<(), WireError> {
            write_array(bytes, offset, value.to_le_bytes())
        }
    };
}

integer_accessors!(read_u16_le, write_u16_le, u16, 2);
integer_accessors!(read_u32_le, write_u32_le, u32, 4);
integer_accessors!(read_u64_le, write_u64_le, u64, 8);
integer_accessors!(read_i16_le, write_i16_le, i16, 2);
integer_accessors!(read_i32_le, write_i32_le, i32, 4);
integer_accessors!(read_i64_le, write_i64_le, i64, 8);

pub fn read_u8(bytes: &[u8], offset: usize) -> Result<u8, WireError> {
    Ok(read_array::<1>(bytes, offset)?[0])
}

pub fn write_u8(bytes: &mut [u8], offset: usize, value: u8) -> Result<(), WireError> {
    write_array(bytes, offset, [value])
}

pub fn read_i8(bytes: &[u8], offset: usize) -> Result<i8, WireError> {
    Ok(i8::from_le_bytes(read_array(bytes, offset)?))
}

pub fn write_i8(bytes: &mut [u8], offset: usize, value: i8) -> Result<(), WireError> {
    write_array(bytes, offset, value.to_le_bytes())
}

pub fn read_f32_le(bytes: &[u8], offset: usize) -> Result<f32, WireError> {
    Ok(f32::from_bits(read_u32_le(bytes, offset)?))
}

pub fn write_f32_le(bytes: &mut [u8], offset: usize, value: f32) -> Result<(), WireError> {
    write_u32_le(bytes, offset, value.to_bits())
}

pub fn read_f64_le(bytes: &[u8], offset: usize) -> Result<f64, WireError> {
    Ok(f64::from_bits(read_u64_le(bytes, offset)?))
}

pub fn write_f64_le(bytes: &mut [u8], offset: usize, value: f64) -> Result<(), WireError> {
    write_u64_le(bytes, offset, value.to_bits())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_scalar_fixture_round_trips() {
        let mut bytes = [0u8; 31];
        write_u8(&mut bytes, 1, 0xa5).expect("u8 fixture fits");
        write_i8(&mut bytes, 2, -2).expect("i8 fixture fits");
        write_u16_le(&mut bytes, 3, 0x1234).expect("u16 fixture fits");
        write_i16_le(&mut bytes, 5, -0x1234).expect("i16 fixture fits");
        write_u32_le(&mut bytes, 7, 0x89ab_cdef).expect("u32 fixture fits");
        write_i32_le(&mut bytes, 11, -0x0123_4567).expect("i32 fixture fits");
        write_u64_le(&mut bytes, 15, 0x0123_4567_89ab_cdef).expect("u64 fixture fits");
        write_i64_le(&mut bytes, 23, -0x0123_4567_89ab_cdef).expect("i64 fixture fits");

        assert_eq!(read_u8(&bytes, 1), Ok(0xa5));
        assert_eq!(read_i8(&bytes, 2), Ok(-2));
        assert_eq!(&bytes[3..5], &[0x34, 0x12]);
        assert_eq!(read_u16_le(&bytes, 3), Ok(0x1234));
        assert_eq!(read_i16_le(&bytes, 5), Ok(-0x1234));
        assert_eq!(&bytes[7..11], &[0xef, 0xcd, 0xab, 0x89]);
        assert_eq!(read_u32_le(&bytes, 7), Ok(0x89ab_cdef));
        assert_eq!(read_i32_le(&bytes, 11), Ok(-0x0123_4567));
        assert_eq!(read_u64_le(&bytes, 15), Ok(0x0123_4567_89ab_cdef));
        assert_eq!(read_i64_le(&bytes, 23), Ok(-0x0123_4567_89ab_cdef));
    }

    #[test]
    fn nan_payloads_are_preserved_exactly() {
        let f32_bits = 0x7fc0_1234;
        let f64_bits = 0x7ff8_0000_0000_1234;
        let mut bytes = [0u8; 13];

        write_f32_le(&mut bytes, 1, f32::from_bits(f32_bits)).expect("f32 fixture fits");
        write_f64_le(&mut bytes, 5, f64::from_bits(f64_bits)).expect("f64 fixture fits");

        assert_eq!(
            read_f32_le(&bytes, 1)
                .expect("f32 fixture is present")
                .to_bits(),
            f32_bits
        );
        assert_eq!(
            read_f64_le(&bytes, 5)
                .expect("f64 fixture is present")
                .to_bits(),
            f64_bits
        );
    }

    #[test]
    fn byte_path_is_unaligned_and_host_endian_independent() {
        let value = 0x0123_4567_89ab_cdef;
        let mut unaligned = [0xff; 10];
        write_u64_le(&mut unaligned, 1, value).expect("unaligned fixture fits");

        let big_endian = value.to_be_bytes();
        for index in 0..8 {
            assert_eq!(unaligned[1 + index], big_endian[7 - index]);
        }
        assert_eq!(read_u64_le(&unaligned, 1), Ok(value));
        assert_eq!(
            Word::read_from(&unaligned, 1)
                .expect("unaligned word is present")
                .get(),
            value
        );
    }

    #[test]
    fn short_or_overflowing_access_is_rejected_before_indexing() {
        let mut bytes = [0u8; 7];
        assert!(matches!(
            read_u64_le(&bytes, 0),
            Err(WireError::OutOfBounds { .. })
        ));
        assert!(matches!(
            write_u32_le(&mut bytes, usize::MAX, 1),
            Err(WireError::ArithmeticOverflow)
        ));
    }
}
