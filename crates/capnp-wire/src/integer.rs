use core::{fmt, ops::Range};

use crate::WORD_BYTES;

/// Failure produced before a wire slice is indexed or an offset is applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireError {
    /// An intermediate byte/word calculation did not fit in `usize`.
    ArithmeticOverflow,
    /// The requested byte range is not fully present in the input.
    OutOfBounds {
        offset: usize,
        len: usize,
        available: usize,
    },
    /// A signed relative offset moved before byte/word position zero.
    OffsetBeforeStart { base: usize, delta: i32 },
}

impl fmt::Display for WireError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::ArithmeticOverflow => formatter.write_str("wire arithmetic overflow"),
            Self::OutOfBounds {
                offset,
                len,
                available,
            } => write!(
                formatter,
                "wire range offset {offset} length {len} exceeds {available} available bytes"
            ),
            Self::OffsetBeforeStart { base, delta } => {
                write!(formatter, "wire offset {delta} moves before base {base}")
            }
        }
    }
}

impl core::error::Error for WireError {}

/// Returns a byte range only after proving both addition and bounds safety.
pub fn checked_range(
    offset: usize,
    len: usize,
    available: usize,
) -> Result<Range<usize>, WireError> {
    let end = offset
        .checked_add(len)
        .ok_or(WireError::ArithmeticOverflow)?;
    if end > available {
        Err(WireError::OutOfBounds {
            offset,
            len,
            available,
        })
    } else {
        Ok(offset..end)
    }
}

/// Converts a word range to bytes without permitting multiplication overflow.
pub fn checked_word_range(
    word_offset: usize,
    word_count: usize,
    available_bytes: usize,
) -> Result<Range<usize>, WireError> {
    let byte_offset = word_offset
        .checked_mul(WORD_BYTES)
        .ok_or(WireError::ArithmeticOverflow)?;
    let byte_len = word_count
        .checked_mul(WORD_BYTES)
        .ok_or(WireError::ArithmeticOverflow)?;
    checked_range(byte_offset, byte_len, available_bytes)
}

/// Applies a signed wire-relative displacement without lossy casts or wrapping.
pub fn checked_add_signed(base: usize, delta: i32) -> Result<usize, WireError> {
    if delta >= 0 {
        let magnitude = usize::try_from(delta).map_err(|_| WireError::ArithmeticOverflow)?;
        base.checked_add(magnitude)
            .ok_or(WireError::ArithmeticOverflow)
    } else {
        let magnitude =
            usize::try_from(delta.unsigned_abs()).map_err(|_| WireError::ArithmeticOverflow)?;
        base.checked_sub(magnitude)
            .ok_or(WireError::OffsetBeforeStart { base, delta })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_checks_every_arithmetic_boundary() {
        assert_eq!(checked_range(3, 4, 7), Ok(3..7));
        assert_eq!(
            checked_range(3, 5, 7),
            Err(WireError::OutOfBounds {
                offset: 3,
                len: 5,
                available: 7,
            })
        );
        assert_eq!(
            checked_range(usize::MAX, 1, usize::MAX),
            Err(WireError::ArithmeticOverflow)
        );
        assert_eq!(checked_word_range(2, 3, 40), Ok(16..40));
        assert_eq!(
            checked_word_range(usize::MAX, 1, usize::MAX),
            Err(WireError::ArithmeticOverflow)
        );
    }

    #[test]
    fn signed_addition_rejects_both_directions_of_overflow() {
        assert_eq!(checked_add_signed(9, -9), Ok(0));
        assert_eq!(checked_add_signed(9, 7), Ok(16));
        assert_eq!(
            checked_add_signed(8, -9),
            Err(WireError::OffsetBeforeStart { base: 8, delta: -9 })
        );
        assert_eq!(
            checked_add_signed(usize::MAX, 1),
            Err(WireError::ArithmeticOverflow)
        );
        assert_eq!(
            checked_add_signed(0, i32::MIN),
            Err(WireError::OffsetBeforeStart {
                base: 0,
                delta: i32::MIN,
            })
        );
    }
}
