#![no_std]
#![doc = "Low-level Cap'n Proto wire primitives."]

/// Cap'n Proto's fundamental wire-format unit is a 64-bit word.
pub const WORD_BYTES: usize = 8;

#[cfg(test)]
mod tests {
    use super::WORD_BYTES;

    #[test]
    fn wire_word_is_eight_bytes() {
        assert_eq!(WORD_BYTES, size_of::<u64>());
    }
}
