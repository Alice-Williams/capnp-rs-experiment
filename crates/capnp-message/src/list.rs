//! Typed list readers and Cap'n Proto's legal list evolution rules.
//!
//! The compatibility source is the pinned C++ `readListPointer()` behavior.
//! Primitive and pointer expectations may read the first compatible field of a
//! wider or inline-composite element. Any non-bit list may be viewed as a
//! struct list; the historical bit-list-to-struct upgrade is rejected exactly
//! as in the reference implementation. List targets are charged once when the
//! reader is created, so indexing and iteration have identical accounting.
//!
//! Builders, mutation, schema reflection, and generated typed wrappers are not
//! part of this milestone.

use core::fmt;
use core::iter::FusedIterator;
use core::marker::PhantomData;

use capnp_wire::{
    ElementSize, WireError, read_f32_le, read_f64_le, read_i8, read_i16_le, read_i32_le,
    read_i64_le, read_u8, read_u16_le, read_u32_le, read_u64_le,
};

use crate::{
    BlobError, BoundedPointer, DataReader, DataSection, EnumValue, ListRef, MessageSegments,
    NestingLimit, NestingLimitExceeded, PointerDefault, PrimitiveError, ResolvedPointer,
    ResolvedPointerField, StructReadError, StructReader, TextReader, TraversalBudget,
    TraversalError, UnionDiscriminant, UnionValue, ValidationError, WireLocation,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ListReadError {
    Traversal(TraversalError),
    Blob(BlobError),
    Struct(StructReadError),
    Wire(WireError),
    Primitive(PrimitiveError),
    ExpectedListPointer,
    IncompatibleElementSize {
        actual: ElementSize,
        expected: ElementSize,
    },
    IndexOutOfBounds {
        index: u32,
        len: u32,
    },
    UnknownSegment {
        segment_id: u32,
    },
    RangeOverflow,
    Nesting(NestingLimitExceeded),
}

impl fmt::Display for ListReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for ListReadError {}

macro_rules! error_from {
    ($source:ty, $variant:ident) => {
        impl From<$source> for ListReadError {
            fn from(value: $source) -> Self {
                Self::$variant(value)
            }
        }
    };
}

error_from!(TraversalError, Traversal);
error_from!(BlobError, Blob);
error_from!(StructReadError, Struct);
error_from!(WireError, Wire);
error_from!(PrimitiveError, Primitive);
error_from!(NestingLimitExceeded, Nesting);

impl From<ValidationError> for ListReadError {
    fn from(value: ValidationError) -> Self {
        Self::Traversal(TraversalError::Validation(value))
    }
}

mod sealed {
    pub trait Sealed {}
}

pub trait PrimitiveListElement: sealed::Sealed + Copy {
    const ELEMENT_SIZE: ElementSize;
    #[doc(hidden)]
    fn read_at(bytes: &[u8], bit_offset: u64) -> Result<Self, ListReadError>;
}

impl sealed::Sealed for () {}
impl PrimitiveListElement for () {
    const ELEMENT_SIZE: ElementSize = ElementSize::Void;

    fn read_at(_bytes: &[u8], _bit_offset: u64) -> Result<Self, ListReadError> {
        Ok(())
    }
}

impl sealed::Sealed for bool {}
impl PrimitiveListElement for bool {
    const ELEMENT_SIZE: ElementSize = ElementSize::Bit;

    fn read_at(bytes: &[u8], bit_offset: u64) -> Result<Self, ListReadError> {
        let byte_offset =
            usize::try_from(bit_offset / 8).map_err(|_| ListReadError::RangeOverflow)?;
        let bit = u8::try_from(bit_offset % 8).map_err(|_| ListReadError::RangeOverflow)?;
        Ok(bytes
            .get(byte_offset)
            .is_some_and(|byte| byte & (1 << bit) != 0))
    }
}

macro_rules! primitive_element {
    ($ty:ty, $size:ident, $read:ident) => {
        impl sealed::Sealed for $ty {}
        impl PrimitiveListElement for $ty {
            const ELEMENT_SIZE: ElementSize = ElementSize::$size;

            fn read_at(bytes: &[u8], bit_offset: u64) -> Result<Self, ListReadError> {
                if bit_offset % 8 != 0 {
                    return Err(ListReadError::RangeOverflow);
                }
                let byte_offset =
                    usize::try_from(bit_offset / 8).map_err(|_| ListReadError::RangeOverflow)?;
                Ok($read(bytes, byte_offset)?)
            }
        }
    };
}

primitive_element!(u8, Byte, read_u8);
primitive_element!(i8, Byte, read_i8);
primitive_element!(u16, TwoBytes, read_u16_le);
primitive_element!(i16, TwoBytes, read_i16_le);
primitive_element!(u32, FourBytes, read_u32_le);
primitive_element!(i32, FourBytes, read_i32_le);
primitive_element!(u64, EightBytes, read_u64_le);
primitive_element!(i64, EightBytes, read_i64_le);
primitive_element!(f32, FourBytes, read_f32_le);
primitive_element!(f64, EightBytes, read_f64_le);

#[derive(Debug)]
pub struct ListReader<'context, 'data, B> {
    segments: &'context MessageSegments<'data>,
    budget: &'context B,
    reference: Option<ListRef>,
    nesting: NestingLimit,
}

impl<'context, 'data, B> Clone for ListReader<'context, 'data, B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'context, 'data, B> Copy for ListReader<'context, 'data, B> {}

impl<'data> MessageSegments<'data> {
    /// Opens a charged list reader which can then be viewed as a compatible type.
    ///
    /// ```
    /// use capnp_message::{LocalTraversalBudget, MessageSegments, NestingLimit, WireLocation};
    /// let null = [0u8; 8];
    /// let message = MessageSegments::new(&[&null])?;
    /// let budget = LocalTraversalBudget::new(0);
    /// let values = message
    ///     .read_list(
    ///         WireLocation { segment_id: 0, word_offset: 0 },
    ///         &budget,
    ///         NestingLimit::new(0),
    ///     )?
    ///     .as_primitive::<u32>()?;
    /// assert!(values.is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[inline(always)]
    pub fn read_list<'context, B: TraversalBudget>(
        &'context self,
        location: WireLocation,
        budget: &'context B,
        nesting: NestingLimit,
    ) -> Result<ListReader<'context, 'data, B>, ListReadError> {
        let bounded = self.validate_list_pointer_with_limits(location, budget, nesting)?;
        match bounded.pointer {
            ResolvedPointer::Null => Ok(ListReader::empty(self, budget, bounded.child_nesting)),
            ResolvedPointer::List(reference) => Ok(ListReader {
                segments: self,
                budget,
                reference: Some(reference),
                nesting: bounded.child_nesting,
            }),
            ResolvedPointer::Struct(_) | ResolvedPointer::Capability(_) => {
                Err(ListReadError::ExpectedListPointer)
            }
        }
    }
}

