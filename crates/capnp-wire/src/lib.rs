#![no_std]
#![doc = "Low-level, allocation-free Cap'n Proto wire primitives."]
//!
//! This crate follows the wire representation in `capnp/endian.h` and the
//! `WirePointer` definition in `capnp/layout.c++` from the pinned C++ oracle
//! commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. Cap'n Proto scalar values
//! are little-endian and a wire word is exactly eight bytes.
//!
//! All access is byte-oriented and checked. This deliberately avoids aligned
//! native loads, unchecked pointer arithmetic, allocation, message framing,
//! pointer traversal, and validation of pointed-to objects; those belong to
//! later milestones.

mod endian;
mod integer;
mod pointer;
mod segment;

pub use endian::{
    Word, WordIter, WordIterMut, WordSlice, WordSliceMut, WordSlot, read_f32_le, read_f64_le,
    read_i8, read_i16_le, read_i32_le, read_i64_le, read_u8, read_u16_le, read_u32_le, read_u64_le,
    write_f32_le, write_f64_le, write_i8, write_i16_le, write_i32_le, write_i64_le, write_u8,
    write_u16_le, write_u32_le, write_u64_le,
};
pub use integer::{WireError, checked_add_signed, checked_range, checked_word_range};
pub use pointer::{
    ElementSize, FarPointerFields, InlineCompositeTagFields, ListPointerFields, PointerKind,
    StructPointerFields, WirePointer,
};
pub use segment::Segment;

/// Cap'n Proto's fundamental wire-format unit is a 64-bit word.
pub const WORD_BYTES: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_word_is_eight_bytes() {
        assert_eq!(WORD_BYTES, size_of::<u64>());
        assert_eq!(size_of::<Word>(), WORD_BYTES);
        assert_eq!(align_of::<Word>(), 1);
    }
}
