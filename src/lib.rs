//! A fresh, conformance-first experiment in implementing Cap'n Proto in Rust.
//!
//! The implementation will grow only alongside reference-compatible fixtures
//! and explicit resource limits for untrusted input.

/// Cap'n Proto's fundamental wire-format unit is a 64-bit word.
pub const WORD_BYTES: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_word_is_eight_bytes() {
        assert_eq!(WORD_BYTES, size_of::<u64>());
    }
}