impl<'context, 'data, B: TraversalBudget> ListReader<'context, 'data, B> {
    pub(crate) const fn from_precharged(
        segments: &'context MessageSegments<'data>,
        budget: &'context B,
        reference: Option<ListRef>,
        nesting: NestingLimit,
    ) -> Self {
        Self {
            segments,
            budget,
            reference,
            nesting,
        }
    }

    pub(crate) fn empty(
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

    pub const fn reference(self) -> Option<ListRef> {
        self.reference
    }

    pub const fn len(self) -> u32 {
        match self.reference {
            Some(reference) => reference.element_count,
            None => 0,
        }
    }

    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    #[inline(always)]
    pub fn as_primitive<T: PrimitiveListElement>(
        self,
    ) -> Result<PrimitiveListReader<'context, 'data, B, T>, ListReadError> {
        let layout = self.layout()?;
        if self.reference.is_some() {
            compatible(layout, T::ELEMENT_SIZE)?;
        }
        Ok(PrimitiveListReader {
            bytes: self.segment()?,
            len: self.len(),
            content_start_bits: self.content_start_bits()?,
            step_bits: layout.step_bits,
            marker: PhantomData,
        })
    }

    pub fn as_enum<E: TryFrom<u16>>(
        self,
    ) -> Result<EnumListReader<'context, 'data, B, E>, ListReadError> {
        Ok(EnumListReader {
            elements: self.as_primitive::<u16>()?,
            marker: PhantomData,
        })
    }

    #[inline(always)]
    pub fn as_pointers(self) -> Result<PointerListReader<'context, 'data, B>, ListReadError> {
        if self.reference.is_some() {
            compatible(self.layout()?, ElementSize::Pointer)?;
        }
        Ok(PointerListReader { list: self })
    }

    #[inline(always)]
    pub fn as_structs(self) -> Result<StructListReader<'context, 'data, B>, ListReadError> {
        if self
            .reference
            .is_some_and(|value| value.element_size == ElementSize::Bit)
        {
            return Err(ListReadError::IncompatibleElementSize {
                actual: ElementSize::Bit,
                expected: ElementSize::InlineComposite,
            });
        }
        Ok(StructListReader { list: self })
    }

    #[inline(always)]
    fn layout(self) -> Result<ElementLayout, ListReadError> {
        Ok(match self.reference {
            None => ElementLayout {
                actual: ElementSize::Void,
                step_bits: 0,
                data_bits: 0,
                pointers: 0,
                inline: false,
            },
            Some(reference) => match reference.element_size {
                ElementSize::Void => ElementLayout::data(ElementSize::Void, 0),
                ElementSize::Bit => ElementLayout::data(ElementSize::Bit, 1),
                ElementSize::Byte => ElementLayout::data(ElementSize::Byte, 8),
                ElementSize::TwoBytes => ElementLayout::data(ElementSize::TwoBytes, 16),
                ElementSize::FourBytes => ElementLayout::data(ElementSize::FourBytes, 32),
                ElementSize::EightBytes => ElementLayout::data(ElementSize::EightBytes, 64),
                ElementSize::Pointer => ElementLayout {
                    actual: ElementSize::Pointer,
                    step_bits: 64,
                    data_bits: 0,
                    pointers: 1,
                    inline: false,
                },
                ElementSize::InlineComposite => {
                    let (data_words, pointer_count) = reference
                        .inline_struct_size
                        .ok_or(ListReadError::RangeOverflow)?;
                    ElementLayout {
                        actual: ElementSize::InlineComposite,
                        step_bits: (u64::from(data_words) + u64::from(pointer_count)) * 64,
                        data_bits: u64::from(data_words) * 64,
                        pointers: pointer_count,
                        inline: true,
                    }
                }
            },
        })
    }

    #[inline(always)]
    fn segment(self) -> Result<&'data [u8], ListReadError> {
        let segment_id = self.reference.map_or(0, |value| value.content.segment_id);
        self.segments
            .segment(segment_id)
            .ok_or(ListReadError::UnknownSegment { segment_id })
    }

