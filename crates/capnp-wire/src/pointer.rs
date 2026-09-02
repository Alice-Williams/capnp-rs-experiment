use crate::{WireError, Word};

const SIGNED_30_MIN: i32 = -(1 << 29);
const SIGNED_30_MAX: i32 = (1 << 29) - 1;
const UNSIGNED_29_MAX: u32 = (1 << 29) - 1;
const UNSIGNED_30_MAX: u32 = (1 << 30) - 1;

/// The two-bit discriminator common to all pointer words.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum PointerKind {
    Struct = 0,
    List = 1,
    Far = 2,
    Other = 3,
}

impl PointerKind {
    #[inline]
    const fn from_bits(bits: u32) -> Self {
        match bits & 3 {
            0 => Self::Struct,
            1 => Self::List,
            2 => Self::Far,
            _ => Self::Other,
        }
    }
}

/// The three-bit element-size code in a list pointer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u8)]
pub enum ElementSize {
    Void = 0,
    Bit = 1,
    Byte = 2,
    TwoBytes = 3,
    FourBytes = 4,
    EightBytes = 5,
    Pointer = 6,
    InlineComposite = 7,
}

impl ElementSize {
    pub const ALL: [Self; 8] = [
        Self::Void,
        Self::Bit,
        Self::Byte,
        Self::TwoBytes,
        Self::FourBytes,
        Self::EightBytes,
        Self::Pointer,
        Self::InlineComposite,
    ];

