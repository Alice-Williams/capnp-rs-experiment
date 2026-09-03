use core::fmt;

use alloc::{boxed::Box, sync::Arc, vec};

use capnp_wire::{ElementSize, PointerKind, Segment, WirePointer};

use crate::{BudgetExhausted, NestingLimit, NestingLimitExceeded, TraversalBudget};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WireLocation {
    pub segment_id: u32,
    pub word_offset: u32,
}

impl WireLocation {
    pub const ROOT: Self = Self {
        segment_id: 0,
        word_offset: 0,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StructRef {
    pub content: WireLocation,
    pub data_words: u16,
    pub pointer_count: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListRef {
    pub content: WireLocation,
    pub element_size: ElementSize,
    pub element_count: u32,
    pub content_words: u32,
    pub inline_struct_size: Option<(u16, u16)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapabilityRef {
    pub index: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResolvedPointer {
    Null,
    Struct(StructRef),
    List(ListRef),
    Capability(CapabilityRef),
}

impl ResolvedPointer {
    pub const fn list(self) -> Option<ListRef> {
        match self {
            Self::List(list) => Some(list),
            Self::Null | Self::Struct(_) | Self::Capability(_) => None,
        }
    }

    pub const fn structure(self) -> Option<StructRef> {
        match self {
            Self::Struct(structure) => Some(structure),
            Self::Null | Self::List(_) | Self::Capability(_) => None,
        }
    }
}

/// A validated target whose traversal charge has already been deducted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoundedPointer {
    pub pointer: ResolvedPointer,
    pub child_nesting: NestingLimit,
    pub charged_words: u64,
}

pub(crate) enum FastByteList<'a> {
    Null,
    Bytes(&'a [u8]),
    Slow,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TraversalStats {
    pub pointers_followed: u64,
    pub words_charged: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TraversalError {
    Validation(ValidationError),
    Budget(BudgetExhausted),
    Nesting(NestingLimitExceeded),
}

impl fmt::Display for TraversalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => error.fmt(formatter),
            Self::Budget(error) => error.fmt(formatter),
            Self::Nesting(error) => error.fmt(formatter),
        }
    }
}

impl core::error::Error for TraversalError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Budget(error) => Some(error),
            Self::Nesting(error) => Some(error),
        }
    }
}

impl From<ValidationError> for TraversalError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

impl From<BudgetExhausted> for TraversalError {
    fn from(value: BudgetExhausted) -> Self {
        Self::Budget(value)
    }
}