    #[inline(always)]
    fn content_start_bits(self) -> Result<u64, ListReadError> {
        u64::from(self.reference.map_or(0, |value| value.content.word_offset))
            .checked_mul(64)
            .ok_or(ListReadError::RangeOverflow)
    }
}

#[derive(Clone, Copy)]
struct ElementLayout {
    actual: ElementSize,
    step_bits: u64,
    data_bits: u64,
    pointers: u16,
    inline: bool,
}

impl ElementLayout {
    const fn data(actual: ElementSize, bits: u64) -> Self {
        Self {
            actual,
            step_bits: bits,
            data_bits: bits,
            pointers: 0,
            inline: false,
        }
    }
}

fn expected_shape(size: ElementSize) -> (u64, u16) {
    match size {
        ElementSize::Void => (0, 0),
        ElementSize::Bit => (1, 0),
        ElementSize::Byte => (8, 0),
        ElementSize::TwoBytes => (16, 0),
        ElementSize::FourBytes => (32, 0),
        ElementSize::EightBytes => (64, 0),
        ElementSize::Pointer => (0, 1),
        ElementSize::InlineComposite => (0, 0),
    }
}

fn compatible(layout: ElementLayout, expected: ElementSize) -> Result<(), ListReadError> {
    if expected == ElementSize::Bit && layout.inline
        || layout.actual == ElementSize::Bit && expected != ElementSize::Bit
    {
        return Err(ListReadError::IncompatibleElementSize {
            actual: layout.actual,
            expected,
        });
    }
    let (data_bits, pointers) = expected_shape(expected);
    if data_bits > layout.data_bits || pointers > layout.pointers {
        Err(ListReadError::IncompatibleElementSize {
            actual: layout.actual,
            expected,
        })
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub struct PrimitiveListReader<'context, 'data, B, T> {
    bytes: &'data [u8],
    len: u32,
    content_start_bits: u64,
    step_bits: u64,
    marker: PhantomData<(&'context B, T)>,
}

impl<'context, 'data, B, T> Clone for PrimitiveListReader<'context, 'data, B, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'context, 'data, B, T> Copy for PrimitiveListReader<'context, 'data, B, T> {}

impl<'context, 'data, B: TraversalBudget, T: PrimitiveListElement>
    PrimitiveListReader<'context, 'data, B, T>
{
    pub const fn len(self) -> u32 {
        self.len
    }

    pub const fn is_empty(self) -> bool {
        self.len == 0
    }

    #[inline(always)]
    pub fn get(self, index: u32) -> Result<T, ListReadError> {
        check_index(index, self.len())?;
        let bit_offset = u64::from(index)
            .checked_mul(self.step_bits)
            .and_then(|offset| self.content_start_bits.checked_add(offset))
            .ok_or(ListReadError::RangeOverflow)?;
        T::read_at(self.bytes, bit_offset)
    }

    pub const fn iter(self) -> PrimitiveListIter<'context, 'data, B, T> {
        PrimitiveListIter {
            list: self,
            next: 0,
        }
    }
}

pub struct PrimitiveListIter<'context, 'data, B, T> {
    list: PrimitiveListReader<'context, 'data, B, T>,
    next: u32,
}

impl<B: TraversalBudget, T: PrimitiveListElement> Iterator for PrimitiveListIter<'_, '_, B, T> {
    type Item = Result<T, ListReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.list.len() {
            return None;
        }
        let index = self.next;
        self.next += 1;
        Some(self.list.get(index))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = usize::try_from(self.list.len() - self.next).unwrap_or(usize::MAX);
        (remaining, Some(remaining))
    }
}

impl<B: TraversalBudget, T: PrimitiveListElement> ExactSizeIterator
    for PrimitiveListIter<'_, '_, B, T>
{
}
impl<B: TraversalBudget, T: PrimitiveListElement> FusedIterator
    for PrimitiveListIter<'_, '_, B, T>
{
}

#[derive(Debug)]
pub struct EnumListReader<'context, 'data, B, E> {
    elements: PrimitiveListReader<'context, 'data, B, u16>,
    marker: PhantomData<E>,
}

impl<'context, 'data, B, E> Clone for EnumListReader<'context, 'data, B, E> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'context, 'data, B, E> Copy for EnumListReader<'context, 'data, B, E> {}

impl<'context, 'data, B: TraversalBudget, E: TryFrom<u16>> EnumListReader<'context, 'data, B, E> {
    pub const fn len(self) -> u32 {
        self.elements.len()
    }

    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn get(self, index: u32) -> Result<EnumValue<E>, ListReadError> {
        let ordinal = self.elements.get(index)?;
        Ok(match E::try_from(ordinal) {
            Ok(value) => EnumValue::Known(value),
            Err(_) => EnumValue::Unknown(ordinal),
        })
    }

    pub fn iter(self) -> EnumListIter<'context, 'data, B, E> {
        EnumListIter {
            list: self,
            next: 0,
        }
    }
}

pub struct EnumListIter<'context, 'data, B, E> {
    list: EnumListReader<'context, 'data, B, E>,
    next: u32,
}

impl<B: TraversalBudget, E: TryFrom<u16>> Iterator for EnumListIter<'_, '_, B, E> {
    type Item = Result<EnumValue<E>, ListReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.list.len() {
            return None;
        }
        let index = self.next;
        self.next += 1;
        Some(self.list.get(index))
    }
}

#[derive(Debug)]
pub struct PointerListReader<'context, 'data, B> {
    list: ListReader<'context, 'data, B>,
}

impl<'context, 'data, B> Clone for PointerListReader<'context, 'data, B> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'context, 'data, B> Copy for PointerListReader<'context, 'data, B> {}

impl<'context, 'data, B: TraversalBudget> PointerListReader<'context, 'data, B> {
    pub const fn len(self) -> u32 {
        self.list.len()
    }

    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn get(self, index: u32) -> Result<ResolvedPointerField<'context, 'data>, ListReadError> {
        let (location, nesting) = self.element_location(index)?;
        Ok(ResolvedPointerField {
            source: self.list.segments,
            value: self.list.segments.validate_pointer_with_limits(
                location,
                self.list.budget,
                nesting,
            )?,
        })
    }

    #[inline(always)]
    pub fn get_list(self, index: u32) -> Result<ListReader<'context, 'data, B>, ListReadError> {
        let (location, nesting) = self.element_location(index)?;
        self.list
            .segments
            .read_list(location, self.list.budget, nesting)
    }

    pub fn get_struct(self, index: u32) -> Result<StructReader<'context, 'data, B>, ListReadError> {
        let (location, nesting) = self.element_location(index)?;
        Ok(self
            .list
            .segments
            .read_struct(location, self.list.budget, nesting)?)
    }

    #[inline(always)]
    pub fn read_text(self, index: u32) -> Result<TextReader<'data>, ListReadError> {
        let (location, nesting) = self.element_location(index)?;
        Ok(self
            .list
            .segments
            .read_text(location, self.list.budget, nesting)?)
    }

    #[inline(always)]
    pub fn read_data(self, index: u32) -> Result<DataReader<'data>, ListReadError> {
        let (location, nesting) = self.element_location(index)?;
        Ok(self
            .list
            .segments
            .read_data(location, self.list.budget, nesting)?)
    }

    pub const fn iter(self) -> PointerListIter<'context, 'data, B> {
        PointerListIter {
            list: self,
            next: 0,
        }
    }

    #[inline(always)]
    pub(crate) fn element_location(
        self,
        index: u32,
    ) -> Result<(WireLocation, NestingLimit), ListReadError> {
        check_index(index, self.len())?;
        let reference = self.list.reference.ok_or(ListReadError::RangeOverflow)?;
        if reference.element_size == ElementSize::Pointer {
            return Ok((
                add_words(reference.content, u64::from(index))?,
                self.list.nesting,
            ));
        }
        let (data_words, pointer_count) =
            reference
                .inline_struct_size
                .ok_or(ListReadError::IncompatibleElementSize {
                    actual: reference.element_size,
                    expected: ElementSize::Pointer,
                })?;
        if pointer_count == 0 {
            return Err(ListReadError::IncompatibleElementSize {
                actual: reference.element_size,
                expected: ElementSize::Pointer,
            });
        }
        let step = u64::from(data_words) + u64::from(pointer_count);
        let offset = u64::from(index)
            .checked_mul(step)
            .and_then(|value| value.checked_add(u64::from(data_words)))
            .ok_or(ListReadError::RangeOverflow)?;
        Ok((
            add_words(reference.content, offset)?,
            self.list.nesting.descend()?,
        ))
    }
}

