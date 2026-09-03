use crate::WORD_BYTES;

/// One immutable, word-aligned message segment.
///
/// The private representation makes word alignment a reusable type invariant:
/// framing validates it once, and message readers can then borrow descriptor
/// tables without rescanning or copying them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Segment<'a> {
    bytes: &'a [u8],
}

impl<'a> Segment<'a> {
    pub const EMPTY: Self = Self { bytes: &[] };

    #[inline]
    pub const fn from_bytes(bytes: &'a [u8]) -> Option<Self> {
        if bytes.len() % WORD_BYTES == 0 {
            Some(Self { bytes })
        } else {
            None
        }
    }

    #[inline]
    pub const fn word_count(self) -> u32 {
        (self.bytes.len() / WORD_BYTES) as u32
    }

    #[inline]
    pub const fn bytes(self) -> &'a [u8] {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_complete_wire_words_become_segments() {
        assert!(Segment::from_bytes(&[0; 7]).is_none());
        let segment = Segment::from_bytes(&[0; 16]).expect("two complete words");
        assert_eq!(segment.word_count(), 2);
        assert_eq!(segment.bytes().len(), 16);
    }
}