impl From<NestingLimitExceeded> for TraversalError {
    fn from(value: NestingLimitExceeded) -> Self {
        Self::Nesting(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ValidationError {
    NoSegments,
    SegmentNotWordAligned {
        segment_id: u32,
        bytes: usize,
    },
    TooManySegments,
    UnknownSegment {
        segment_id: u32,
    },
    PointerOutOfBounds {
        location: WireLocation,
    },
    TargetBeforeSegment {
        location: WireLocation,
        offset: i32,
    },
    TargetWordOverflow,
    ObjectOutOfBounds {
        location: WireLocation,
        words: u64,
        segment_words: u64,
    },
    ReservedPointer {
        lower32: u32,
    },
    InvalidFarLandingPad,
    InvalidDoubleFarTag,
    InvalidInlineCompositeTag,
    InlineCompositeOverrun {
        required_words: u64,
        declared_words: u32,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for ValidationError {}

/// Immutable borrowed segments addressed only by stable coordinates.
#[derive(Debug)]
pub struct MessageSegments<'a> {
    first: &'a [u8],
    segments: SegmentStorage<'a>,
}

#[derive(Debug)]
enum SegmentStorage<'a> {
    One,
    Two(&'a [u8]),
    Borrowed(&'a [&'a [u8]]),
    Descriptors(&'a [Segment<'a>]),
    Owned(&'a [Arc<[u8]>]),
    Many(Box<[&'a [u8]]>),
}

impl<'a> MessageSegments<'a> {
    #[inline]
    pub fn new(segments: &[&'a [u8]]) -> Result<Self, ValidationError> {
        validate_segments(segments)?;
        let first = segments[0];
        let segments = match segments {
            [_] => SegmentStorage::One,
            [_, second] => SegmentStorage::Two(second),
            many => SegmentStorage::Many(many.into()),
        };
        Ok(Self { first, segments })
    }

    /// Borrows caller-owned segment descriptors after validating their shape.
    ///
    /// This avoids copying or allocating descriptor storage when a framing layer
    /// already keeps a reusable descriptor array alive for the reader context.
    /// Segment bodies remain borrowed exactly as they are with [`Self::new`].
    #[inline]
    pub fn new_borrowed(segments: &'a [&'a [u8]]) -> Result<Self, ValidationError> {
        validate_segments(segments)?;
        Ok(Self {
            first: segments[0],
            segments: SegmentStorage::Borrowed(segments),
        })
    }

    /// Borrows descriptors whose word alignment was already validated.
    #[inline(always)]
    pub fn from_descriptors(segments: &'a [Segment<'a>]) -> Result<Self, ValidationError> {
        if segments.is_empty() {
            return Err(ValidationError::NoSegments);
        }
        if segments.len() > u32::MAX as usize {
            return Err(ValidationError::TooManySegments);
        }
        Ok(Self {
            first: segments[0].bytes(),
            segments: SegmentStorage::Descriptors(segments),
        })
    }

    #[inline(always)]
    pub(crate) fn from_owned_segments(segments: &'a [Arc<[u8]>]) -> Self {
        let first = segments
            .first()
            .expect("OwnedMessage always contains at least one segment")
            .as_ref();
        Self {
            first,
            segments: SegmentStorage::Owned(segments),
        }
    }

    #[inline]
    pub fn segment_count(&self) -> usize {
        match &self.segments {
            SegmentStorage::One => 1,
            SegmentStorage::Two(_) => 2,
            SegmentStorage::Borrowed(segments) => segments.len(),
            SegmentStorage::Descriptors(segments) => segments.len(),
            SegmentStorage::Owned(segments) => segments.len(),
            SegmentStorage::Many(segments) => segments.len(),
        }
    }

    #[inline(always)]
    pub fn segment(&self, id: u32) -> Option<&'a [u8]> {
        if id == 0 {
            return Some(self.first);
        }
        let index = usize::try_from(id).ok()?;
        match &self.segments {
            SegmentStorage::One => None,
            SegmentStorage::Two(second) => (index == 1).then_some(*second),
            SegmentStorage::Borrowed(segments) => segments.get(index).copied(),
            SegmentStorage::Descriptors(segments) => {
                segments.get(index).copied().map(Segment::bytes)
            }
            SegmentStorage::Owned(segments) => segments.get(index).map(AsRef::as_ref),
            SegmentStorage::Many(segments) => segments.get(index).copied(),
        }
    }

    #[inline(always)]
    pub(crate) fn try_read_byte_list_fast<B: TraversalBudget>(
        &self,
        location: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> FastByteList<'a> {
        let Some(source_segment) = self.segment(location.segment_id) else {
            return FastByteList::Slow;
        };
        let Some(byte_offset) = usize::try_from(location.word_offset)
            .ok()
            .and_then(|offset| offset.checked_mul(8))
        else {
            return FastByteList::Slow;
        };
        let Some(pointer_bytes) = byte_offset
            .checked_add(8)
            .and_then(|end| source_segment.get(byte_offset..end))
        else {
            return FastByteList::Slow;
        };
        let wire_pointer = WirePointer::from_le_bytes(
            pointer_bytes
                .try_into()
                .expect("a checked wire-word range is exactly eight bytes"),
        );
        if wire_pointer.is_null() {
            return FastByteList::Null;
        }
        if wire_pointer.kind() != PointerKind::List {
            return FastByteList::Slow;
        }
        let fields = wire_pointer
            .list_fields()
            .expect("list discriminator was checked");
        if fields.element_size != ElementSize::Byte {
            return FastByteList::Slow;
        }

        let target = i64::from(location.word_offset) + 1 + i64::from(fields.offset);
        let Ok(target_word) = u32::try_from(target) else {
            return FastByteList::Slow;
        };
        let charged_words = u64::from(fields.count).div_ceil(8);
        let Some(target_end) = u64::from(target_word).checked_add(charged_words) else {
            return FastByteList::Slow;
        };
        if target_end > source_segment.len() as u64 / 8 {
            return FastByteList::Slow;
        }
        let Some(start) = usize::try_from(target_word)
            .ok()
            .and_then(|offset| offset.checked_mul(8))
        else {
            return FastByteList::Slow;
        };
        let Some(bytes) = usize::try_from(fields.count)
            .ok()
            .and_then(|count| start.checked_add(count))
            .and_then(|end| source_segment.get(start..end))
        else {
            return FastByteList::Slow;
        };
        if nesting.remaining() == 0 || budget.try_charge(charged_words).is_err() {
            return FastByteList::Slow;
        }
        FastByteList::Bytes(bytes)
    }

    /// Validates and follows a pointer, returning coordinates rather than native pointers.
    #[inline]
    pub fn validate_pointer(
        &self,
        location: WireLocation,
    ) -> Result<ResolvedPointer, ValidationError> {
        let pointer = self.read_pointer(location)?;
        self.validate_wire_pointer(location, pointer)
    }

    #[inline(always)]
    fn validate_wire_pointer(
        &self,
        location: WireLocation,
        pointer: WirePointer,
    ) -> Result<ResolvedPointer, ValidationError> {
        if pointer.is_null() {
            return Ok(ResolvedPointer::Null);
        }
        match pointer.kind() {
            PointerKind::Struct => self.validate_struct(location, pointer),
            PointerKind::List => self.validate_list(location, pointer),
            PointerKind::Far => self.validate_far(pointer),
            PointerKind::Other if pointer.is_capability() => {
                Ok(ResolvedPointer::Capability(CapabilityRef {
                    index: pointer
                        .capability_index()
                        .expect("capability discriminator was checked"),
                }))
            }
            PointerKind::Other => Err(ValidationError::ReservedPointer {
                lower32: pointer.lower32(),
            }),
        }
    }

    /// Validates one pointer, applies its complete traversal charge, then returns its view.
    ///
    /// Struct and list targets consume one copied nesting level. Null and capability
    /// pointers do not. Far landing pads, physical target words, and zero-sized-list
    /// amplification are included in the single all-or-nothing charge.
    #[inline(always)]
    pub fn validate_pointer_with_limits<B: TraversalBudget>(
        &self,
        location: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<BoundedPointer, TraversalError> {
        let wire_pointer = self.read_pointer(location)?;
        let pointer = self.validate_wire_pointer(location, wire_pointer)?;
        let child_nesting = match pointer {
            ResolvedPointer::Struct(_) | ResolvedPointer::List(_) => nesting.descend()?,
            ResolvedPointer::Null | ResolvedPointer::Capability(_) => nesting,
        };
        let charged_words = traversal_charge(wire_pointer, pointer)?;
        budget.try_charge(charged_words)?;
        Ok(BoundedPointer {
            pointer,
            child_nesting,
            charged_words,
        })
    }

    #[inline]
    pub(crate) fn validate_struct_pointer_with_limits<B: TraversalBudget>(
        &self,
        location: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<BoundedPointer, TraversalError> {
        let (wire_pointer, source_segment) = self.read_pointer_and_segment(location)?;
        if !wire_pointer.is_null() && wire_pointer.kind() == PointerKind::Struct {
            let fields = wire_pointer
                .struct_fields()
                .expect("struct discriminator was checked");
            let content = positional_target(location, fields.offset)?;
            let charged_words = u64::from(fields.data_words) + u64::from(fields.pointer_count);
            check_range_in_segment(content, charged_words, source_segment)?;
            let child_nesting = nesting.descend()?;
            budget.try_charge(charged_words)?;
            return Ok(BoundedPointer {
                pointer: ResolvedPointer::Struct(StructRef {
                    content,
                    data_words: fields.data_words,
                    pointer_count: fields.pointer_count,
                }),
                child_nesting,
                charged_words,
            });
        }

        let pointer = self.validate_wire_pointer(location, wire_pointer)?;
        let child_nesting = match pointer {
            ResolvedPointer::Struct(_) | ResolvedPointer::List(_) => nesting.descend()?,
            ResolvedPointer::Null | ResolvedPointer::Capability(_) => nesting,
        };
        let charged_words = traversal_charge(wire_pointer, pointer)?;
        budget.try_charge(charged_words)?;
        Ok(BoundedPointer {
            pointer,
            child_nesting,
            charged_words,
        })
    }

    #[inline(always)]
    pub(crate) fn validate_root_struct_pointer_with_limits<B: TraversalBudget>(
        &self,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<BoundedPointer, TraversalError> {
        let bytes = self
            .first
            .first_chunk::<8>()
            .ok_or(ValidationError::ObjectOutOfBounds {
                location: WireLocation::ROOT,
                words: 1,
                segment_words: u64::try_from(self.first.len() / 8)
                    .map_err(|_| ValidationError::TargetWordOverflow)?,
            })?;
        let wire_pointer = WirePointer::from_le_bytes(*bytes);
        if !wire_pointer.is_null() && wire_pointer.kind() == PointerKind::Struct {
            let fields = wire_pointer
                .struct_fields()
                .expect("struct discriminator was checked");
            let content = positional_target(WireLocation::ROOT, fields.offset)?;
            let charged_words = u64::from(fields.data_words) + u64::from(fields.pointer_count);
            check_range_in_segment(content, charged_words, self.first)?;
            let child_nesting = nesting.descend()?;
            budget.try_charge(charged_words)?;
            return Ok(BoundedPointer {
                pointer: ResolvedPointer::Struct(StructRef {
                    content,
                    data_words: fields.data_words,
                    pointer_count: fields.pointer_count,
                }),
                child_nesting,
                charged_words,
            });
        }

        let pointer = self.validate_wire_pointer(WireLocation::ROOT, wire_pointer)?;
        let child_nesting = match pointer {
            ResolvedPointer::Struct(_) | ResolvedPointer::List(_) => nesting.descend()?,
            ResolvedPointer::Null | ResolvedPointer::Capability(_) => nesting,
        };
        let charged_words = traversal_charge(wire_pointer, pointer)?;
        budget.try_charge(charged_words)?;
        Ok(BoundedPointer {
            pointer,
            child_nesting,
            charged_words,
        })
    }

    /// Iteratively visits every pointer reachable from `root`.
    ///
    /// Range frames keep wide lists and structs from creating one work-list entry
    /// per sibling. Cycles and overlaps are intentionally revisited and charged;
    /// they terminate through the traversal or nesting limit rather than a call
    /// stack or an identity set.
    pub fn walk_pointer_graph<B: TraversalBudget>(
        &self,
        root: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<TraversalStats, TraversalError> {
        let mut work = vec![WalkItem::Pointer {
            location: root,
            nesting,
        }];
        let mut stats = TraversalStats::default();

        while let Some(item) = work.pop() {
            match item {
                WalkItem::Pointer { location, nesting } => {
                    let bounded = self.validate_pointer_with_limits(location, budget, nesting)?;
                    stats.pointers_followed = stats
                        .pointers_followed
                        .checked_add(1)
                        .ok_or(ValidationError::TargetWordOverflow)?;
                    stats.words_charged = stats
                        .words_charged
                        .checked_add(bounded.charged_words)
                        .ok_or(ValidationError::TargetWordOverflow)?;
                    match bounded.pointer {
                        ResolvedPointer::Struct(value) if value.pointer_count != 0 => {
                            let first = add_words(value.content, u64::from(value.data_words))?;
                            work.push(WalkItem::PointerRange {
                                next: first,
                                remaining: u32::from(value.pointer_count),
                                stride_words: 1,
                                nesting: bounded.child_nesting,
                            });
                        }
                        ResolvedPointer::List(value)
                            if value.element_size == ElementSize::Pointer
                                && value.element_count != 0 =>
                        {
                            work.push(WalkItem::PointerRange {
                                next: value.content,
                                remaining: value.element_count,
                                stride_words: 1,
                                nesting: bounded.child_nesting,
                            });
                        }
                        ResolvedPointer::List(value)
                            if value.element_size == ElementSize::InlineComposite =>
                        {
                            if let Some((data_words, pointer_count)) = value.inline_struct_size {
                                if value.element_count != 0 && pointer_count != 0 {
                                    let element_nesting = bounded.child_nesting.descend()?;
                                    work.push(WalkItem::InlinePointers {
                                        content: value.content,
                                        element: 0,
                                        element_count: value.element_count,
                                        data_words,
                                        pointer_count,
                                        pointer_index: 0,
                                        nesting: element_nesting,
                                    });
                                }
                            }
                        }
                        ResolvedPointer::Null
                        | ResolvedPointer::Struct(_)
                        | ResolvedPointer::List(_)
                        | ResolvedPointer::Capability(_) => {}
                    }
                }
                WalkItem::PointerRange {
                    next,
                    remaining,
                    stride_words,
                    nesting,
                } => {
                    if remaining > 1 {
                        work.push(WalkItem::PointerRange {
                            next: add_words(next, u64::from(stride_words))?,
                            remaining: remaining - 1,
                            stride_words,
                            nesting,
                        });
                    }
                    work.push(WalkItem::Pointer {
                        location: next,
                        nesting,
                    });
                }
                WalkItem::InlinePointers {
                    content,
                    element,
                    element_count,
                    data_words,
                    pointer_count,
                    pointer_index,
                    nesting,
                } => {
                    let words_per_element = u64::from(data_words) + u64::from(pointer_count);
                    let element_offset = u64::from(element)
                        .checked_mul(words_per_element)
                        .ok_or(ValidationError::TargetWordOverflow)?;
                    let pointer_offset = element_offset
                        .checked_add(u64::from(data_words))
                        .and_then(|offset| offset.checked_add(u64::from(pointer_index)))
                        .ok_or(ValidationError::TargetWordOverflow)?;
                    let location = add_words(content, pointer_offset)?;

                    let next_pointer = pointer_index + 1;
                    if next_pointer < pointer_count {
                        work.push(WalkItem::InlinePointers {
                            content,
                            element,
                            element_count,
                            data_words,
                            pointer_count,
                            pointer_index: next_pointer,
                            nesting,
                        });
                    } else if element + 1 < element_count {
                        work.push(WalkItem::InlinePointers {
                            content,
                            element: element + 1,
                            element_count,
                            data_words,
                            pointer_count,
                            pointer_index: 0,
                            nesting,
                        });
                    }
                    work.push(WalkItem::Pointer { location, nesting });
                }
            }
        }
        Ok(stats)
    }

    #[inline(always)]
    fn validate_struct(
        &self,
        pointer_location: WireLocation,
        pointer: WirePointer,
    ) -> Result<ResolvedPointer, ValidationError> {
        let fields = pointer
            .struct_fields()
            .expect("struct discriminator was checked");
        let content = positional_target(pointer_location, fields.offset)?;
        self.validate_struct_at(content, pointer)
    }

    #[inline(always)]
    fn validate_struct_at(
        &self,
        content: WireLocation,
        pointer: WirePointer,
    ) -> Result<ResolvedPointer, ValidationError> {
        let fields = pointer
            .struct_fields()
            .expect("struct discriminator was checked");
        let words = u64::from(fields.data_words) + u64::from(fields.pointer_count);
        self.check_range(content, words)?;
        Ok(ResolvedPointer::Struct(StructRef {
            content,
            data_words: fields.data_words,
            pointer_count: fields.pointer_count,
        }))
    }

    #[inline(always)]
    fn validate_list(
        &self,
        pointer_location: WireLocation,
        pointer: WirePointer,
    ) -> Result<ResolvedPointer, ValidationError> {
        let fields = pointer
            .list_fields()
            .expect("list discriminator was checked");
        let target = positional_target(pointer_location, fields.offset)?;
        self.validate_list_at(target, pointer)
    }

    #[inline(always)]
    fn validate_list_at(
        &self,
        target: WireLocation,
        pointer: WirePointer,
    ) -> Result<ResolvedPointer, ValidationError> {
        let fields = pointer
            .list_fields()
            .expect("list discriminator was checked");
        if fields.element_size == ElementSize::InlineComposite {
            let declared_words = fields.count;
            self.check_range(target, u64::from(declared_words) + 1)?;
            let tag = self.read_pointer(target)?;
            let tag_fields = tag
                .inline_composite_tag_fields()
                .ok_or(ValidationError::InvalidInlineCompositeTag)?;
            let words_per_element =
                u64::from(tag_fields.data_words) + u64::from(tag_fields.pointer_count);
            let required_words = words_per_element
                .checked_mul(u64::from(tag_fields.element_count))
                .ok_or(ValidationError::TargetWordOverflow)?;
            if required_words > u64::from(declared_words) {
                return Err(ValidationError::InlineCompositeOverrun {
                    required_words,
                    declared_words,
                });
            }
            let content = WireLocation {
                segment_id: target.segment_id,
                word_offset: target
                    .word_offset
                    .checked_add(1)
                    .ok_or(ValidationError::TargetWordOverflow)?,
            };
            Ok(ResolvedPointer::List(ListRef {
                content,
                element_size: fields.element_size,
                element_count: tag_fields.element_count,
                content_words: declared_words,
                inline_struct_size: Some((tag_fields.data_words, tag_fields.pointer_count)),
            }))
        } else {
            let words = list_word_count(fields.element_size, fields.count)?;
            self.check_range(target, words)?;
            Ok(ResolvedPointer::List(ListRef {
                content: target,
                element_size: fields.element_size,
                element_count: fields.count,
                content_words: u32::try_from(words)
                    .map_err(|_| ValidationError::TargetWordOverflow)?,
                inline_struct_size: None,
            }))
        }
    }

    #[inline(always)]
    fn validate_far(&self, pointer: WirePointer) -> Result<ResolvedPointer, ValidationError> {
        let fields = pointer.far_fields().expect("far discriminator was checked");
        let pad = WireLocation {
            segment_id: fields.segment_id,
            word_offset: fields.landing_pad_word,
        };
        self.check_range(pad, if fields.double_far { 2 } else { 1 })?;
        let first = self.read_pointer(pad)?;
        if !fields.double_far {
            if first.kind() == PointerKind::Far {
                return Err(ValidationError::InvalidFarLandingPad);
            }
            return self.validate_non_far(pad, first);
        }

        if first.kind() != PointerKind::Far {
            return Err(ValidationError::InvalidFarLandingPad);
        }
        let first_far = first.far_fields().expect("far discriminator was checked");
        let object = WireLocation {
            segment_id: first_far.segment_id,
            word_offset: first_far.landing_pad_word,
        };
        let tag_location = WireLocation {
            segment_id: pad.segment_id,
            word_offset: pad
                .word_offset
                .checked_add(1)
                .ok_or(ValidationError::TargetWordOverflow)?,
        };
        let tag = self.read_pointer(tag_location)?;
        match tag.kind() {
            PointerKind::Struct => self.validate_struct_at(object, tag),
            PointerKind::List => self.validate_list_at(object, tag),
            PointerKind::Far | PointerKind::Other => Err(ValidationError::InvalidDoubleFarTag),
        }
    }

    #[inline(always)]
    fn validate_non_far(
        &self,
        location: WireLocation,
        pointer: WirePointer,
    ) -> Result<ResolvedPointer, ValidationError> {
        match pointer.kind() {
            PointerKind::Struct => self.validate_struct(location, pointer),
            PointerKind::List => self.validate_list(location, pointer),
            PointerKind::Other if pointer.is_capability() => {
                Ok(ResolvedPointer::Capability(CapabilityRef {
                    index: pointer
                        .capability_index()
                        .expect("capability discriminator was checked"),
                }))
            }
            PointerKind::Other => Err(ValidationError::ReservedPointer {
                lower32: pointer.lower32(),
            }),
            PointerKind::Far => Err(ValidationError::InvalidFarLandingPad),
        }
    }

    #[inline(always)]
    fn read_pointer(&self, location: WireLocation) -> Result<WirePointer, ValidationError> {
        self.read_pointer_and_segment(location)
            .map(|(pointer, _)| pointer)
    }

    #[inline(always)]
    fn read_pointer_and_segment(
        &self,
        location: WireLocation,
    ) -> Result<(WirePointer, &'a [u8]), ValidationError> {
        let segment = self
            .segment(location.segment_id)
            .ok_or(ValidationError::UnknownSegment {
                segment_id: location.segment_id,
            })?;
        check_range_in_segment(location, 1, segment)?;
        let byte_offset = usize::try_from(location.word_offset)
            .map_err(|_| ValidationError::TargetWordOverflow)?
            .checked_mul(8)
            .ok_or(ValidationError::TargetWordOverflow)?;
        let byte_end = byte_offset
            .checked_add(8)
            .ok_or(ValidationError::TargetWordOverflow)?;
        let bytes = segment
            .get(byte_offset..byte_end)
            .ok_or(ValidationError::PointerOutOfBounds { location })?;
        Ok((
            WirePointer::from_le_bytes(
                bytes
                    .try_into()
                    .expect("a checked wire-word range is exactly eight bytes"),
            ),
            segment,
        ))
    }

    #[inline(always)]
    fn check_range(&self, location: WireLocation, words: u64) -> Result<(), ValidationError> {
        let segment = self
            .segment(location.segment_id)
            .ok_or(ValidationError::UnknownSegment {
                segment_id: location.segment_id,
            })?;
        let segment_words =
            u64::try_from(segment.len() / 8).map_err(|_| ValidationError::TargetWordOverflow)?;
        let end = u64::from(location.word_offset)
            .checked_add(words)
            .ok_or(ValidationError::TargetWordOverflow)?;
        if end > segment_words {
            Err(ValidationError::ObjectOutOfBounds {
                location,
                words,
                segment_words,
            })
        } else {
            Ok(())
        }
    }
}

#[inline]
fn validate_segments(segments: &[&[u8]]) -> Result<(), ValidationError> {
    if segments.is_empty() {
        return Err(ValidationError::NoSegments);
    }
    if segments.len() > u32::MAX as usize {
        return Err(ValidationError::TooManySegments);
    }
    for (index, segment) in segments.iter().enumerate() {
        if segment.len() % 8 != 0 {
            return Err(ValidationError::SegmentNotWordAligned {
                segment_id: u32::try_from(index).map_err(|_| ValidationError::TooManySegments)?,
                bytes: segment.len(),
            });
        }
    }
    Ok(())
}

#[inline(always)]
fn check_range_in_segment(
    location: WireLocation,
    words: u64,
    segment: &[u8],
) -> Result<(), ValidationError> {
    let segment_words =
        u64::try_from(segment.len() / 8).map_err(|_| ValidationError::TargetWordOverflow)?;
    let end = u64::from(location.word_offset)
        .checked_add(words)
        .ok_or(ValidationError::TargetWordOverflow)?;
    if end > segment_words {
        Err(ValidationError::ObjectOutOfBounds {
            location,
            words,
            segment_words,
        })
    } else {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum WalkItem {
    Pointer {
        location: WireLocation,
        nesting: NestingLimit,
    },
    PointerRange {
        next: WireLocation,
        remaining: u32,
        stride_words: u32,
        nesting: NestingLimit,
    },
    InlinePointers {
        content: WireLocation,
        element: u32,
        element_count: u32,
        data_words: u16,
        pointer_count: u16,
        pointer_index: u16,
        nesting: NestingLimit,
    },
}

#[inline(always)]
fn traversal_charge(
    wire_pointer: WirePointer,
    resolved: ResolvedPointer,
) -> Result<u64, ValidationError> {
    let landing_pad_words = wire_pointer
        .far_fields()
        .map_or(0, |fields| if fields.double_far { 2 } else { 1 });
    let target_words = match resolved {
        ResolvedPointer::Null | ResolvedPointer::Capability(_) => 0,
        ResolvedPointer::Struct(value) => {
            u64::from(value.data_words) + u64::from(value.pointer_count)
        }
        ResolvedPointer::List(value) => {
            let physical = u64::from(value.content_words)
                + u64::from(value.element_size == ElementSize::InlineComposite);
            let amplified = if value.element_size == ElementSize::Void
                || value.inline_struct_size == Some((0, 0))
            {
                u64::from(value.element_count)
            } else {
                0
            };
            physical
                .checked_add(amplified)
                .ok_or(ValidationError::TargetWordOverflow)?
        }
    };
    target_words
        .checked_add(landing_pad_words)
        .ok_or(ValidationError::TargetWordOverflow)
}

fn add_words(location: WireLocation, words: u64) -> Result<WireLocation, ValidationError> {
    let word_offset = u64::from(location.word_offset)
        .checked_add(words)
        .and_then(|offset| u32::try_from(offset).ok())
        .ok_or(ValidationError::TargetWordOverflow)?;
    Ok(WireLocation {
        segment_id: location.segment_id,
        word_offset,
    })
}

#[inline(always)]
fn positional_target(
    pointer_location: WireLocation,
    offset: i32,
) -> Result<WireLocation, ValidationError> {
    let target = i64::from(pointer_location.word_offset) + 1 + i64::from(offset);
    if target < 0 {
        return Err(ValidationError::TargetBeforeSegment {
            location: pointer_location,
            offset,
        });
    }
    Ok(WireLocation {
        segment_id: pointer_location.segment_id,
        word_offset: u32::try_from(target).map_err(|_| ValidationError::TargetWordOverflow)?,
    })
}

#[inline(always)]
fn list_word_count(element_size: ElementSize, count: u32) -> Result<u64, ValidationError> {
    let count = u64::from(count);
    let words = match element_size {
        ElementSize::Void => 0,
        ElementSize::Bit => {
            count
                .checked_add(63)
                .ok_or(ValidationError::TargetWordOverflow)?
                / 64
        }
        ElementSize::Byte => {
            count
                .checked_add(7)
                .ok_or(ValidationError::TargetWordOverflow)?
                / 8
        }
        ElementSize::TwoBytes => {
            count
                .checked_add(3)
                .ok_or(ValidationError::TargetWordOverflow)?
                / 4
        }
        ElementSize::FourBytes => {
            count
                .checked_add(1)
                .ok_or(ValidationError::TargetWordOverflow)?
                / 2
        }
        ElementSize::EightBytes | ElementSize::Pointer => count,
        ElementSize::InlineComposite => return Err(ValidationError::InvalidInlineCompositeTag),
    };
    Ok(words)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LocalTraversalBudget, TraversalBudget};
    use alloc::vec::Vec;

    fn with_pointer(pointer: WirePointer, trailing_words: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; (trailing_words + 1) * 8];
        pointer.write_to(&mut bytes, 0).expect("pointer word fits");
        bytes
    }

    #[test]
    fn one_and_two_segment_contexts_use_inline_storage() {
        let bytes = [0u8; 8];
        let one = MessageSegments::new(&[&bytes]).expect("one segment is valid");
        assert!(matches!(one.segments, SegmentStorage::One));

        let two = MessageSegments::new(&[&bytes, &bytes]).expect("two segments are valid");
        assert!(matches!(two.segments, SegmentStorage::Two(_)));
        assert_eq!(two.segment(0), Some(bytes.as_slice()));
        assert_eq!(two.segment(1), Some(bytes.as_slice()));
        assert_eq!(two.segment(2), None);
    }

    #[test]
    fn borrowed_context_reuses_caller_descriptor_storage() {
        let bytes = [0u8; 8];
        let descriptors = [&bytes[..]; 3];
        let message = MessageSegments::new_borrowed(&descriptors).expect("segments are valid");
        assert!(matches!(message.segments, SegmentStorage::Borrowed(_)));
        assert_eq!(message.segment_count(), 3);
        assert_eq!(message.segment(2), Some(bytes.as_slice()));
    }

    #[test]
    fn owned_context_borrows_arc_table_without_descriptor_copy() {
        let bytes: Arc<[u8]> = Arc::from([0u8; 8]);
        let owned = [bytes];
        let message = MessageSegments::from_owned_segments(&owned);
        assert!(matches!(message.segments, SegmentStorage::Owned(_)));
        assert_eq!(message.segment_count(), 1);
        assert_eq!(message.segment(0), Some(owned[0].as_ref()));
    }

    #[test]
    fn validated_descriptors_are_borrowed_without_a_shape_scan() {
        let bytes = [0u8; 8];
        let descriptor = Segment::from_bytes(&bytes).expect("one complete word");
        let descriptors = [descriptor; 3];
        let message = MessageSegments::from_descriptors(&descriptors).expect("non-empty table");
        assert!(matches!(message.segments, SegmentStorage::Descriptors(_)));
        assert_eq!(message.segment(2), Some(bytes.as_slice()));
    }

    #[test]
    fn null_struct_and_capability_validate_to_coordinates() {
        let null = [0u8; 8];
        let segments = MessageSegments::new(&[&null]).expect("segment is aligned");
        assert_eq!(
            segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            Ok(ResolvedPointer::Null)
        );

        let bytes = with_pointer(
            WirePointer::new_struct(0, 1, 1).expect("struct pointer fits"),
            2,
        );
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        assert!(matches!(
            segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            Ok(ResolvedPointer::Struct(_))
        ));

        let bytes = with_pointer(WirePointer::new_capability(42), 0);
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        assert_eq!(
            segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            Ok(ResolvedPointer::Capability(CapabilityRef { index: 42 }))
        );
    }

    #[test]
    fn every_direct_list_size_is_bounds_checked() {
        for element_size in ElementSize::ALL[..7].iter().copied() {
            let bytes = with_pointer(
                WirePointer::new_list(0, element_size, 8).expect("list pointer fits"),
                8,
            );
            let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
            assert!(matches!(
                segments.validate_pointer(WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                }),
                Ok(ResolvedPointer::List(_))
            ));
        }
    }

    #[test]
    fn inline_composite_rejects_element_overrun() {
        let list =
            WirePointer::new_list(0, ElementSize::InlineComposite, 1).expect("list pointer fits");
        let tag = WirePointer::new_inline_composite_tag(2, 1, 0).expect("tag fits");
        let mut bytes = vec![0u8; 24];
        list.write_to(&mut bytes, 0).expect("list fits");
        tag.write_to(&mut bytes, 8).expect("tag fits");
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        assert!(matches!(
            segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            Err(ValidationError::InlineCompositeOverrun { .. })
        ));
    }

    #[test]
    fn valid_inline_composite_and_reserved_other_are_distinguished() {
        let list =
            WirePointer::new_list(0, ElementSize::InlineComposite, 4).expect("list pointer fits");
        let tag = WirePointer::new_inline_composite_tag(2, 1, 1).expect("tag fits");
        let mut bytes = vec![0u8; 48];
        list.write_to(&mut bytes, 0).expect("list fits");
        tag.write_to(&mut bytes, 8).expect("tag fits");
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        assert!(matches!(
            segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            Ok(ResolvedPointer::List(ListRef {
                element_size: ElementSize::InlineComposite,
                element_count: 2,
                content_words: 4,
                inline_struct_size: Some((1, 1)),
                ..
            }))
        ));

        let reserved = WirePointer::from_le_bytes([7, 0, 0, 0, 0, 0, 0, 0]);
        let bytes = with_pointer(reserved, 0);
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        assert_eq!(
            segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            Err(ValidationError::ReservedPointer { lower32: 7 })
        );
    }

    #[test]
    fn single_far_pointer_resolves_relative_to_its_landing_pad() {
        let segment0 = with_pointer(
            WirePointer::new_far(false, 0, 1).expect("outer far pointer fits"),
            0,
        );
        let segment1 = with_pointer(
            WirePointer::new_struct(0, 1, 0).expect("landing pointer fits"),
            1,
        );
        let segments = MessageSegments::new(&[&segment0, &segment1]).expect("segments are aligned");
        assert_eq!(
            segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            Ok(ResolvedPointer::Struct(StructRef {
                content: WireLocation {
                    segment_id: 1,
                    word_offset: 1,
                },
                data_words: 1,
                pointer_count: 0,
            }))
        );
    }

    #[test]
    fn double_far_pointer_uses_adjacent_tag_and_explicit_target() {
        let segment0 = with_pointer(
            WirePointer::new_far(true, 0, 1).expect("outer far pointer fits"),
            0,
        );
        let mut segment1 = vec![0u8; 16];
        WirePointer::new_far(false, 0, 2)
            .expect("inner far pointer fits")
            .write_to(&mut segment1, 0)
            .expect("inner far word fits");
        WirePointer::new_struct(0, 1, 0)
            .expect("tag fits")
            .write_to(&mut segment1, 8)
            .expect("tag word fits");
        let segment2 = [0u8; 8];
        let segments =
            MessageSegments::new(&[&segment0, &segment1, &segment2]).expect("segments are aligned");
        assert_eq!(
            segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            Ok(ResolvedPointer::Struct(StructRef {
                content: WireLocation {
                    segment_id: 2,
                    word_offset: 0,
                },
                data_words: 1,
                pointer_count: 0,
            }))
        );
    }

    #[test]
    fn malformed_far_pads_fail_without_following_unchecked_coordinates() {
        let unknown = with_pointer(
            WirePointer::new_far(false, 0, 7).expect("far pointer fits"),
            0,
        );
        let segments = MessageSegments::new(&[&unknown]).expect("segment is aligned");
        assert_eq!(
            segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            Err(ValidationError::UnknownSegment { segment_id: 7 })
        );

        let outer = with_pointer(
            WirePointer::new_far(true, 0, 1).expect("far pointer fits"),
            0,
        );
        let bad_pad = [0u8; 16];
        let segments = MessageSegments::new(&[&outer, &bad_pad]).expect("segments are aligned");
        assert_eq!(
            segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }),
            Err(ValidationError::InvalidFarLandingPad)
        );
    }

    #[test]
    fn malformed_direct_pointers_never_escape_the_segment() {
        let mut state = 0x1234_5678_9abc_def0u64;
        for _ in 0..10_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let bytes = state.to_le_bytes();
            let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
            if let Ok(resolved) = segments.validate_pointer(WireLocation {
                segment_id: 0,
                word_offset: 0,
            }) {
                match resolved {
                    ResolvedPointer::Struct(value) => assert!(value.content.word_offset <= 1),
                    ResolvedPointer::List(value) => assert!(value.content.word_offset <= 1),
                    ResolvedPointer::Null | ResolvedPointer::Capability(_) => {}
                }
            }
        }
    }

    #[test]
    fn randomized_multisegment_words_never_return_out_of_bounds_coordinates() {
        let mut state = 0x0ddc_0ffe_e15e_beefu64;
        for _ in 0..10_000 {
            let mut first = [0u8; 32];
            let mut second = [0u8; 32];
            for chunk in first.chunks_exact_mut(8).chain(second.chunks_exact_mut(8)) {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                chunk.copy_from_slice(&state.to_le_bytes());
            }
            let segments = MessageSegments::new(&[&first, &second]).expect("segments are aligned");
            for segment_id in 0..2 {
                if let Ok(resolved) = segments.validate_pointer(WireLocation {
                    segment_id,
                    word_offset: 0,
                }) {
                    match resolved {
                        ResolvedPointer::Struct(value) => {
                            let end = u64::from(value.content.word_offset)
                                + u64::from(value.data_words)
                                + u64::from(value.pointer_count);
                            assert!(value.content.segment_id < 2);
                            assert!(end <= 4);
                        }
                        ResolvedPointer::List(value) => {
                            let end = u64::from(value.content.word_offset)
                                + u64::from(value.content_words);
                            assert!(value.content.segment_id < 2);
                            assert!(end <= 4);
                        }
                        ResolvedPointer::Null | ResolvedPointer::Capability(_) => {}
                    }
                }
            }
        }
    }

    #[test]
    fn a_target_is_charged_completely_before_its_view_is_returned() {
        let bytes = with_pointer(
            WirePointer::new_struct(0, 2, 1).expect("struct pointer fits"),
            3,
        );
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(2);
        assert_eq!(
            segments.validate_pointer_with_limits(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(1),
            ),
            Err(TraversalError::Budget(BudgetExhausted {
                requested_words: 3,
                remaining_words: 2,
            }))
        );
        assert_eq!(budget.remaining_words(), 2);
    }

    #[test]
    fn void_and_zero_sized_struct_lists_pay_amplification_charges() {
        let void = with_pointer(
            WirePointer::new_list(0, ElementSize::Void, 20).expect("void list fits"),
            0,
        );
        let segments = MessageSegments::new(&[&void]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(19);
        assert!(matches!(
            segments.validate_pointer_with_limits(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(1),
            ),
            Err(TraversalError::Budget(BudgetExhausted {
                requested_words: 20,
                remaining_words: 19,
            }))
        ));

        let list = WirePointer::new_list(0, ElementSize::InlineComposite, 0)
            .expect("inline list pointer fits");
        let tag = WirePointer::new_inline_composite_tag(20, 0, 0).expect("tag fits");
        let mut bytes = vec![0u8; 16];
        list.write_to(&mut bytes, 0).expect("list fits");
        tag.write_to(&mut bytes, 8).expect("tag fits");
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(21);
        let bounded = segments
            .validate_pointer_with_limits(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(1),
            )
            .expect("tag plus one word per logical element fits exactly");
        assert_eq!(bounded.charged_words, 21);
        assert_eq!(budget.remaining_words(), 0);
    }

    #[test]
    fn far_landing_pads_are_part_of_the_charge() {
        let segment0 = with_pointer(
            WirePointer::new_far(false, 0, 1).expect("outer far pointer fits"),
            0,
        );
        let segment1 = with_pointer(
            WirePointer::new_struct(0, 1, 0).expect("landing pointer fits"),
            1,
        );
        let segments = MessageSegments::new(&[&segment0, &segment1]).expect("segments are aligned");
        let budget = LocalTraversalBudget::new(2);
        let bounded = segments
            .validate_pointer_with_limits(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(1),
            )
            .expect("landing pad and target both fit");
        assert_eq!(bounded.charged_words, 2);
        assert_eq!(budget.remaining_words(), 0);
    }

    #[test]
    fn cycles_repeat_charges_until_the_exact_budget_terminates_the_walk() {
        let bytes = with_pointer(
            WirePointer::new_struct(-1, 0, 1).expect("self pointer fits"),
            0,
        );
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(3);
        assert_eq!(
            segments.walk_pointer_graph(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new(10),
            ),
            Err(TraversalError::Budget(BudgetExhausted {
                requested_words: 1,
                remaining_words: 0,
            }))
        );
        assert_eq!(budget.remaining_words(), 0);
    }

    #[test]
    fn overlapping_targets_are_charged_for_each_dereference() {
        let mut bytes = vec![0u8; 32];
        WirePointer::new_struct(0, 0, 2)
            .expect("root fits")
            .write_to(&mut bytes, 0)
            .expect("root word fits");
        WirePointer::new_struct(1, 1, 0)
            .expect("first child fits")
            .write_to(&mut bytes, 8)
            .expect("first child word fits");
        WirePointer::new_struct(0, 1, 0)
            .expect("second child fits")
            .write_to(&mut bytes, 16)
            .expect("second child word fits");
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(4);
        assert_eq!(
            segments
                .walk_pointer_graph(
                    WireLocation {
                        segment_id: 0,
                        word_offset: 0,
                    },
                    &budget,
                    NestingLimit::new(2),
                )
                .expect("both aliases fit"),
            TraversalStats {
                pointers_followed: 3,
                words_charged: 4,
            }
        );
        assert_eq!(budget.remaining_words(), 0);
    }

    #[test]
    fn malicious_depth_uses_an_iterative_work_list() {
        const POINTERS: usize = 50_000;
        let mut bytes = vec![0u8; POINTERS * 8];
        for index in 0..POINTERS - 1 {
            WirePointer::new_struct(0, 0, 1)
                .expect("chain pointer fits")
                .write_to(&mut bytes, index * 8)
                .expect("chain word fits");
        }
        let segments = MessageSegments::new(&[&bytes]).expect("segment is aligned");
        let budget = LocalTraversalBudget::new(POINTERS as u64);
        let stats = segments
            .walk_pointer_graph(
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                &budget,
                NestingLimit::new((POINTERS - 1) as u32),
            )
            .expect("iterative traversal reaches the null terminator");
        assert_eq!(stats.pointers_followed, POINTERS as u64);
        assert_eq!(stats.words_charged, (POINTERS - 1) as u64);
    }
}