    #[inline]
    const fn from_bits(bits: u32) -> Self {
        match bits & 7 {
            0 => Self::Void,
            1 => Self::Bit,
            2 => Self::Byte,
            3 => Self::TwoBytes,
            4 => Self::FourBytes,
            5 => Self::EightBytes,
            6 => Self::Pointer,
            _ => Self::InlineComposite,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StructPointerFields {
    pub offset: i32,
    pub data_words: u16,
    pub pointer_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InlineCompositeTagFields {
    pub element_count: u32,
    pub data_words: u16,
    pub pointer_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListPointerFields {
    pub offset: i32,
    pub element_size: ElementSize,
    /// Element count, except for `InlineComposite`, where this is a word count.
    pub count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FarPointerFields {
    pub double_far: bool,
    pub landing_pad_word: u32,
    pub segment_id: u32,
}

/// One uninterpreted Cap'n Proto pointer word.
///
/// This type knows field widths and discriminators, but does not follow offsets,
/// decide whether a struct-kind word is a pointer or inline-composite tag, or
/// validate any target object.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
#[repr(transparent)]
pub struct WirePointer(Word);

impl WirePointer {
    pub const NULL: Self = Self(Word::ZERO);

    #[inline]
    pub const fn from_le_bytes(bytes: [u8; 8]) -> Self {
        Self(Word::from_le_bytes(bytes))
    }

    #[inline]
    pub const fn to_le_bytes(self) -> [u8; 8] {
        self.0.to_le_bytes()
    }

    #[inline]
    pub const fn from_word(word: Word) -> Self {
        Self(word)
    }

    #[inline]
    pub const fn into_word(self) -> Word {
        self.0
    }

    #[inline]
    pub fn read_from(bytes: &[u8], offset: usize) -> Result<Self, WireError> {
        Ok(Self(Word::read_from(bytes, offset)?))
    }

    #[inline]
    pub fn write_to(self, bytes: &mut [u8], offset: usize) -> Result<(), WireError> {
        self.0.write_to(bytes, offset)
    }

    #[inline]
    pub const fn raw(self) -> u64 {
        self.0.get()
    }

    #[inline]
    pub const fn lower32(self) -> u32 {
        self.raw() as u32
    }

    #[inline]
    pub const fn upper32(self) -> u32 {
        (self.raw() >> 32) as u32
    }

    #[inline]
    pub const fn kind(self) -> PointerKind {
        PointerKind::from_bits(self.lower32())
    }

    #[inline]
    pub const fn is_null(self) -> bool {
        self.raw() == 0
    }

    #[inline]
    pub const fn is_positional(self) -> bool {
        matches!(self.kind(), PointerKind::Struct | PointerKind::List)
    }

    /// An `OTHER` pointer is a capability only when all its upper offset bits are zero.
    #[inline]
    pub const fn is_capability(self) -> bool {
        self.lower32() == PointerKind::Other as u32
    }

    #[inline]
    pub const fn positional_offset(self) -> Option<i32> {
        if self.is_positional() {
            Some((self.lower32() as i32) >> 2)
        } else {
            None
        }
    }

    #[inline]
    pub const fn struct_fields(self) -> Option<StructPointerFields> {
        if matches!(self.kind(), PointerKind::Struct) {
            Some(StructPointerFields {
                offset: (self.lower32() as i32) >> 2,
                data_words: self.upper32() as u16,
                pointer_count: (self.upper32() >> 16) as u16,
            })
        } else {
            None
        }
    }

    #[inline]
    pub const fn inline_composite_tag_fields(self) -> Option<InlineCompositeTagFields> {
        if matches!(self.kind(), PointerKind::Struct) {
            Some(InlineCompositeTagFields {
                element_count: self.lower32() >> 2,
                data_words: self.upper32() as u16,
                pointer_count: (self.upper32() >> 16) as u16,
            })
        } else {
            None
        }
    }

    #[inline]
    pub const fn list_fields(self) -> Option<ListPointerFields> {
        if matches!(self.kind(), PointerKind::List) {
            Some(ListPointerFields {
                offset: (self.lower32() as i32) >> 2,
                element_size: ElementSize::from_bits(self.upper32()),
                count: self.upper32() >> 3,
            })
        } else {
            None
        }
    }

    #[inline]
    pub const fn far_fields(self) -> Option<FarPointerFields> {
        if matches!(self.kind(), PointerKind::Far) {
            Some(FarPointerFields {
                double_far: ((self.lower32() >> 2) & 1) != 0,
                landing_pad_word: self.lower32() >> 3,
                segment_id: self.upper32(),
            })
        } else {
            None
        }
    }

    #[inline]
    pub const fn capability_index(self) -> Option<u32> {
        if self.is_capability() {
            Some(self.upper32())
        } else {
            None
        }
    }

    #[inline]
    pub fn new_struct(offset: i32, data_words: u16, pointer_count: u16) -> Result<Self, WireError> {
        let lower = positional_lower(PointerKind::Struct, offset)?;
        let upper = u32::from(data_words) | (u32::from(pointer_count) << 16);
        Ok(Self::from_parts(lower, upper))
    }

    /// Encodes the non-null representation used for an empty struct.
    #[inline]
    pub fn empty_struct() -> Self {
        Self::new_struct(-1, 0, 0).expect("-1 is a valid signed 30-bit offset")
    }

    /// Encodes a list pointer. For `InlineComposite`, `count` is the content word count.
    #[inline]
    pub fn new_list(offset: i32, element_size: ElementSize, count: u32) -> Result<Self, WireError> {
        require_unsigned("list count", count, UNSIGNED_29_MAX)?;
        let lower = positional_lower(PointerKind::List, offset)?;
        let upper = (count << 3) | element_size as u32;
        Ok(Self::from_parts(lower, upper))
    }

    #[inline]
    pub fn new_inline_composite_tag(
        element_count: u32,
        data_words: u16,
        pointer_count: u16,
    ) -> Result<Self, WireError> {
        require_unsigned(
            "inline-composite element count",
            element_count,
            UNSIGNED_30_MAX,
        )?;
        let lower = element_count << 2;
        let upper = u32::from(data_words) | (u32::from(pointer_count) << 16);
        Ok(Self::from_parts(lower, upper))
    }

    #[inline]
    pub fn new_far(
        double_far: bool,
        landing_pad_word: u32,
        segment_id: u32,
    ) -> Result<Self, WireError> {
        require_unsigned("far landing-pad word", landing_pad_word, UNSIGNED_29_MAX)?;
        let lower =
            (landing_pad_word << 3) | (u32::from(double_far) << 2) | PointerKind::Far as u32;
        Ok(Self::from_parts(lower, segment_id))
    }

    #[inline]
    pub const fn new_capability(index: u32) -> Self {
        Self::from_parts(PointerKind::Other as u32, index)
    }

    #[inline]
    const fn from_parts(lower: u32, upper: u32) -> Self {
        Self(Word::from_u64((lower as u64) | ((upper as u64) << 32)))
    }
}

#[inline]
fn positional_lower(kind: PointerKind, offset: i32) -> Result<u32, WireError> {
    if !(SIGNED_30_MIN..=SIGNED_30_MAX).contains(&offset) {
        return Err(WireError::ValueOutOfRange {
            field: "positional offset",
            value: i64::from(offset),
            min: i64::from(SIGNED_30_MIN),
            max: i64::from(SIGNED_30_MAX),
        });
    }
    Ok(((offset as u32) << 2) | kind as u32)
}

#[inline]
fn require_unsigned(field: &'static str, value: u32, max: u32) -> Result<(), WireError> {
    if value > max {
        Err(WireError::ValueOutOfRange {
            field,
            value: i64::from(value),
            min: 0,
            max: i64::from(max),
        })
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_cpp_fixture_root_pointer_decodes() {
        // Bytes 8..16 of M01's pinned C++ `wire-unpacked.bin`: zero offset,
        // nine data words, and 28 pointer words.
        let pointer = WirePointer::from_le_bytes([0x00, 0x00, 0x00, 0x00, 0x09, 0x00, 0x1c, 0x00]);
        assert_eq!(
            pointer.struct_fields(),
            Some(StructPointerFields {
                offset: 0,
                data_words: 9,
                pointer_count: 28,
            })
        );
        assert!(!pointer.is_null());
    }

    #[test]
    fn signed_30_bit_edges_round_trip() {
        for offset in [SIGNED_30_MIN, -1, 0, 1, SIGNED_30_MAX] {
            let pointer = WirePointer::new_struct(offset, u16::MAX, u16::MAX)
                .expect("edge offset is representable");
            assert_eq!(pointer.positional_offset(), Some(offset));
            assert_eq!(
                pointer.struct_fields(),
                Some(StructPointerFields {
                    offset,
                    data_words: u16::MAX,
                    pointer_count: u16::MAX,
                })
            );
        }

        assert!(matches!(
            WirePointer::new_struct(SIGNED_30_MIN - 1, 0, 0),
            Err(WireError::ValueOutOfRange { .. })
        ));
        assert!(matches!(
            WirePointer::new_struct(SIGNED_30_MAX + 1, 0, 0),
            Err(WireError::ValueOutOfRange { .. })
        ));
    }

    #[test]
    fn all_list_element_sizes_and_counts_round_trip() {
        for element_size in ElementSize::ALL {
            for count in [0, 1, 7, UNSIGNED_29_MAX] {
                let pointer = WirePointer::new_list(-17, element_size, count)
                    .expect("list fields are representable");
                assert_eq!(
                    pointer.list_fields(),
                    Some(ListPointerFields {
                        offset: -17,
                        element_size,
                        count,
                    })
                );
            }
        }
        assert!(matches!(
            WirePointer::new_list(0, ElementSize::Byte, UNSIGNED_29_MAX + 1),
            Err(WireError::ValueOutOfRange { .. })
        ));
    }

    #[test]
    fn inline_composite_tag_uses_unsigned_30_bit_count() {
        for element_count in [0, 1, UNSIGNED_30_MAX] {
            let tag = WirePointer::new_inline_composite_tag(element_count, 3, 4)
                .expect("tag fields are representable");
            assert_eq!(
                tag.inline_composite_tag_fields(),
                Some(InlineCompositeTagFields {
                    element_count,
                    data_words: 3,
                    pointer_count: 4,
                })
            );
        }
        assert!(matches!(
            WirePointer::new_inline_composite_tag(UNSIGNED_30_MAX + 1, 0, 0),
            Err(WireError::ValueOutOfRange { .. })
        ));
    }

    #[test]
    fn far_and_capability_fields_round_trip() {
        for double_far in [false, true] {
            for landing_pad_word in [0, 1, UNSIGNED_29_MAX] {
                let pointer = WirePointer::new_far(double_far, landing_pad_word, 0x89ab_cdef)
                    .expect("far fields are representable");
                assert_eq!(
                    pointer.far_fields(),
                    Some(FarPointerFields {
                        double_far,
                        landing_pad_word,
                        segment_id: 0x89ab_cdef,
                    })
                );
            }
        }
        assert!(matches!(
            WirePointer::new_far(false, UNSIGNED_29_MAX + 1, 0),
            Err(WireError::ValueOutOfRange { .. })
        ));

        let capability = WirePointer::new_capability(u32::MAX);
        assert!(capability.is_capability());
        assert_eq!(capability.capability_index(), Some(u32::MAX));

        let reserved_other = WirePointer::from_le_bytes([7, 0, 0, 0, 1, 0, 0, 0]);
        assert_eq!(reserved_other.kind(), PointerKind::Other);
        assert!(!reserved_other.is_capability());
        assert_eq!(reserved_other.capability_index(), None);
    }

    #[test]
    fn exact_pointer_bytes_match_the_oracle_layout() {
        assert_eq!(
            WirePointer::new_struct(0, 1, 2)
                .expect("struct fields fit")
                .to_le_bytes(),
            [0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x02, 0x00]
        );
        assert_eq!(
            WirePointer::empty_struct().to_le_bytes(),
            [0xfc, 0xff, 0xff, 0xff, 0, 0, 0, 0]
        );
        assert_eq!(
            WirePointer::new_list(2, ElementSize::FourBytes, 3)
                .expect("list fields fit")
                .to_le_bytes(),
            [0x09, 0, 0, 0, 0x1c, 0, 0, 0]
        );
        assert_eq!(
            WirePointer::new_far(true, 0x123, 0x4567_89ab)
                .expect("far fields fit")
                .to_le_bytes(),
            [0x1e, 0x09, 0, 0, 0xab, 0x89, 0x67, 0x45]
        );
        assert_eq!(
            WirePointer::new_capability(0x1234_5678).to_le_bytes(),
            [0x03, 0, 0, 0, 0x78, 0x56, 0x34, 0x12]
        );
    }

    #[test]
    fn pointer_word_uses_the_checked_unaligned_byte_path() {
        let pointer = WirePointer::new_list(-2, ElementSize::Pointer, 42)
            .expect("list fields are representable");
        let mut bytes = [0u8; 10];
        pointer.write_to(&mut bytes, 1).expect("pointer fits");
        assert_eq!(
            WirePointer::read_from(&bytes, 1).expect("pointer is present"),
            pointer
        );
    }
}
