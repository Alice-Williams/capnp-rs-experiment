//! Zero-copy byte, Text, and Data views over validated byte-list targets.
//!
//! Compatibility follows the pinned C++ `readTextPointer()` and
//! `readDataPointer()` behavior. Text must contain at least its trailing NUL;
//! the terminator is checked and omitted from the returned payload. UTF-8 is
//! validated only when requested, matching the current Rust API. Interior NUL
//! bytes remain part of the length-delimited payload.

use core::fmt;

use capnp_wire::ElementSize;

use crate::{
    MessageSegments, NestingLimit, ResolvedPointer, TraversalBudget, TraversalError, WireLocation,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BlobError {
    Traversal(TraversalError),
    ExpectedListPointer,
    ExpectedByteList {
        actual: ElementSize,
    },
    UnknownSegment {
        segment_id: u32,
    },
    RangeOverflow,
    OutOfBounds {
        location: WireLocation,
        bytes: u32,
        segment_bytes: usize,
    },
    TextMissingNul,
}

impl fmt::Display for BlobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for BlobError {}

impl From<TraversalError> for BlobError {
    fn from(value: TraversalError) -> Self {
        Self::Traversal(value)
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct DataReader<'a>(&'a [u8]);

impl<'a> DataReader<'a> {
    pub const fn empty() -> Self {
        Self(&[])
    }

    pub const fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    pub const fn len(self) -> usize {
        self.0.len()
    }

    pub const fn is_empty(self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for DataReader<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct TextReader<'a> {
    with_nul: &'a [u8],
}

impl<'a> TextReader<'a> {
    pub const fn empty() -> Self {
        Self { with_nul: b"\0" }
    }

    /// Creates a borrowed Text view after checking the mandatory terminator.
    ///
    /// ```
    /// use capnp_message::TextReader;
    /// let text = TextReader::from_bytes_with_nul(b"hello\0")?;
    /// assert_eq!(text.to_str()?, "hello");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_bytes_with_nul(bytes: &'a [u8]) -> Result<Self, BlobError> {
        if bytes.last() != Some(&0) {
            return Err(BlobError::TextMissingNul);
        }
        Ok(Self { with_nul: bytes })
    }

    pub const fn as_bytes(self) -> &'a [u8] {
        let len_without_nul = self.with_nul.len() - 1;
        self.with_nul.split_at(len_without_nul).0
    }

    pub const fn as_bytes_with_nul(self) -> &'a [u8] {
        self.with_nul
    }

    pub const fn len(self) -> usize {
        self.with_nul.len() - 1
    }

    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn to_str(self) -> Result<&'a str, core::str::Utf8Error> {
        core::str::from_utf8(self.as_bytes())
    }
}

impl fmt::Debug for TextReader<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.to_str() {
            Ok(text) => text.fmt(formatter),
            Err(_) => formatter
                .debug_tuple("invalid UTF-8")
                .field(&self.as_bytes())
                .finish(),
        }
    }
}

impl<'a> MessageSegments<'a> {
    /// Reads a Data pointer only after validation and an exact traversal charge.
    pub fn read_data<B: TraversalBudget>(
        &self,
        location: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<DataReader<'a>, BlobError> {
        match self.byte_list(location, budget, nesting)? {
            Some(bytes) => Ok(DataReader(bytes)),
            None => Ok(DataReader::empty()),
        }
    }

    /// Reads a Text pointer only after validation and an exact traversal charge.
    pub fn read_text<B: TraversalBudget>(
        &self,
        location: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<TextReader<'a>, BlobError> {
        match self.byte_list(location, budget, nesting)? {
            Some(bytes) => TextReader::from_bytes_with_nul(bytes),
            None => Ok(TextReader::empty()),
        }
    }

    fn byte_list<B: TraversalBudget>(
        &self,
        location: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<Option<&'a [u8]>, BlobError> {
        let bounded = self.validate_pointer_with_limits(location, budget, nesting)?;
        let list = match bounded.pointer {
            ResolvedPointer::Null => return Ok(None),
            ResolvedPointer::List(list) => list,
            ResolvedPointer::Struct(_) | ResolvedPointer::Capability(_) => {
                return Err(BlobError::ExpectedListPointer);
            }
        };
        if list.element_size != ElementSize::Byte {
            return Err(BlobError::ExpectedByteList {
                actual: list.element_size,
            });
        }
        let segment = self
            .segment(list.content.segment_id)
            .ok_or(BlobError::UnknownSegment {
                segment_id: list.content.segment_id,
            })?;
        let start = usize::try_from(list.content.word_offset)
            .map_err(|_| BlobError::RangeOverflow)?
            .checked_mul(8)
            .ok_or(BlobError::RangeOverflow)?;
        let bytes = usize::try_from(list.element_count).map_err(|_| BlobError::RangeOverflow)?;
        let end = start.checked_add(bytes).ok_or(BlobError::RangeOverflow)?;
        segment
            .get(start..end)
            .map(Some)
            .ok_or(BlobError::OutOfBounds {
                location: list.content,
                bytes: list.element_count,
                segment_bytes: segment.len(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BudgetExhausted, LocalTraversalBudget};
    use alloc::vec;
    use capnp_wire::WirePointer;

    #[test]
    fn text_requires_only_the_final_nul_and_defers_utf8_validation() {
        let interior = TextReader::from_bytes_with_nul(b"a\0b\0").expect("final NUL is present");
        assert_eq!(interior.as_bytes(), b"a\0b");
        assert_eq!(interior.to_str(), Ok("a\0b"));

        let invalid = TextReader::from_bytes_with_nul(&[0xff, 0]).expect("final NUL is present");
        assert!(invalid.to_str().is_err());
        assert_eq!(invalid.as_bytes(), &[0xff]);

        assert_eq!(
            TextReader::from_bytes_with_nul(b"not terminated"),
            Err(BlobError::TextMissingNul)
        );
        assert_eq!(
            TextReader::from_bytes_with_nul(b""),
            Err(BlobError::TextMissingNul)
        );
    }

    #[test]
    fn null_blob_pointers_read_as_empty_without_a_charge() {
        let bytes = [0u8; 8];
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(0);
        let location = WireLocation {
            segment_id: 0,
            word_offset: 0,
        };
        assert_eq!(
            segments
                .read_text(location, &budget, NestingLimit::new(0))
                .expect("null Text is empty")
                .as_bytes(),
            b""
        );
        assert_eq!(
            segments
                .read_data(location, &budget, NestingLimit::new(0))
                .expect("null Data is empty")
                .as_bytes(),
            b""
        );
    }

    #[test]
    fn byte_list_view_is_never_returned_when_its_charge_fails() {
        let mut bytes = vec![0u8; 24];
        WirePointer::new_list(0, ElementSize::Byte, 9)
            .expect("byte list pointer fits")
            .write_to(&mut bytes, 0)
            .expect("pointer word fits");
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(1);
        assert_eq!(
            segments.read_data(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(1),
            ),
            Err(BlobError::Traversal(TraversalError::Budget(
                BudgetExhausted {
                    requested_words: 2,
                    remaining_words: 1,
                }
            )))
        );
    }

    #[test]
    fn message_text_rejects_a_byte_list_without_its_terminal_nul() {
        let mut bytes = vec![0u8; 16];
        WirePointer::new_list(0, ElementSize::Byte, 3)
            .expect("byte list pointer fits")
            .write_to(&mut bytes, 0)
            .expect("pointer word fits");
        bytes[8..11].copy_from_slice(b"abc");
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(1);
        assert_eq!(
            segments.read_text(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(1),
            ),
            Err(BlobError::TextMissingNul)
        );
    }
}