pub struct PointerListIter<'context, 'data, B> {
    list: PointerListReader<'context, 'data, B>,
    next: u32,
}

impl<'context, 'data, B: TraversalBudget> Iterator for PointerListIter<'context, 'data, B> {
    type Item = Result<ResolvedPointerField<'context, 'data>, ListReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.list.len() {
            return None;
        }
        let index = self.next;
        self.next += 1;
        Some(self.list.get(index))
    }
}

#[derive(Debug)]
pub struct StructListReader<'context, 'data, B> {
    list: ListReader<'context, 'data, B>,
}

impl<'context, 'data, B> Clone for StructListReader<'context, 'data, B> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'context, 'data, B> Copy for StructListReader<'context, 'data, B> {}

impl<'context, 'data, B: TraversalBudget> StructListReader<'context, 'data, B> {
    pub const fn len(self) -> u32 {
        self.list.len()
    }

    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }

    pub fn get(self, index: u32) -> Result<StructElementReader<'context, 'data, B>, ListReadError> {
        check_index(index, self.len())?;
        let nesting = self.list.nesting.descend()?;
        let Some(reference) = self.list.reference else {
            return Ok(StructElementReader::empty(
                self.list.segments,
                self.list.budget,
                nesting,
            ));
        };
        let layout = self.list.layout()?;
        let start_bit = self
            .list
            .content_start_bits()?
            .checked_add(
                u64::from(index)
                    .checked_mul(layout.step_bits)
                    .ok_or(ListReadError::RangeOverflow)?,
            )
            .ok_or(ListReadError::RangeOverflow)?;
        let pointer_start = if layout.pointers == 0 {
            None
        } else {
            let element_word = start_bit / 64;
            let pointer_word = element_word
                .checked_add(layout.data_bits / 64)
                .ok_or(ListReadError::RangeOverflow)?;
            Some(WireLocation {
                segment_id: reference.content.segment_id,
                word_offset: u32::try_from(pointer_word)
                    .map_err(|_| ListReadError::RangeOverflow)?,
            })
        };
        Ok(StructElementReader {
            segments: self.list.segments,
            budget: self.list.budget,
            segment_id: reference.content.segment_id,
            data_start_bit: start_bit,
            data_bits: layout.data_bits,
            pointer_start,
            pointer_count: layout.pointers,
            nesting,
        })
    }

    pub const fn iter(self) -> StructListIter<'context, 'data, B> {
        StructListIter {
            list: self,
            next: 0,
        }
    }
}

