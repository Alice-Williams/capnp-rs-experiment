//! Coordinate-based struct readers and schema-evolution semantics.
//!
//! This follows the pinned C++ `StructReader` field behavior: short data and
//! pointer sections read as zero, scalar defaults are handled by `DataSection`,
//! and groups are views over the parent's exact storage. Union discriminants
//! are retained as raw `u16` values so newer cases survive older readers.
//! Schema pointer defaults are represented as separate immutable messages and
//! are followed through the same validation, nesting, and traversal limits.
//!
//! Typed generated accessors, list upgrades, owned messages, capabilities, and
//! reflection remain later milestones.

use core::fmt;

use crate::{
    BlobError, BoundedPointer, DataReader, DataSection, MessageSegments, NestingLimit,
    PrimitiveError, ResolvedPointer, StructRef, TextReader, TraversalBudget, TraversalError,
    ValidationError, WireLocation,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StructReadError {
    Traversal(TraversalError),
    Blob(BlobError),
    Primitive(PrimitiveError),
    ExpectedStructPointer,
    UnknownSegment {
        segment_id: u32,
    },
    RangeOverflow,
    DataOutOfBounds {
        location: WireLocation,
        data_words: u16,
        segment_bytes: usize,
    },
}

impl fmt::Display for StructReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for StructReadError {}

impl From<TraversalError> for StructReadError {
    fn from(value: TraversalError) -> Self {
        Self::Traversal(value)
    }
}

impl From<ValidationError> for StructReadError {
    fn from(value: ValidationError) -> Self {
        Self::Traversal(TraversalError::Validation(value))
    }
}

impl From<BlobError> for StructReadError {
    fn from(value: BlobError) -> Self {
        Self::Blob(value)
    }
}

impl From<PrimitiveError> for StructReadError {
    fn from(value: PrimitiveError) -> Self {
        Self::Primitive(value)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PointerDefault<'context, 'data> {
    pub segments: &'context MessageSegments<'data>,
    pub location: WireLocation,
}

#[derive(Clone, Copy, Debug)]
pub struct ResolvedPointerField<'context, 'data> {
    pub source: &'context MessageSegments<'data>,
    pub value: BoundedPointer,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct UnionDiscriminant(pub u16);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnionValue<C> {
    Known(C),
    Unknown(UnionDiscriminant),
}

/// A short-lived view that stores coordinates and reader context, not native pointers.
#[derive(Debug)]
pub struct StructReader<'context, 'data, B> {
    segments: &'context MessageSegments<'data>,
    budget: &'context B,
    reference: Option<StructRef>,
    nesting: NestingLimit,
}

impl<'context, 'data, B> Clone for StructReader<'context, 'data, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'context, 'data, B> Copy for StructReader<'context, 'data, B> {}

impl<'data> MessageSegments<'data> {
    /// Opens the charged root struct reader at segment zero, word zero.
    ///
    /// ```
    /// use capnp_message::{LocalTraversalBudget, MessageSegments, NestingLimit};
    /// let null = [0u8; 8];
    /// let message = MessageSegments::new(&[&null])?;
    /// let budget = LocalTraversalBudget::new(0);
    /// let root = message.read_root_struct(&budget, NestingLimit::new(0))?;
    /// assert!(root.reference().is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline(always)]
    pub fn read_root_struct<'context, B: TraversalBudget>(
        &'context self,
        budget: &'context B,
        nesting: NestingLimit,
    ) -> Result<StructReader<'context, 'data, B>, StructReadError> {
        let bounded = self.validate_root_struct_pointer_with_limits(budget, nesting)?;
        self.struct_reader_from_bounded(bounded, budget)
    }

    /// Opens a charged struct reader at a pointer location.
    ///
    /// ```
    /// use capnp_message::{LocalTraversalBudget, MessageSegments, NestingLimit, WireLocation};
    /// let null = [0u8; 8];
    /// let message = MessageSegments::new(&[&null])?;
    /// let budget = LocalTraversalBudget::new(0);
    /// let root = message.read_struct(
    ///     WireLocation { segment_id: 0, word_offset: 0 },
    ///     &budget,
    ///     NestingLimit::new(0),
    /// )?;
    /// assert!(root.reference().is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline]
    pub fn read_struct<'context, B: TraversalBudget>(
        &'context self,
        location: WireLocation,
        budget: &'context B,
        nesting: NestingLimit,
    ) -> Result<StructReader<'context, 'data, B>, StructReadError> {
        let bounded = self.validate_struct_pointer_with_limits(location, budget, nesting)?;
        self.struct_reader_from_bounded(bounded, budget)
    }

    #[inline(always)]
    fn struct_reader_from_bounded<'context, B: TraversalBudget>(
        &'context self,
        bounded: BoundedPointer,
        budget: &'context B,
    ) -> Result<StructReader<'context, 'data, B>, StructReadError> {
        match bounded.pointer {
            ResolvedPointer::Null => Ok(StructReader {
                segments: self,
                budget,
                reference: None,
                nesting: bounded.child_nesting,
            }),
            ResolvedPointer::Struct(reference) => Ok(StructReader {
                segments: self,
                budget,
                reference: Some(reference),
                nesting: bounded.child_nesting,
            }),
            ResolvedPointer::List(_) | ResolvedPointer::Capability(_) => {
                Err(StructReadError::ExpectedStructPointer)
            }
        }
    }
}

impl<'context, 'data, B: TraversalBudget> StructReader<'context, 'data, B> {
    pub(crate) const fn from_prevalidated(
        segments: &'context MessageSegments<'data>,
        budget: &'context B,
        reference: Option<StructRef>,
        nesting: NestingLimit,
    ) -> Self {
        Self {
            segments,
            budget,
            reference,
            nesting,
        }
    }

    pub(crate) const fn empty_from_context(
        segments: &'context MessageSegments<'data>,
        budget: &'context B,
        nesting: NestingLimit,
    ) -> Self {
        Self {
            segments,
            budget,
            reference: None,
            nesting,
        }
    }

    pub(crate) const fn segments(self) -> &'context MessageSegments<'data> {
        self.segments
    }

    pub(crate) const fn budget(self) -> &'context B {
        self.budget
    }

    pub(crate) const fn nesting(self) -> NestingLimit {
        self.nesting
    }

    pub const fn nesting_limit(self) -> NestingLimit {
        self.nesting
    }

    pub const fn reference(self) -> Option<StructRef> {
        self.reference
    }

    /// Returns the number of pointer slots present in the encoded struct.
    #[inline(always)]
    pub const fn pointer_count(self) -> usize {
        match self.reference {
            Some(reference) => reference.pointer_count as usize,
            None => 0,
        }
    }

    /// Returns another schema view over the same bytes without charging or descending.
    pub const fn group(self) -> Self {
        self
    }

    #[inline(always)]
    pub const fn data_byte_len(self) -> usize {
        match self.reference {
            Some(reference) => reference.data_words as usize * 8,
            None => 0,
        }
    }

    /// Reads one data word directly from this struct, applying its schema default.
    #[inline(always)]
    pub fn read_u64(self, offset: u32, default: u64) -> Result<u64, StructReadError> {
        let Some(reference) = self.reference else {
            return Ok(default);
        };
        if offset >= u32::from(reference.data_words) {
            return Ok(default);
        }
        let word_offset = reference
            .content
            .word_offset
            .checked_add(offset)
            .ok_or(StructReadError::RangeOverflow)?;
        let segment = self.segments.segment(reference.content.segment_id).ok_or(
            StructReadError::UnknownSegment {
                segment_id: reference.content.segment_id,
            },
        )?;
        let start = usize::try_from(word_offset)
            .map_err(|_| StructReadError::RangeOverflow)?
            .checked_mul(8)
            .ok_or(StructReadError::RangeOverflow)?;
        let end = start.checked_add(8).ok_or(StructReadError::RangeOverflow)?;
        let bytes = segment
            .get(start..end)
            .ok_or(StructReadError::DataOutOfBounds {
                location: reference.content,
                data_words: reference.data_words,
                segment_bytes: segment.len(),
            })?;
        Ok(u64::from_le_bytes(
            bytes
                .try_into()
                .expect("a checked data-word range is exactly eight bytes"),
        ) ^ default)
    }

    #[inline(always)]
    pub fn data_section(self) -> Result<DataSection<'data>, StructReadError> {
        let Some(reference) = self.reference else {
            return Ok(DataSection::new(&[])?);
        };
        let segment = self.segments.segment(reference.content.segment_id).ok_or(
            StructReadError::UnknownSegment {
                segment_id: reference.content.segment_id,
            },
        )?;
        let start = usize::try_from(reference.content.word_offset)
            .map_err(|_| StructReadError::RangeOverflow)?
            .checked_mul(8)
            .ok_or(StructReadError::RangeOverflow)?;
        let bytes = usize::from(reference.data_words)
            .checked_mul(8)
            .ok_or(StructReadError::RangeOverflow)?;
        let end = start
            .checked_add(bytes)
            .ok_or(StructReadError::RangeOverflow)?;
        let data = segment
            .get(start..end)
            .ok_or(StructReadError::DataOutOfBounds {
                location: reference.content,
                data_words: reference.data_words,
                segment_bytes: segment.len(),
            })?;
        Ok(DataSection::from_validated_bytes(data))
    }

    #[inline(always)]
    pub fn pointer_location(self, index: u16) -> Result<Option<WireLocation>, StructReadError> {
        let Some(reference) = self.reference else {
            return Ok(None);
        };
        if index >= reference.pointer_count {
            return Ok(None);
        }
        let word_offset = reference
            .content
            .word_offset
            .checked_add(u32::from(reference.data_words))
            .and_then(|offset| offset.checked_add(u32::from(index)))
            .ok_or(StructReadError::RangeOverflow)?;
        Ok(Some(WireLocation {
            segment_id: reference.content.segment_id,
            word_offset,
        }))
    }

    pub fn union_discriminant(self, u16_offset: u32) -> Result<UnionDiscriminant, StructReadError> {
        Ok(UnionDiscriminant(
            self.data_section()?.read_u16(u16_offset, 0)?,
        ))
    }

    pub fn union_value<C: TryFrom<u16>>(
        self,
        u16_offset: u32,
    ) -> Result<UnionValue<C>, StructReadError> {
        let discriminant = self.union_discriminant(u16_offset)?;
        Ok(match C::try_from(discriminant.0) {
            Ok(value) => UnionValue::Known(value),
            Err(_) => UnionValue::Unknown(discriminant),
        })
    }

    pub fn resolve_pointer<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<ResolvedPointerField<'reader, 'data>, StructReadError>
    where
        'context: 'reader,
    {
        let selected = self.select_pointer(index, default)?;
        match selected {
            Some((segments, location)) => Ok(ResolvedPointerField {
                source: segments,
                value: segments.validate_pointer_with_limits(
                    location,
                    self.budget,
                    self.nesting,
                )?,
            }),
            None => Ok(ResolvedPointerField {
                source: self.segments,
                value: BoundedPointer {
                    pointer: ResolvedPointer::Null,
                    child_nesting: self.nesting,
                    charged_words: 0,
                },
            }),
        }
    }

    #[inline(always)]
    pub fn read_text<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<TextReader<'data>, StructReadError>
    where
        'context: 'reader,
    {
        if default.is_none() {
            return match self.pointer_location(index)? {
                Some(location) => self.read_text_at(location),
                None => Ok(TextReader::empty()),
            };
        }
        self.read_text_with_default(index, default)
    }

    #[inline(always)]
    fn read_text_at(&self, location: WireLocation) -> Result<TextReader<'data>, StructReadError> {
        Ok(self
            .segments
            .read_text(location, self.budget, self.nesting)?)
    }

    #[inline(never)]
    fn read_text_with_default<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<TextReader<'data>, StructReadError>
    where
        'context: 'reader,
    {
        match self.select_pointer(index, default)? {
            Some((segments, location)) => {
                Ok(segments.read_text(location, self.budget, self.nesting)?)
            }
            None => Ok(TextReader::empty()),
        }
    }

    #[inline(always)]
    pub fn read_data<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<DataReader<'data>, StructReadError>
    where
        'context: 'reader,
    {
        if default.is_none() {
            return match self.pointer_location(index)? {
                Some(location) => self.read_data_at(location),
                None => Ok(DataReader::empty()),
            };
        }
        self.read_data_with_default(index, default)
    }

    #[inline(always)]
    fn read_data_at(&self, location: WireLocation) -> Result<DataReader<'data>, StructReadError> {
        Ok(self
            .segments
            .read_data(location, self.budget, self.nesting)?)
    }

    #[inline(never)]
    fn read_data_with_default<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<DataReader<'data>, StructReadError>
    where
        'context: 'reader,
    {
        match self.select_pointer(index, default)? {
            Some((segments, location)) => {
                Ok(segments.read_data(location, self.budget, self.nesting)?)
            }
            None => Ok(DataReader::empty()),
        }
    }

    pub fn read_struct<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<StructReader<'reader, 'data, B>, StructReadError>
    where
        'context: 'reader,
    {
        match self.select_pointer(index, default)? {
            Some((segments, location)) => segments.read_struct(location, self.budget, self.nesting),
            None => Ok(StructReader {
                segments: self.segments,
                budget: self.budget,
                reference: None,
                nesting: self.nesting,
            }),
        }
    }

    pub(crate) fn select_pointer<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<Option<(&'reader MessageSegments<'data>, WireLocation)>, StructReadError>
    where
        'context: 'reader,
    {
        let Some(location) = self.pointer_location(index)? else {
            return Ok(default.map(|value| (value.segments, value.location)));
        };
        match self.segments.validate_pointer(location)? {
            ResolvedPointer::Null => Ok(default.map(|value| (value.segments, value.location))),
            ResolvedPointer::Struct(_)
            | ResolvedPointer::List(_)
            | ResolvedPointer::Capability(_) => Ok(Some((self.segments, location))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BudgetExhausted, LocalTraversalBudget};
    use alloc::vec;
    use capnp_wire::{ElementSize, WirePointer};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum KnownUnion {
        First,
        Second,
    }

    impl TryFrom<u16> for KnownUnion {
        type Error = ();

        fn try_from(value: u16) -> Result<Self, Self::Error> {
            match value {
                0 => Ok(Self::First),
                1 => Ok(Self::Second),
                _ => Err(()),
            }
        }
    }

    #[test]
    fn short_sections_default_and_groups_share_the_parent_view() {
        let bytes = [0u8; 8];
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(0);
        let reader = segments
            .read_struct(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(0),
            )
            .expect("null struct is empty");
        assert_eq!(
            reader.data_section().expect("empty data").read_u64(99, 7),
            Ok(7)
        );
        assert_eq!(reader.pointer_location(0), Ok(None));
        assert_eq!(reader.group().reference(), reader.reference());
        assert_eq!(reader.data_byte_len(), 0);
        assert_eq!(reader.pointer_count(), 0);
        assert_eq!(reader.read_u64(0, 7), Ok(7));
    }

    #[test]
    fn direct_data_word_read_preserves_defaults_and_bounds() {
        let mut bytes = [0u8; 16];
        WirePointer::new_struct(0, 1, 0)
            .expect("root shape fits")
            .write_to(&mut bytes, 0)
            .expect("root pointer fits");
        bytes[8..].copy_from_slice(&0x0123_4567_89ab_cdef_u64.to_le_bytes());
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(1);
        let reader = segments
            .read_root_struct(&budget, NestingLimit::new(1))
            .expect("root struct is valid");
        assert_eq!(reader.data_byte_len(), 8);
        assert_eq!(reader.pointer_count(), 0);
        assert_eq!(reader.read_u64(0, 0), Ok(0x0123_4567_89ab_cdef));
        assert_eq!(reader.read_u64(1, 7), Ok(7));
    }

    #[test]
    fn unknown_union_discriminants_survive_as_raw_values() {
        let mut bytes = vec![0u8; 16];
        WirePointer::new_struct(0, 1, 0)
            .expect("root pointer fits")
            .write_to(&mut bytes, 0)
            .expect("root word fits");
        bytes[8..10].copy_from_slice(&77u16.to_le_bytes());
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(1);
        let reader = segments
            .read_struct(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(2),
            )
            .expect("root struct validates");
        assert_eq!(reader.union_discriminant(0), Ok(UnionDiscriminant(77)));
        assert_eq!(
            reader.union_value::<KnownUnion>(0),
            Ok(UnionValue::Unknown(UnionDiscriminant(77)))
        );
    }

    #[test]
    fn pointer_defaults_are_followed_through_the_same_budget() {
        let mut message_bytes = vec![0u8; 8];
        WirePointer::new_struct(0, 0, 0)
            .expect("empty root fits")
            .write_to(&mut message_bytes, 0)
            .expect("root word fits");
        let message = MessageSegments::new(&[&message_bytes]).expect("message is aligned");

        let mut default_bytes = vec![0u8; 16];
        WirePointer::new_list(0, ElementSize::Byte, 8)
            .expect("default Text fits")
            .write_to(&mut default_bytes, 0)
            .expect("default pointer fits");
        default_bytes[8..16].copy_from_slice(b"default\0");
        let defaults = MessageSegments::new(&[&default_bytes]).expect("default is aligned");

        let budget = LocalTraversalBudget::new(0);
        let root = message
            .read_struct(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(2),
            )
            .expect("zero-sized root costs no words");
        let default = PointerDefault {
            segments: &defaults,
            location: WireLocation {
                segment_id: 0,
                word_offset: 0,
            },
        };
        assert_eq!(
            root.read_text(0, Some(default)),
            Err(StructReadError::Blob(BlobError::Traversal(
                TraversalError::Budget(BudgetExhausted {
                    requested_words: 1,
                    remaining_words: 0,
                })
            )))
        );

        let budget = LocalTraversalBudget::new(1);
        let root = message
            .read_struct(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(2),
            )
            .expect("zero-sized root costs no words");
        assert_eq!(
            root.read_text(0, Some(default))
                .expect("default fits its budget")
                .to_str(),
            Ok("default")
        );
    }

    #[test]
    fn nested_struct_pointer_access_reuses_context_and_copied_nesting() {
        let mut bytes = vec![0u8; 24];
        WirePointer::new_struct(0, 0, 1)
            .expect("root pointer fits")
            .write_to(&mut bytes, 0)
            .expect("root pointer word fits");
        WirePointer::new_struct(0, 1, 0)
            .expect("child pointer fits")
            .write_to(&mut bytes, 8)
            .expect("child pointer word fits");
        bytes[16..24].copy_from_slice(&55u64.to_le_bytes());
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(2);
        let root = segments
            .read_struct(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(2),
            )
            .expect("root struct validates");
        let child = root
            .read_struct(0, None)
            .expect("child struct validates through the same context");
        assert_eq!(
            child.data_section().expect("child data").read_u64(0, 0),
            Ok(55)
        );
        assert_eq!(budget.remaining_words(), 0);
    }
}