pub struct StructListIter<'context, 'data, B> {
    list: StructListReader<'context, 'data, B>,
    next: u32,
}

impl<'context, 'data, B: TraversalBudget> Iterator for StructListIter<'context, 'data, B> {
    type Item = Result<StructElementReader<'context, 'data, B>, ListReadError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.list.len() {
            return None;
        }
        let index = self.next;
        self.next += 1;
        Some(self.list.get(index))
    }
}

#[derive(Debug)]
pub struct StructElementReader<'context, 'data, B> {
    segments: &'context MessageSegments<'data>,
    budget: &'context B,
    segment_id: u32,
    data_start_bit: u64,
    data_bits: u64,
    pointer_start: Option<WireLocation>,
    pointer_count: u16,
    nesting: NestingLimit,
}

impl<'context, 'data, B> Clone for StructElementReader<'context, 'data, B> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<'context, 'data, B> Copy for StructElementReader<'context, 'data, B> {}

impl<'context, 'data, B: TraversalBudget> StructElementReader<'context, 'data, B> {
    fn empty(
        segments: &'context MessageSegments<'data>,
        budget: &'context B,
        nesting: NestingLimit,
    ) -> Self {
        Self {
            segments,
            budget,
            segment_id: 0,
            data_start_bit: 0,
            data_bits: 0,
            pointer_start: None,
            pointer_count: 0,
            nesting,
        }
    }

    pub fn data_section(self) -> Result<DataSection<'data>, ListReadError> {
        if self.data_bits == 0 {
            return Ok(DataSection::from_validated_bytes(&[]));
        }
        if self.data_start_bit % 8 != 0 || self.data_bits % 8 != 0 {
            return Err(ListReadError::RangeOverflow);
        }
        let segment =
            self.segments
                .segment(self.segment_id)
                .ok_or(ListReadError::UnknownSegment {
                    segment_id: self.segment_id,
                })?;
        let start =
            usize::try_from(self.data_start_bit / 8).map_err(|_| ListReadError::RangeOverflow)?;
        let len = usize::try_from(self.data_bits / 8).map_err(|_| ListReadError::RangeOverflow)?;
        let end = start.checked_add(len).ok_or(ListReadError::RangeOverflow)?;
        let bytes = segment
            .get(start..end)
            .ok_or(ListReadError::RangeOverflow)?;
        Ok(DataSection::from_validated_bytes(bytes))
    }

    pub const fn pointer_section(self) -> crate::PointerSection<'context, 'data, B> {
        let base = match self.pointer_start {
            Some(location) => location,
            None => WireLocation::ROOT,
        };
        crate::PointerSection::from_parts(
            self.segments,
            self.budget,
            base,
            self.pointer_count,
            self.nesting,
        )
    }

    pub const fn group(self) -> Self {
        self
    }

    pub(crate) const fn nesting(self) -> NestingLimit {
        self.nesting
    }

    pub const fn nesting_limit(self) -> NestingLimit {
        self.nesting
    }

    pub fn union_discriminant(self, offset: u32) -> Result<UnionDiscriminant, ListReadError> {
        Ok(UnionDiscriminant(self.data_section()?.read_u16(offset, 0)?))
    }

    pub fn union_value<C: TryFrom<u16>>(self, offset: u32) -> Result<UnionValue<C>, ListReadError> {
        let discriminant = self.union_discriminant(offset)?;
        Ok(match C::try_from(discriminant.0) {
            Ok(value) => UnionValue::Known(value),
            Err(_) => UnionValue::Unknown(discriminant),
        })
    }

    pub fn pointer_location(self, index: u16) -> Result<Option<WireLocation>, ListReadError> {
        if index >= self.pointer_count {
            return Ok(None);
        }
        let start = self.pointer_start.ok_or(ListReadError::RangeOverflow)?;
        Ok(Some(add_words(start, u64::from(index))?))
    }

    pub fn resolve_pointer<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<ResolvedPointerField<'reader, 'data>, ListReadError>
    where
        'context: 'reader,
    {
        match self.select_pointer(index, default)? {
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

    pub fn read_text<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<TextReader<'data>, ListReadError>
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

    pub fn read_data<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<DataReader<'data>, ListReadError>
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
    ) -> Result<StructReader<'reader, 'data, B>, ListReadError>
    where
        'context: 'reader,
    {
        match self.select_pointer(index, default)? {
            Some((segments, location)) => {
                Ok(segments.read_struct(location, self.budget, self.nesting)?)
            }
            None => Ok(StructReader::empty_from_context(
                self.segments,
                self.budget,
                self.nesting,
            )),
        }
    }

    pub fn read_list<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<ListReader<'reader, 'data, B>, ListReadError>
    where
        'context: 'reader,
    {
        match self.select_pointer(index, default)? {
            Some((segments, location)) => segments.read_list(location, self.budget, self.nesting),
            None => Ok(ListReader::empty(self.segments, self.budget, self.nesting)),
        }
    }

    fn select_pointer<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<Option<(&'reader MessageSegments<'data>, WireLocation)>, ListReadError>
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

impl<'context, 'data, B: TraversalBudget> StructReader<'context, 'data, B> {
    pub fn read_list<'reader>(
        &'reader self,
        index: u16,
        default: Option<PointerDefault<'reader, 'data>>,
    ) -> Result<ListReader<'reader, 'data, B>, ListReadError>
    where
        'context: 'reader,
    {
        match self.select_pointer(index, default)? {
            Some((segments, location)) => {
                segments.read_list(location, self.budget(), self.nesting())
            }
            None => Ok(ListReader::empty(
                self.segments(),
                self.budget(),
                self.nesting(),
            )),
        }
    }
}

fn check_index(index: u32, len: u32) -> Result<(), ListReadError> {
    if index < len {
        Ok(())
    } else {
        Err(ListReadError::IndexOutOfBounds { index, len })
    }
}

fn add_words(location: WireLocation, words: u64) -> Result<WireLocation, ListReadError> {
    let word_offset = u64::from(location.word_offset)
        .checked_add(words)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(ListReadError::RangeOverflow)?;
    Ok(WireLocation {
        segment_id: location.segment_id,
        word_offset,
    })
}
