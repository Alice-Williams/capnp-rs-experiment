//! Exclusive, zero-initializing message construction.
//!
//! This is the ordinary builder from ADR-0003 and follows the pointer and
//! allocation rules in the pinned C++ `layout.c++` implementation. One mutable
//! borrow owns the arena; child builders reborrow it and carry typed word
//! offsets rather than native pointers. Every allocation is checked, grown as
//! zeroed bytes, and only then linked from its parent. M12 extends that same
//! representation across deterministic segments and emits direct, single-far,
//! or double-far pointers according to the actual placement.
//!
//! M13 adds iterative schema-independent copy/clear and typed same-arena
//! orphans. Copy failures roll allocations back, bounded clear failures are
//! non-mutating, and dropped orphans recursively zero abandoned storage.
//! Canonicalization reuses this allocator internally to produce a single dense
//! segment. Generated setters and parallel construction are later milestones.

use core::fmt;
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU64, Ordering};

use alloc::{boxed::Box, vec, vec::Vec};

use capnp_wire::{
    ElementSize, WireError, WirePointer, write_f32_le, write_f64_le, write_i8, write_i16_le,
    write_i32_le, write_i64_le, write_u8, write_u16_le, write_u32_le, write_u64_le,
};

use crate::validation::FastByteList;
use crate::{
    ListRef, MessageSegments, NestingLimit, ResolvedPointer, TraversalBudget, TraversalError,
    ValidationError, WireLocation,
};

/// One segment cannot exceed the span addressable by every signed positional pointer.
pub const MAX_SINGLE_SEGMENT_WORDS: u32 = 1 << 29;
static NEXT_ARENA_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WordOffset {
    segment_id: u32,
    word_offset: u32,
}

impl WordOffset {
    pub const fn segment_id(self) -> u32 {
        self.segment_id
    }

    pub const fn word_offset(self) -> u32 {
        self.word_offset
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StructOffset {
    arena_id: u64,
    content: WordOffset,
    data_words: u16,
    pointer_count: u16,
}

impl StructOffset {
    pub const fn content(self) -> WordOffset {
        self.content
    }

    pub const fn data_words(self) -> u16 {
        self.data_words
    }

    pub const fn pointer_count(self) -> u16 {
        self.pointer_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ListOffset {
    arena_id: u64,
    pointer_target: WordOffset,
    content: WordOffset,
    element_size: ElementSize,
    element_count: u32,
    content_words: u32,
    inline_struct_size: Option<(u16, u16)>,
}

impl ListOffset {
    pub const fn content(self) -> WordOffset {
        self.content
    }

    pub const fn element_size(self) -> ElementSize {
        self.element_size
    }

    pub const fn element_count(self) -> u32 {
        self.element_count
    }

    pub const fn content_words(self) -> u32 {
        self.content_words
    }

    pub const fn inline_struct_size(self) -> Option<(u16, u16)> {
        self.inline_struct_size
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaError {
    InvalidWordLimit { requested: u32 },
    InvalidSegmentLimit { requested: u32 },
    AlreadyInitialized,
    AllocationOverflow,
    AllocationLimit { requested: u64, limit: u64 },
    SegmentLimit { requested: u32, limit: u32 },
    AllocationFailed,
    MultipleSegments,
    IndexOutOfBounds { index: u32, len: u32 },
    PointerIndexOutOfBounds { index: u16, len: u16 },
    Wire(WireError),
}

impl fmt::Display for ArenaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for ArenaError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Wire(error) => Some(error),
            Self::InvalidWordLimit { .. }
            | Self::InvalidSegmentLimit { .. }
            | Self::AlreadyInitialized
            | Self::AllocationOverflow
            | Self::AllocationLimit { .. }
            | Self::SegmentLimit { .. }
            | Self::AllocationFailed
            | Self::MultipleSegments
            | Self::IndexOutOfBounds { .. }
            | Self::PointerIndexOutOfBounds { .. } => None,
        }
    }
}

impl From<WireError> for ArenaError {
    fn from(value: WireError) -> Self {
        Self::Wire(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GraphError {
    Arena(ArenaError),
    Traversal(TraversalError),
    DestinationNotNull { location: WireLocation },
    WrongArena,
    ExpectedStruct,
    ExpectedList,
    CapabilityNotCanonicalizable { index: u32 },
}

impl fmt::Display for GraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for GraphError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Arena(error) => Some(error),
            Self::Traversal(error) => Some(error),
            Self::DestinationNotNull { .. }
            | Self::WrongArena
            | Self::ExpectedStruct
            | Self::ExpectedList
            | Self::CapabilityNotCanonicalizable { .. } => None,
        }
    }
}

impl From<ArenaError> for GraphError {
    fn from(value: ArenaError) -> Self {
        Self::Arena(value)
    }
}

impl From<TraversalError> for GraphError {
    fn from(value: TraversalError) -> Self {
        Self::Traversal(value)
    }
}

impl From<ValidationError> for GraphError {
    fn from(value: ValidationError) -> Self {
        Self::Traversal(TraversalError::Validation(value))
    }
}

#[derive(Debug)]
struct SegmentStorage {
    bytes: Vec<u8>,
    used_bytes: usize,
    word_limit: u32,
}

struct ArenaSegments<'a> {
    first: Option<&'a [u8]>,
    additional: core::slice::Iter<'a, SegmentStorage>,
}

impl<'a> Iterator for ArenaSegments<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        self.first
            .take()
            .or_else(|| self.additional.next().map(SegmentStorage::used))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let length = self.len();
        (length, Some(length))
    }
}

impl ExactSizeIterator for ArenaSegments<'_> {
    fn len(&self) -> usize {
        usize::from(self.first.is_some()) + self.additional.len()
    }
}

impl SegmentStorage {
    #[inline]
    fn used(&self) -> &[u8] {
        &self.bytes[..self.used_bytes]
    }

    #[inline]
    fn used_mut(&mut self) -> &mut [u8] {
        &mut self.bytes[..self.used_bytes]
    }
}

/// A growable, exclusively borrowed arena with deterministic segment policy.
///
/// `new()` retains M11's single-segment behavior. `new_segmented()` fixes the
/// first and preferred later segment sizes so tests and callers can force
/// landing-pad placement without depending on allocator capacity.
#[derive(Debug)]
pub struct ExclusiveArena {
    arena_id: u64,
    first_segment: SegmentStorage,
    additional_segments: Vec<SegmentStorage>,
    next_segment_words: u32,
    max_segments: u32,
    max_total_words: u64,
    root_initialized: bool,
}

impl ExclusiveArena {
    #[inline]
    pub fn new(initial_capacity_words: u32, max_words: u32) -> Result<Self, ArenaError> {
        validate_segment_words(max_words)?;
        Self::new_with_policy(
            initial_capacity_words.max(1).min(max_words),
            max_words,
            max_words,
            1,
            u64::from(max_words),
        )
    }

    #[inline]
    pub fn new_segmented(
        first_segment_words: u32,
        next_segment_words: u32,
        max_segments: u32,
        max_total_words: u64,
    ) -> Result<Self, ArenaError> {
        validate_segment_words(first_segment_words)?;
        validate_segment_words(next_segment_words)?;
        if max_segments == 0 {
            return Err(ArenaError::InvalidSegmentLimit {
                requested: max_segments,
            });
        }
        if max_total_words == 0 {
            return Err(ArenaError::InvalidWordLimit { requested: 0 });
        }
        Self::new_with_policy(
            first_segment_words,
            first_segment_words,
            next_segment_words,
            max_segments,
            max_total_words,
        )
    }

    #[inline]
    fn new_with_policy(
        initial_capacity_words: u32,
        first_word_limit: u32,
        next_segment_words: u32,
        max_segments: u32,
        max_total_words: u64,
    ) -> Result<Self, ArenaError> {
        let arena_id = NEXT_ARENA_ID.fetch_add(1, Ordering::Relaxed);
        if arena_id == u64::MAX {
            return Err(ArenaError::AllocationOverflow);
        }
        let initialized_bytes = word_bytes(initial_capacity_words)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(initialized_bytes)
            .map_err(|_| ArenaError::AllocationFailed)?;
        bytes.resize(initialized_bytes, 0);
        Ok(Self {
            arena_id,
            first_segment: SegmentStorage {
                bytes,
                used_bytes: 8,
                word_limit: first_word_limit,
            },
            additional_segments: Vec::new(),
            next_segment_words,
            max_segments,
            max_total_words,
            root_initialized: false,
        })
    }

    #[inline]
    pub fn word_len(&self) -> u64 {
        core::iter::once(&self.first_segment)
            .chain(self.additional_segments.iter())
            .map(|segment| (segment.used_bytes / 8) as u64)
            .sum()
    }

    pub const fn max_words(&self) -> u64 {
        self.max_total_words
    }

    #[inline]
    pub fn segment_count(&self) -> usize {
        self.additional_segments.len() + 1
    }

    #[inline]
    pub fn segment(&self, id: u32) -> Option<&[u8]> {
        if id == 0 {
            return Some(self.first_segment.used());
        }
        usize::try_from(id - 1)
            .ok()
            .and_then(|index| self.additional_segments.get(index))
            .map(SegmentStorage::used)
    }

    #[inline]
    pub fn segments(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        ArenaSegments {
            first: Some(self.first_segment.used()),
            additional: self.additional_segments.iter(),
        }
    }

    /// Clears all used storage and retains the first segment for another message.
    ///
    /// Resetting requires exclusive access, so no builder can remain live. Every
    /// previously used byte is zeroed before additional segments are released,
    /// and the arena identity changes so copied offsets cannot be reused with the
    /// next message.
    #[inline]
    pub fn reset(&mut self) -> Result<(), ArenaError> {
        let arena_id = NEXT_ARENA_ID.fetch_add(1, Ordering::Relaxed);
        if arena_id == u64::MAX {
            return Err(ArenaError::AllocationOverflow);
        }
        self.first_segment.used_mut().fill(0);
        for segment in &mut self.additional_segments {
            segment.used_mut().fill(0);
        }
        self.first_segment.used_bytes = 8;
        self.additional_segments.clear();
        self.arena_id = arena_id;
        self.root_initialized = false;
        Ok(())
    }

    pub fn as_segment(&self) -> &[u8] {
        self.segment(0).expect("an arena always has segment zero")
    }

    pub fn into_segment(mut self) -> Result<Box<[u8]>, ArenaError> {
        if !self.additional_segments.is_empty() {
            return Err(ArenaError::MultipleSegments);
        }
        self.first_segment
            .bytes
            .truncate(self.first_segment.used_bytes);
        Ok(self.first_segment.bytes.into_boxed_slice())
    }

    pub fn into_segments(mut self) -> Vec<Box<[u8]>> {
        let mut output = Vec::with_capacity(self.additional_segments.len() + 1);
        self.first_segment
            .bytes
            .truncate(self.first_segment.used_bytes);
        output.push(self.first_segment.bytes.into_boxed_slice());
        output.extend(self.additional_segments.into_iter().map(|mut segment| {
            segment.bytes.truncate(segment.used_bytes);
            segment.bytes.into_boxed_slice()
        }));
        output
    }

    pub(crate) fn primitive_list_storage_mut<T: PrimitiveListValue>(
        &mut self,
        reference: ListOffset,
    ) -> Result<&mut [u8], ArenaError> {
        if reference.arena_id != self.arena_id || reference.element_size != T::ELEMENT_SIZE {
            return Err(ArenaError::AllocationOverflow);
        }
        let start = byte_offset(reference.content)?;
        let bytes = usize::try_from(reference.content_words)
            .ok()
            .and_then(|words| words.checked_mul(8))
            .ok_or(ArenaError::AllocationOverflow)?;
        let end = start
            .checked_add(bytes)
            .ok_or(ArenaError::AllocationOverflow)?;
        self.segment_mut(reference.content.segment_id)?
            .get_mut(start..end)
            .ok_or(ArenaError::AllocationOverflow)
    }

    pub(crate) fn set_pointer_list_far(
        &mut self,
        reference: ListOffset,
        index: u32,
        target_segment: u32,
    ) -> Result<(), ArenaError> {
        if reference.arena_id != self.arena_id || reference.element_size != ElementSize::Pointer {
            return Err(ArenaError::AllocationOverflow);
        }
        check_index(index, reference.element_count)?;
        let slot = add_words(reference.content, u64::from(index))?;
        self.write_pointer(slot, WirePointer::new_far(false, 0, target_segment)?)
    }

    /// Initializes the root once and returns the arena's exclusive struct view.
    #[inline]
    pub fn init_root_struct(
        &mut self,
        data_words: u16,
        pointer_count: u16,
    ) -> Result<StructBuilder<'_>, ArenaError> {
        self.require_uninitialized_root()?;
        let reference = self.allocate_struct(data_words, pointer_count)?;
        self.emit_struct(root_offset(), reference)?;
        self.root_initialized = true;
        Ok(StructBuilder {
            arena: self,
            reference,
        })
    }

    pub fn init_root_list<T: PrimitiveListValue>(
        &mut self,
        element_count: u32,
    ) -> Result<DataListBuilder<'_, T>, ArenaError> {
        self.require_uninitialized_root()?;
        let reference = self.allocate_data_list(T::ELEMENT_SIZE, element_count)?;
        self.emit_list(root_offset(), reference)?;
        self.root_initialized = true;
        Ok(DataListBuilder {
            arena: self,
            reference,
            marker: PhantomData,
        })
    }

    pub fn init_root_pointer_list(
        &mut self,
        element_count: u32,
    ) -> Result<PointerListBuilder<'_>, ArenaError> {
        self.require_uninitialized_root()?;
        let reference = self.allocate_data_list(ElementSize::Pointer, element_count)?;
        self.emit_list(root_offset(), reference)?;
        self.root_initialized = true;
        Ok(PointerListBuilder {
            arena: self,
            reference,
        })
    }

    pub fn init_root_struct_list(
        &mut self,
        element_count: u32,
        data_words: u16,
        pointer_count: u16,
    ) -> Result<StructListBuilder<'_>, ArenaError> {
        self.require_uninitialized_root()?;
        let reference = self.allocate_struct_list(element_count, data_words, pointer_count)?;
        self.emit_list(root_offset(), reference)?;
        self.root_initialized = true;
        Ok(StructListBuilder {
            arena: self,
            reference,
        })
    }

    #[inline]
    fn require_uninitialized_root(&self) -> Result<(), ArenaError> {
        if self.root_initialized {
            return Err(ArenaError::AlreadyInitialized);
        }
        Ok(())
    }

    #[inline(always)]
    fn allocate_words(&mut self, words: u64) -> Result<WordOffset, ArenaError> {
        let words_u32 = u32::try_from(words).map_err(|_| ArenaError::AllocationOverflow)?;
        if words_u32 > MAX_SINGLE_SEGMENT_WORDS {
            return Err(ArenaError::AllocationOverflow);
        }
        let last = u32::try_from(self.additional_segments.len())
            .map_err(|_| ArenaError::AllocationOverflow)?;
        if let Some(location) = self.try_allocate_in_segment(last, words_u32)? {
            return Ok(location);
        }
        self.allocate_new_segment(words_u32)
    }

    #[inline(always)]
    fn try_allocate_in_segment(
        &mut self,
        segment_id: u32,
        words: u32,
    ) -> Result<Option<WordOffset>, ArenaError> {
        let index = usize::try_from(segment_id).map_err(|_| ArenaError::AllocationOverflow)?;
        let segment = if index == 0 {
            &self.first_segment
        } else {
            self.additional_segments
                .get(index - 1)
                .ok_or(ArenaError::AllocationOverflow)?
        };
        let current =
            u32::try_from(segment.used_bytes / 8).map_err(|_| ArenaError::AllocationOverflow)?;
        let end = current
            .checked_add(words)
            .ok_or(ArenaError::AllocationOverflow)?;
        if end > segment.word_limit {
            return Ok(None);
        }
        self.ensure_total_limit(u64::from(words))?;
        let new_len = word_bytes(end)?;
        let segment = if index == 0 {
            &mut self.first_segment
        } else {
            self.additional_segments
                .get_mut(index - 1)
                .ok_or(ArenaError::AllocationOverflow)?
        };
        if new_len > segment.bytes.len() {
            segment
                .bytes
                .try_reserve_exact(new_len - segment.bytes.len())
                .map_err(|_| ArenaError::AllocationFailed)?;
            segment.bytes.resize(new_len, 0);
        }
        segment.used_bytes = new_len;
        Ok(Some(WordOffset {
            segment_id,
            word_offset: current,
        }))
    }

    #[inline]
    fn allocate_new_segment(&mut self, words: u32) -> Result<WordOffset, ArenaError> {
        self.ensure_total_limit(u64::from(words))?;
        let requested_segments = u32::try_from(self.additional_segments.len() + 1)
            .map_err(|_| ArenaError::AllocationOverflow)?
            .checked_add(1)
            .ok_or(ArenaError::AllocationOverflow)?;
        if requested_segments > self.max_segments {
            return Err(ArenaError::SegmentLimit {
                requested: requested_segments,
                limit: self.max_segments,
            });
        }
        let word_limit = self.next_segment_words.max(words);
        validate_segment_words(word_limit)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(word_bytes(words)?)
            .map_err(|_| ArenaError::AllocationFailed)?;
        bytes.resize(word_bytes(words)?, 0);
        let segment_id = u32::try_from(self.additional_segments.len() + 1)
            .map_err(|_| ArenaError::AllocationOverflow)?;
        self.additional_segments.push(SegmentStorage {
            bytes,
            used_bytes: word_bytes(words)?,
            word_limit,
        });
        Ok(WordOffset {
            segment_id,
            word_offset: 0,
        })
    }

    #[inline]
    fn ensure_total_limit(&self, additional: u64) -> Result<(), ArenaError> {
        let requested = self
            .word_len()
            .checked_add(additional)
            .ok_or(ArenaError::AllocationOverflow)?;
        if requested > self.max_total_words {
            Err(ArenaError::AllocationLimit {
                requested,
                limit: self.max_total_words,
            })
        } else {
            Ok(())
        }
    }

    #[inline]
    fn allocate_struct(
        &mut self,
        data_words: u16,
        pointer_count: u16,
    ) -> Result<StructOffset, ArenaError> {
        let words = u64::from(data_words) + u64::from(pointer_count);
        let content = self.allocate_words(words)?;
        Ok(StructOffset {
            arena_id: self.arena_id,
            content,
            data_words,
            pointer_count,
        })
    }

    fn allocate_data_list(
        &mut self,
        element_size: ElementSize,
        element_count: u32,
    ) -> Result<ListOffset, ArenaError> {
        if element_size == ElementSize::InlineComposite {
            return Err(ArenaError::AllocationOverflow);
        }
        WirePointer::new_list(0, element_size, element_count)?;
        let words = list_words(element_size, element_count)?;
        let content = self.allocate_words(u64::from(words))?;
        Ok(ListOffset {
            arena_id: self.arena_id,
            pointer_target: content,
            content,
            element_size,
            element_count,
            content_words: words,
            inline_struct_size: None,
        })
    }

    fn allocate_struct_list(
        &mut self,
        element_count: u32,
        data_words: u16,
        pointer_count: u16,
    ) -> Result<ListOffset, ArenaError> {
        let step = u64::from(data_words) + u64::from(pointer_count);
        let content_words = u64::from(element_count)
            .checked_mul(step)
            .ok_or(ArenaError::AllocationOverflow)?;
        let content_words_u32 =
            u32::try_from(content_words).map_err(|_| ArenaError::AllocationOverflow)?;
        WirePointer::new_list(0, ElementSize::InlineComposite, content_words_u32)?;
        let tag = WirePointer::new_inline_composite_tag(element_count, data_words, pointer_count)?;
        let pointer_target = self.allocate_words(
            content_words
                .checked_add(1)
                .ok_or(ArenaError::AllocationOverflow)?,
        )?;
        self.write_pointer(pointer_target, tag)?;
        let content = add_words(pointer_target, 1)?;
        Ok(ListOffset {
            arena_id: self.arena_id,
            pointer_target,
            content,
            element_size: ElementSize::InlineComposite,
            element_count,
            content_words: content_words_u32,
            inline_struct_size: Some((data_words, pointer_count)),
        })
    }

    #[inline]
    fn emit_struct(
        &mut self,
        pointer_location: WordOffset,
        target: StructOffset,
    ) -> Result<(), ArenaError> {
        if target.data_words == 0 && target.pointer_count == 0 {
            return self.write_pointer(pointer_location, WirePointer::empty_struct());
        }
        if pointer_location.segment_id == target.content.segment_id {
            let pointer = WirePointer::new_struct(
                relative_offset(pointer_location, target.content)?,
                target.data_words,
                target.pointer_count,
            )?;
            return self.write_pointer(pointer_location, pointer);
        }
        self.emit_far(
            pointer_location,
            target.content,
            FarTag::Struct {
                data_words: target.data_words,
                pointer_count: target.pointer_count,
            },
        )
    }

    fn emit_list(
        &mut self,
        pointer_location: WordOffset,
        target: ListOffset,
    ) -> Result<(), ArenaError> {
        let count = if target.element_size == ElementSize::InlineComposite {
            target.content_words
        } else {
            target.element_count
        };
        if pointer_location.segment_id == target.pointer_target.segment_id {
            let pointer = WirePointer::new_list(
                relative_offset(pointer_location, target.pointer_target)?,
                target.element_size,
                count,
            )?;
            return self.write_pointer(pointer_location, pointer);
        }
        self.emit_far(
            pointer_location,
            target.pointer_target,
            FarTag::List {
                element_size: target.element_size,
                count,
            },
        )
    }

    #[inline]
    fn emit_far(
        &mut self,
        pointer_location: WordOffset,
        object: WordOffset,
        tag: FarTag,
    ) -> Result<(), ArenaError> {
        if let Some(pad) = self.try_allocate_in_segment(object.segment_id, 1)? {
            self.write_pointer(pad, tag.positional(pad, object)?)?;
            return self.write_pointer(
                pointer_location,
                WirePointer::new_far(false, pad.word_offset, pad.segment_id)?,
            );
        }

        let pad = self.allocate_words(2)?;
        let tag_location = add_words(pad, 1)?;
        self.write_pointer(
            pad,
            WirePointer::new_far(false, object.word_offset, object.segment_id)?,
        )?;
        self.write_pointer(tag_location, tag.double_far_tag()?)?;
        self.write_pointer(
            pointer_location,
            WirePointer::new_far(true, pad.word_offset, pad.segment_id)?,
        )
    }

    #[inline(always)]
    fn write_pointer(
        &mut self,
        location: WordOffset,
        pointer: WirePointer,
    ) -> Result<(), ArenaError> {
        let segment = self.segment_mut(location.segment_id)?;
        pointer.write_to(segment, byte_offset(location)?)?;
        Ok(())
    }

    /// Copies an arbitrary validated source root without schema knowledge.
    pub fn copy_root<B: TraversalBudget>(
        &mut self,
        source: &MessageSegments<'_>,
        source_location: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<(), GraphError> {
        self.require_uninitialized_root()?;
        self.copy_pointer_at(root_offset(), source, source_location, budget, nesting)?;
        self.root_initialized = true;
        Ok(())
    }

    pub(crate) fn canonicalize_from<B: TraversalBudget>(
        source: &MessageSegments<'_>,
        budget: &B,
        nesting: NestingLimit,
        max_output_words: u32,
    ) -> Result<Box<[u8]>, GraphError> {
        let mut arena = Self::new(1, max_output_words)?;
        arena.canonical_copy_tasks(
            CopyTasks::new(CopyTask {
                destination: root_offset(),
                source: WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                nesting,
            }),
            source,
            budget,
        )?;
        arena.root_initialized = true;
        arena.into_segment().map_err(GraphError::from)
    }

    /// Clears the complete root graph only after a bounded traversal succeeds.
    pub fn clear_root<B: TraversalBudget>(
        &mut self,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<(), GraphError> {
        self.clear_pointer_at(root_offset(), budget, nesting)?;
        self.root_initialized = false;
        Ok(())
    }

    fn copy_pointer_at<B: TraversalBudget>(
        &mut self,
        destination: WordOffset,
        source: &MessageSegments<'_>,
        source_location: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<(), GraphError> {
        if !self.read_pointer(destination)?.is_null() {
            return Err(GraphError::DestinationNotNull {
                location: wire_location(destination),
            });
        }
        let checkpoint = self.checkpoint();
        let result = self.copy_tasks(
            CopyTasks::new(CopyTask {
                destination,
                source: source_location,
                nesting,
            }),
            source,
            budget,
        );
        if let Err(error) = result {
            self.rollback(checkpoint);
            self.write_pointer(destination, WirePointer::NULL)?;
            return Err(error);
        }
        Ok(())
    }

    fn copy_tasks<B: TraversalBudget>(
        &mut self,
        mut tasks: CopyTasks,
        source: &MessageSegments<'_>,
        budget: &B,
    ) -> Result<(), GraphError> {
        while let Some(task) = tasks.pop() {
            if task.source != WireLocation::ROOT {
                match source.try_read_byte_list_fast(task.source, budget, task.nesting) {
                    FastByteList::Null => {
                        self.write_pointer(task.destination, WirePointer::NULL)?;
                        continue;
                    }
                    FastByteList::Bytes(bytes) => {
                        let count = u32::try_from(bytes.len())
                            .map_err(|_| ArenaError::AllocationOverflow)?;
                        let target = self.allocate_data_list(ElementSize::Byte, count)?;
                        self.emit_list(task.destination, target)?;
                        let start = byte_offset(target.content)?;
                        let end = start
                            .checked_add(bytes.len())
                            .ok_or(ArenaError::AllocationOverflow)?;
                        self.segment_mut(target.content.segment_id)?[start..end]
                            .copy_from_slice(bytes);
                        continue;
                    }
                    FastByteList::Slow => {}
                }
            }
            let bounded = if task.source == WireLocation::ROOT {
                source.validate_root_struct_pointer_with_limits(budget, task.nesting)?
            } else {
                source.validate_pointer_with_limits(task.source, budget, task.nesting)?
            };
            match bounded.pointer {
                ResolvedPointer::Null => self.write_pointer(task.destination, WirePointer::NULL)?,
                ResolvedPointer::Capability(capability) => self.write_pointer(
                    task.destination,
                    WirePointer::new_capability(capability.index),
                )?,
                ResolvedPointer::Struct(reference) => {
                    let target =
                        self.allocate_struct(reference.data_words, reference.pointer_count)?;
                    self.emit_struct(task.destination, target)?;
                    self.copy_words_from(
                        target.content,
                        source,
                        reference.content,
                        u64::from(reference.data_words),
                    )?;
                    for index in (0..reference.pointer_count).rev() {
                        tasks.push(CopyTask {
                            destination: add_words(
                                target.content,
                                u64::from(reference.data_words) + u64::from(index),
                            )?,
                            source: add_wire_words(
                                reference.content,
                                u64::from(reference.data_words) + u64::from(index),
                            )?,
                            nesting: bounded.child_nesting,
                        });
                    }
                }
                ResolvedPointer::List(reference) => {
                    self.copy_list(
                        task.destination,
                        reference,
                        bounded.child_nesting,
                        source,
                        &mut tasks,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn canonical_copy_tasks<B: TraversalBudget>(
        &mut self,
        mut tasks: CopyTasks,
        source: &MessageSegments<'_>,
        budget: &B,
    ) -> Result<(), GraphError> {
        while let Some(task) = tasks.pop() {
            let bounded = source.validate_pointer_with_limits(task.source, budget, task.nesting)?;
            match bounded.pointer {
                ResolvedPointer::Null => self.write_pointer(task.destination, WirePointer::NULL)?,
                ResolvedPointer::Capability(capability) => {
                    return Err(GraphError::CapabilityNotCanonicalizable {
                        index: capability.index,
                    });
                }
                ResolvedPointer::Struct(reference) => {
                    let mut data_words = reference.data_words;
                    while data_words != 0
                        && source_word_is_zero(
                            source,
                            add_wire_words(reference.content, u64::from(data_words - 1))?,
                        )?
                    {
                        data_words -= 1;
                    }
                    let mut pointer_count = reference.pointer_count;
                    while pointer_count != 0
                        && read_message_pointer(
                            source,
                            add_wire_words(
                                reference.content,
                                u64::from(reference.data_words) + u64::from(pointer_count - 1),
                            )?,
                        )?
                        .is_null()
                    {
                        pointer_count -= 1;
                    }

                    let target = self.allocate_struct(data_words, pointer_count)?;
                    self.emit_struct(task.destination, target)?;
                    self.copy_words_from(
                        target.content,
                        source,
                        reference.content,
                        u64::from(data_words),
                    )?;
                    for index in (0..pointer_count).rev() {
                        tasks.push(CopyTask {
                            destination: add_words(
                                target.content,
                                u64::from(data_words) + u64::from(index),
                            )?,
                            source: add_wire_words(
                                reference.content,
                                u64::from(reference.data_words) + u64::from(index),
                            )?,
                            nesting: bounded.child_nesting,
                        });
                    }
                }
                ResolvedPointer::List(reference) => self.canonical_copy_list(
                    task.destination,
                    reference,
                    bounded.child_nesting,
                    source,
                    &mut tasks,
                )?,
            }
        }
        Ok(())
    }

    fn canonical_copy_list(
        &mut self,
        destination: WordOffset,
        reference: ListRef,
        nesting: NestingLimit,
        source: &MessageSegments<'_>,
        tasks: &mut CopyTasks,
    ) -> Result<(), GraphError> {
        if reference.element_size != ElementSize::InlineComposite {
            let target =
                self.allocate_data_list(reference.element_size, reference.element_count)?;
            self.emit_list(destination, target)?;
            if reference.element_size == ElementSize::Pointer {
                for index in (0..reference.element_count).rev() {
                    tasks.push(CopyTask {
                        destination: add_words(target.content, u64::from(index))?,
                        source: add_wire_words(reference.content, u64::from(index))?,
                        nesting,
                    });
                }
            } else {
                self.copy_words_from(
                    target.content,
                    source,
                    reference.content,
                    u64::from(reference.content_words),
                )?;
                self.clear_primitive_padding(
                    target.content,
                    reference.element_size,
                    reference.element_count,
                )?;
            }
            return Ok(());
        }

        let (source_data_words, source_pointer_count) = reference
            .inline_struct_size
            .ok_or(ArenaError::AllocationOverflow)?;
        let source_step = u64::from(source_data_words) + u64::from(source_pointer_count);
        let mut data_words = source_data_words;
        while data_words != 0 {
            let mut any_nonzero = false;
            for element in 0..reference.element_count {
                let offset = u64::from(element)
                    .checked_mul(source_step)
                    .and_then(|value| value.checked_add(u64::from(data_words - 1)))
                    .ok_or(ArenaError::AllocationOverflow)?;
                any_nonzero |=
                    !source_word_is_zero(source, add_wire_words(reference.content, offset)?)?;
            }
            if any_nonzero {
                break;
            }
            data_words -= 1;
        }
        let mut pointer_count = source_pointer_count;
        while pointer_count != 0 {
            let mut any_nonnull = false;
            for element in 0..reference.element_count {
                let offset = u64::from(element)
                    .checked_mul(source_step)
                    .and_then(|value| value.checked_add(u64::from(source_data_words)))
                    .and_then(|value| value.checked_add(u64::from(pointer_count - 1)))
                    .ok_or(ArenaError::AllocationOverflow)?;
                any_nonnull |=
                    !read_message_pointer(source, add_wire_words(reference.content, offset)?)?
                        .is_null();
            }
            if any_nonnull {
                break;
            }
            pointer_count -= 1;
        }

        let target =
            self.allocate_struct_list(reference.element_count, data_words, pointer_count)?;
        self.emit_list(destination, target)?;
        let target_step = u64::from(data_words) + u64::from(pointer_count);
        for element in 0..reference.element_count {
            let source_element = u64::from(element)
                .checked_mul(source_step)
                .ok_or(ArenaError::AllocationOverflow)?;
            let target_element = u64::from(element)
                .checked_mul(target_step)
                .ok_or(ArenaError::AllocationOverflow)?;
            self.copy_words_from(
                add_words(target.content, target_element)?,
                source,
                add_wire_words(reference.content, source_element)?,
                u64::from(data_words),
            )?;
        }
        let child_nesting = nesting.descend().map_err(TraversalError::from)?;
        for element in (0..reference.element_count).rev() {
            let source_element = u64::from(element)
                .checked_mul(source_step)
                .ok_or(ArenaError::AllocationOverflow)?;
            let target_element = u64::from(element)
                .checked_mul(target_step)
                .ok_or(ArenaError::AllocationOverflow)?;
            for pointer in (0..pointer_count).rev() {
                tasks.push(CopyTask {
                    destination: add_words(
                        target.content,
                        target_element + u64::from(data_words) + u64::from(pointer),
                    )?,
                    source: add_wire_words(
                        reference.content,
                        source_element + u64::from(source_data_words) + u64::from(pointer),
                    )?,
                    nesting: child_nesting,
                });
            }
        }
        Ok(())
    }

    fn clear_primitive_padding(
        &mut self,
        content: WordOffset,
        element_size: ElementSize,
        element_count: u32,
    ) -> Result<(), ArenaError> {
        let bit_count = u64::from(element_count)
            .checked_mul(element_bits(element_size))
            .ok_or(ArenaError::AllocationOverflow)?;
        let content_words = u64::from(list_words(element_size, element_count)?);
        let total_bytes = usize::try_from(content_words)
            .ok()
            .and_then(|words| words.checked_mul(8))
            .ok_or(ArenaError::AllocationOverflow)?;
        let used_bytes =
            usize::try_from(bit_count.div_ceil(8)).map_err(|_| ArenaError::AllocationOverflow)?;
        let start = byte_offset(content)?;
        let segment = self.segment_mut(content.segment_id)?;
        if bit_count % 8 != 0 {
            let keep = (1_u8 << (bit_count % 8)) - 1;
            segment[start + used_bytes - 1] &= keep;
        }
        segment[start + used_bytes..start + total_bytes].fill(0);
        Ok(())
    }

    fn copy_list(
        &mut self,
        destination: WordOffset,
        reference: ListRef,
        nesting: NestingLimit,
        source: &MessageSegments<'_>,
        tasks: &mut CopyTasks,
    ) -> Result<(), GraphError> {
        if reference.element_size != ElementSize::InlineComposite {
            let target =
                self.allocate_data_list(reference.element_size, reference.element_count)?;
            self.emit_list(destination, target)?;
            if reference.element_size == ElementSize::Pointer {
                for index in (0..reference.element_count).rev() {
                    tasks.push(CopyTask {
                        destination: add_words(target.content, u64::from(index))?,
                        source: add_wire_words(reference.content, u64::from(index))?,
                        nesting,
                    });
                }
            } else {
                self.copy_words_from(
                    target.content,
                    source,
                    reference.content,
                    u64::from(reference.content_words),
                )?;
            }
            return Ok(());
        }

        let (data_words, pointer_count) = reference
            .inline_struct_size
            .ok_or(ArenaError::AllocationOverflow)?;
        let target =
            self.allocate_struct_list(reference.element_count, data_words, pointer_count)?;
        self.emit_list(destination, target)?;
        let step = u64::from(data_words) + u64::from(pointer_count);
        let child_nesting = nesting.descend().map_err(TraversalError::from)?;
        for element in 0..reference.element_count {
            let element_offset = u64::from(element)
                .checked_mul(step)
                .ok_or(ArenaError::AllocationOverflow)?;
            self.copy_words_from(
                add_words(target.content, element_offset)?,
                source,
                add_wire_words(reference.content, element_offset)?,
                u64::from(data_words),
            )?;
            for pointer in (0..pointer_count).rev() {
                let pointer_offset = element_offset
                    .checked_add(u64::from(data_words))
                    .and_then(|value| value.checked_add(u64::from(pointer)))
                    .ok_or(ArenaError::AllocationOverflow)?;
                tasks.push(CopyTask {
                    destination: add_words(target.content, pointer_offset)?,
                    source: add_wire_words(reference.content, pointer_offset)?,
                    nesting: child_nesting,
                });
            }
        }
        Ok(())
    }

    fn copy_words_from(
        &mut self,
        destination: WordOffset,
        source: &MessageSegments<'_>,
        source_location: WireLocation,
        words: u64,
    ) -> Result<(), GraphError> {
        if words == 0 {
            return Ok(());
        }
        let source_segment =
            source
                .segment(source_location.segment_id)
                .ok_or(ValidationError::UnknownSegment {
                    segment_id: source_location.segment_id,
                })?;
        let source_start = wire_byte_offset(source_location)?;
        let len = usize::try_from(words)
            .ok()
            .and_then(|words| words.checked_mul(8))
            .ok_or(ArenaError::AllocationOverflow)?;
        let source_end = source_start
            .checked_add(len)
            .ok_or(ArenaError::AllocationOverflow)?;
        let source_bytes = source_segment.get(source_start..source_end).ok_or(
            ValidationError::ObjectOutOfBounds {
                location: source_location,
                words,
                segment_words: u64::try_from(source_segment.len() / 8)
                    .map_err(|_| ArenaError::AllocationOverflow)?,
            },
        )?;
        let destination_start = byte_offset(destination)?;
        let destination_end = destination_start
            .checked_add(len)
            .ok_or(ArenaError::AllocationOverflow)?;
        self.segment_mut(destination.segment_id)?[destination_start..destination_end]
            .copy_from_slice(source_bytes);
        Ok(())
    }

    fn clear_pointer_at<B: TraversalBudget>(
        &mut self,
        root: WordOffset,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<(), GraphError> {
        let borrowed = self.segments().collect::<Vec<_>>();
        let message = MessageSegments::new(&borrowed)?;
        let mut tasks = vec![(wire_location(root), nesting)];
        let mut ranges = Vec::new();
        while let Some((location, nesting)) = tasks.pop() {
            let raw = read_message_pointer(&message, location)?;
            ranges.push((location, 1));
            if let Some(far) = raw.far_fields() {
                ranges.push((
                    WireLocation {
                        segment_id: far.segment_id,
                        word_offset: far.landing_pad_word,
                    },
                    if far.double_far { 2 } else { 1 },
                ));
            }
            let bounded = message.validate_pointer_with_limits(location, budget, nesting)?;
            match bounded.pointer {
                ResolvedPointer::Null | ResolvedPointer::Capability(_) => {}
                ResolvedPointer::Struct(reference) => {
                    ranges.push((
                        reference.content,
                        u64::from(reference.data_words) + u64::from(reference.pointer_count),
                    ));
                    for index in 0..reference.pointer_count {
                        tasks.push((
                            add_wire_words(
                                reference.content,
                                u64::from(reference.data_words) + u64::from(index),
                            )?,
                            bounded.child_nesting,
                        ));
                    }
                }
                ResolvedPointer::List(reference) => {
                    if reference.element_size == ElementSize::InlineComposite {
                        let tag = sub_wire_word(reference.content)?;
                        ranges.push((tag, u64::from(reference.content_words) + 1));
                        let (data_words, pointer_count) = reference
                            .inline_struct_size
                            .ok_or(ArenaError::AllocationOverflow)?;
                        let step = u64::from(data_words) + u64::from(pointer_count);
                        let child_nesting = bounded
                            .child_nesting
                            .descend()
                            .map_err(TraversalError::from)?;
                        for element in 0..reference.element_count {
                            let base = u64::from(element)
                                .checked_mul(step)
                                .and_then(|value| value.checked_add(u64::from(data_words)))
                                .ok_or(ArenaError::AllocationOverflow)?;
                            for pointer in 0..pointer_count {
                                tasks.push((
                                    add_wire_words(reference.content, base + u64::from(pointer))?,
                                    child_nesting,
                                ));
                            }
                        }
                    } else {
                        ranges.push((reference.content, u64::from(reference.content_words)));
                        if reference.element_size == ElementSize::Pointer {
                            for index in 0..reference.element_count {
                                tasks.push((
                                    add_wire_words(reference.content, u64::from(index))?,
                                    bounded.child_nesting,
                                ));
                            }
                        }
                    }
                }
            }
        }
        drop(message);
        drop(borrowed);
        for (location, words) in ranges {
            self.zero_range(location, words)?;
        }
        Ok(())
    }

    fn read_pointer(&self, location: WordOffset) -> Result<WirePointer, ArenaError> {
        let segment = self
            .segment(location.segment_id)
            .ok_or(ArenaError::AllocationOverflow)?;
        Ok(WirePointer::read_from(segment, byte_offset(location)?)?)
    }

    fn zero_range(&mut self, location: WireLocation, words: u64) -> Result<(), ArenaError> {
        let start = wire_byte_offset(location)?;
        let len = usize::try_from(words)
            .ok()
            .and_then(|words| words.checked_mul(8))
            .ok_or(ArenaError::AllocationOverflow)?;
        let end = start
            .checked_add(len)
            .ok_or(ArenaError::AllocationOverflow)?;
        self.segment_mut(location.segment_id)?[start..end].fill(0);
        Ok(())
    }

    fn checkpoint(&self) -> ArenaCheckpoint {
        ArenaCheckpoint {
            first_length: self.first_segment.used_bytes,
            additional_lengths: self
                .additional_segments
                .iter()
                .map(|segment| segment.used_bytes)
                .collect(),
        }
    }

    fn rollback(&mut self, checkpoint: ArenaCheckpoint) {
        self.first_segment.bytes[checkpoint.first_length..self.first_segment.used_bytes].fill(0);
        self.first_segment.used_bytes = checkpoint.first_length;
        for (index, segment) in self.additional_segments.iter_mut().enumerate() {
            let Some(length) = checkpoint.additional_lengths.get(index).copied() else {
                segment.used_mut().fill(0);
                continue;
            };
            segment.bytes[length..segment.used_bytes].fill(0);
            segment.used_bytes = length;
        }
        self.additional_segments
            .truncate(checkpoint.additional_lengths.len());
    }

    #[inline]
    fn segment_mut(&mut self, segment_id: u32) -> Result<&mut [u8], ArenaError> {
        if segment_id == 0 {
            return Ok(self.first_segment.used_mut());
        }
        usize::try_from(segment_id - 1)
            .ok()
            .and_then(|index| self.additional_segments.get_mut(index))
            .map(SegmentStorage::used_mut)
            .ok_or(ArenaError::AllocationOverflow)
    }

    fn detach_pointer(&mut self, location: WordOffset) -> Result<Detached, GraphError> {
        let borrowed = self.segments().collect::<Vec<_>>();
        let message = MessageSegments::new(&borrowed)?;
        let raw = read_message_pointer(&message, wire_location(location))?;
        let resolved = message.validate_pointer(wire_location(location))?;
        let detached = self.detached_from_resolved(resolved)?;
        drop(message);
        drop(borrowed);
        self.zero_pointer_and_pads(location, raw)?;
        Ok(detached)
    }

    fn resolved_pointer(&self, location: WordOffset) -> Result<ResolvedPointer, GraphError> {
        let borrowed = self.segments().collect::<Vec<_>>();
        let message = MessageSegments::new(&borrowed)?;
        Ok(message.validate_pointer(wire_location(location))?)
    }

    fn detached_from_resolved(&self, resolved: ResolvedPointer) -> Result<Detached, GraphError> {
        Ok(match resolved {
            ResolvedPointer::Null => Detached::Null,
            ResolvedPointer::Struct(reference) => Detached::Struct(StructOffset {
                arena_id: self.arena_id,
                content: word_offset(reference.content),
                data_words: reference.data_words,
                pointer_count: reference.pointer_count,
            }),
            ResolvedPointer::List(reference) => {
                let pointer_target = if reference.element_size == ElementSize::InlineComposite {
                    sub_word(word_offset(reference.content))?
                } else {
                    word_offset(reference.content)
                };
                Detached::List(ListOffset {
                    arena_id: self.arena_id,
                    pointer_target,
                    content: word_offset(reference.content),
                    element_size: reference.element_size,
                    element_count: reference.element_count,
                    content_words: reference.content_words,
                    inline_struct_size: reference.inline_struct_size,
                })
            }
            ResolvedPointer::Capability(_) => Detached::Null,
        })
    }

    fn zero_pointer_and_pads(
        &mut self,
        location: WordOffset,
        raw: WirePointer,
    ) -> Result<(), ArenaError> {
        self.write_pointer(location, WirePointer::NULL)?;
        if let Some(far) = raw.far_fields() {
            self.zero_range(
                WireLocation {
                    segment_id: far.segment_id,
                    word_offset: far.landing_pad_word,
                },
                if far.double_far { 2 } else { 1 },
            )?;
        }
        Ok(())
    }

    fn abandon_detached(&mut self, detached: Detached) -> Result<(), GraphError> {
        let mut tasks = vec![AbandonTask::Object(detached)];
        while let Some(task) = tasks.pop() {
            match task {
                AbandonTask::Pointer(location) => {
                    let borrowed = self.segments().collect::<Vec<_>>();
                    let message = MessageSegments::new(&borrowed)?;
                    let raw = read_message_pointer(&message, wire_location(location))?;
                    let resolved = message.validate_pointer(wire_location(location))?;
                    let detached = self.detached_from_resolved(resolved)?;
                    drop(message);
                    drop(borrowed);
                    self.zero_pointer_and_pads(location, raw)?;
                    tasks.push(AbandonTask::Object(detached));
                }
                AbandonTask::Object(Detached::Null) => {}
                AbandonTask::Object(Detached::Struct(reference)) => {
                    let words =
                        u64::from(reference.data_words) + u64::from(reference.pointer_count);
                    tasks.push(AbandonTask::Zero(reference.content, words));
                    for index in 0..reference.pointer_count {
                        tasks.push(AbandonTask::Pointer(add_words(
                            reference.content,
                            u64::from(reference.data_words) + u64::from(index),
                        )?));
                    }
                }
                AbandonTask::Object(Detached::List(reference)) => {
                    if reference.element_size == ElementSize::InlineComposite {
                        tasks.push(AbandonTask::Zero(
                            reference.pointer_target,
                            u64::from(reference.content_words) + 1,
                        ));
                        let (data_words, pointer_count) = reference
                            .inline_struct_size
                            .ok_or(ArenaError::AllocationOverflow)?;
                        let step = u64::from(data_words) + u64::from(pointer_count);
                        for element in 0..reference.element_count {
                            let base = u64::from(element)
                                .checked_mul(step)
                                .and_then(|value| value.checked_add(u64::from(data_words)))
                                .ok_or(ArenaError::AllocationOverflow)?;
                            for pointer in 0..pointer_count {
                                tasks.push(AbandonTask::Pointer(add_words(
                                    reference.content,
                                    base + u64::from(pointer),
                                )?));
                            }
                        }
                    } else {
                        tasks.push(AbandonTask::Zero(
                            reference.content,
                            u64::from(reference.content_words),
                        ));
                        if reference.element_size == ElementSize::Pointer {
                            for index in 0..reference.element_count {
                                tasks.push(AbandonTask::Pointer(add_words(
                                    reference.content,
                                    u64::from(index),
                                )?));
                            }
                        }
                    }
                }
                AbandonTask::Zero(location, words) => {
                    self.zero_range(wire_location(location), words)?;
                }
            }
        }
        Ok(())
    }
}

struct ArenaCheckpoint {
    first_length: usize,
    additional_lengths: Vec<usize>,
}

#[derive(Clone, Copy)]
struct CopyTask {
    destination: WordOffset,
    source: WireLocation,
    nesting: NestingLimit,
}

struct CopyTasks {
    first: Option<CopyTask>,
    overflow: Vec<CopyTask>,
}

impl CopyTasks {
    fn new(task: CopyTask) -> Self {
        Self {
            first: Some(task),
            overflow: Vec::new(),
        }
    }

    fn push(&mut self, task: CopyTask) {
        if self.first.is_none() {
            self.first = Some(task);
        } else {
            self.overflow.push(task);
        }
    }

    fn pop(&mut self) -> Option<CopyTask> {
        self.overflow.pop().or_else(|| self.first.take())
    }
}

#[derive(Clone, Copy)]
enum Detached {
    Null,
    Struct(StructOffset),
    List(ListOffset),
}

enum AbandonTask {
    Pointer(WordOffset),
    Object(Detached),
    Zero(WordOffset, u64),
}

mod orphan_sealed {
    pub trait Sealed {}
}

pub trait OrphanKind: orphan_sealed::Sealed {}

#[derive(Debug)]
pub struct StructOrphan;
impl orphan_sealed::Sealed for StructOrphan {}
impl OrphanKind for StructOrphan {}

#[derive(Debug)]
pub struct ListOrphan;
impl orphan_sealed::Sealed for ListOrphan {}
impl OrphanKind for ListOrphan {}

/// A detached object that exclusively borrows its arena until adopted or dropped.
///
/// Dropping an orphan recursively zeroes its unreachable storage. While it is
/// live, the original builder cannot be aliased:
///
/// ```compile_fail
/// use capnp_message::ExclusiveArena;
/// let mut arena = ExclusiveArena::new(1, 16).unwrap();
/// let mut root = arena.init_root_struct(0, 2).unwrap();
/// root.init_struct(0, 1, 0).unwrap();
/// let orphan = root.disown_struct(0).unwrap();
/// root.init_struct(1, 1, 0).unwrap();
/// drop(orphan);
/// ```
#[must_use = "dropping an orphan safely clears its detached object"]
pub struct Orphan<'arena, T: OrphanKind> {
    arena: &'arena mut ExclusiveArena,
    detached: Detached,
    adopted: bool,
    marker: PhantomData<fn() -> T>,
}

impl<T: OrphanKind> fmt::Debug for Orphan<'_, T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Orphan")
            .field("adopted", &self.adopted)
            .finish_non_exhaustive()
    }
}

impl<T: OrphanKind> Orphan<'_, T> {
    pub const fn is_null(&self) -> bool {
        matches!(self.detached, Detached::Null)
    }

    fn destination_slot(
        &self,
        parent: StructOffset,
        pointer_index: u16,
    ) -> Result<WordOffset, GraphError> {
        if parent.arena_id != self.arena.arena_id {
            return Err(GraphError::WrongArena);
        }
        Ok(struct_pointer_slot(parent, pointer_index)?)
    }
}

impl Orphan<'_, StructOrphan> {
    pub fn adopt_into_struct(
        mut self,
        parent: StructOffset,
        pointer_index: u16,
    ) -> Result<(), GraphError> {
        let slot = self.destination_slot(parent, pointer_index)?;
        if !self.arena.read_pointer(slot)?.is_null() {
            return Err(GraphError::DestinationNotNull {
                location: wire_location(slot),
            });
        }
        match self.detached {
            Detached::Null => self.arena.write_pointer(slot, WirePointer::NULL)?,
            Detached::Struct(reference) => self.arena.emit_struct(slot, reference)?,
            Detached::List(_) => return Err(GraphError::ExpectedStruct),
        }
        self.adopted = true;
        Ok(())
    }
}

impl Orphan<'_, ListOrphan> {
    pub fn adopt_into_struct(
        mut self,
        parent: StructOffset,
        pointer_index: u16,
    ) -> Result<(), GraphError> {
        let slot = self.destination_slot(parent, pointer_index)?;
        if !self.arena.read_pointer(slot)?.is_null() {
            return Err(GraphError::DestinationNotNull {
                location: wire_location(slot),
            });
        }
        match self.detached {
            Detached::Null => self.arena.write_pointer(slot, WirePointer::NULL)?,
            Detached::List(reference) => self.arena.emit_list(slot, reference)?,
            Detached::Struct(_) => return Err(GraphError::ExpectedList),
        }
        self.adopted = true;
        Ok(())
    }
}

impl<T: OrphanKind> Drop for Orphan<'_, T> {
    fn drop(&mut self) {
        if !self.adopted {
            let _ = self.arena.abandon_detached(self.detached);
        }
    }
}

#[derive(Clone, Copy)]
enum FarTag {
    Struct {
        data_words: u16,
        pointer_count: u16,
    },
    List {
        element_size: ElementSize,
        count: u32,
    },
}

impl FarTag {
    fn positional(
        self,
        pointer: WordOffset,
        object: WordOffset,
    ) -> Result<WirePointer, ArenaError> {
        let offset = relative_offset(pointer, object)?;
        Ok(match self {
            Self::Struct {
                data_words,
                pointer_count,
            } => WirePointer::new_struct(offset, data_words, pointer_count)?,
            Self::List {
                element_size,
                count,
            } => WirePointer::new_list(offset, element_size, count)?,
        })
    }

    fn double_far_tag(self) -> Result<WirePointer, ArenaError> {
        Ok(match self {
            Self::Struct {
                data_words,
                pointer_count,
            } => WirePointer::new_struct(0, data_words, pointer_count)?,
            Self::List {
                element_size,
                count,
            } => WirePointer::new_list(0, element_size, count)?,
        })
    }
}

/// Exclusive view over one allocated struct.
///
/// Reborrowing a child statically prevents two live mutable builders:
///
/// ```compile_fail
/// use capnp_message::ExclusiveArena;
/// let mut arena = ExclusiveArena::new(1, 32).unwrap();
/// let mut root = arena.init_root_struct(0, 2).unwrap();
/// let left = root.init_struct(0, 0, 0).unwrap();
/// let right = root.init_struct(1, 0, 0).unwrap();
/// drop((left, right));
/// ```
pub struct StructBuilder<'arena> {
    arena: &'arena mut ExclusiveArena,
    reference: StructOffset,
}

impl StructBuilder<'_> {
    pub const fn offset(&self) -> StructOffset {
        self.reference
    }

    /// Reborrows the same storage for a schema group view.
    pub fn group(&mut self) -> StructBuilder<'_> {
        StructBuilder {
            arena: self.arena,
            reference: self.reference,
        }
    }

    pub fn set_bool(
        &mut self,
        bit_offset: u32,
        value: bool,
        default: bool,
    ) -> Result<(), ArenaError> {
        let absolute = self.data_bit_offset(bit_offset)?;
        let byte = usize::try_from(absolute / 8).map_err(|_| ArenaError::AllocationOverflow)?;
        let bit = u8::try_from(absolute % 8).map_err(|_| ArenaError::AllocationOverflow)?;
        let mask = 1u8 << bit;
        let segment = self.arena.segment_mut(self.reference.content.segment_id)?;
        if value ^ default {
            segment[byte] |= mask;
        } else {
            segment[byte] &= !mask;
        }
        Ok(())
    }

    pub fn set_u8(&mut self, offset: u32, value: u8, default: u8) -> Result<(), ArenaError> {
        let byte = self.data_element_offset(offset, 1)?;
        write_u8(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            byte,
            value ^ default,
        )?;
        Ok(())
    }

    pub fn set_i8(&mut self, offset: u32, value: i8, default: i8) -> Result<(), ArenaError> {
        let byte = self.data_element_offset(offset, 1)?;
        write_i8(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            byte,
            value ^ default,
        )?;
        Ok(())
    }

    pub fn set_u16(&mut self, offset: u32, value: u16, default: u16) -> Result<(), ArenaError> {
        let byte = self.data_element_offset(offset, 2)?;
        write_u16_le(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            byte,
            value ^ default,
        )?;
        Ok(())
    }

    pub fn set_i16(&mut self, offset: u32, value: i16, default: i16) -> Result<(), ArenaError> {
        let byte = self.data_element_offset(offset, 2)?;
        write_i16_le(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            byte,
            value ^ default,
        )?;
        Ok(())
    }

    pub fn set_u32(&mut self, offset: u32, value: u32, default: u32) -> Result<(), ArenaError> {
        let byte = self.data_element_offset(offset, 4)?;
        write_u32_le(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            byte,
            value ^ default,
        )?;
        Ok(())
    }

    pub fn set_i32(&mut self, offset: u32, value: i32, default: i32) -> Result<(), ArenaError> {
        let byte = self.data_element_offset(offset, 4)?;
        write_i32_le(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            byte,
            value ^ default,
        )?;
        Ok(())
    }

    #[inline]
    pub fn set_u64(&mut self, offset: u32, value: u64, default: u64) -> Result<(), ArenaError> {
        let byte = self.data_element_offset(offset, 8)?;
        write_u64_le(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            byte,
            value ^ default,
        )?;
        Ok(())
    }

    pub fn set_i64(&mut self, offset: u32, value: i64, default: i64) -> Result<(), ArenaError> {
        let byte = self.data_element_offset(offset, 8)?;
        write_i64_le(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            byte,
            value ^ default,
        )?;
        Ok(())
    }

    pub fn set_f32(&mut self, offset: u32, value: f32, default: f32) -> Result<(), ArenaError> {
        let byte = self.data_element_offset(offset, 4)?;
        write_f32_le(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            byte,
            f32::from_bits(value.to_bits() ^ default.to_bits()),
        )?;
        Ok(())
    }

    pub fn set_f64(&mut self, offset: u32, value: f64, default: f64) -> Result<(), ArenaError> {
        let byte = self.data_element_offset(offset, 8)?;
        write_f64_le(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            byte,
            f64::from_bits(value.to_bits() ^ default.to_bits()),
        )?;
        Ok(())
    }

    pub fn init_struct(
        &mut self,
        pointer_index: u16,
        data_words: u16,
        pointer_count: u16,
    ) -> Result<StructBuilder<'_>, ArenaError> {
        let slot = self.pointer_slot(pointer_index)?;
        let reference = self.arena.allocate_struct(data_words, pointer_count)?;
        self.arena.emit_struct(slot, reference)?;
        Ok(StructBuilder {
            arena: self.arena,
            reference,
        })
    }

    pub fn init_list<T: PrimitiveListValue>(
        &mut self,
        pointer_index: u16,
        element_count: u32,
    ) -> Result<DataListBuilder<'_, T>, ArenaError> {
        let slot = self.pointer_slot(pointer_index)?;
        let reference = self
            .arena
            .allocate_data_list(T::ELEMENT_SIZE, element_count)?;
        self.arena.emit_list(slot, reference)?;
        Ok(DataListBuilder {
            arena: self.arena,
            reference,
            marker: PhantomData,
        })
    }

    pub fn init_pointer_list(
        &mut self,
        pointer_index: u16,
        element_count: u32,
    ) -> Result<PointerListBuilder<'_>, ArenaError> {
        let slot = self.pointer_slot(pointer_index)?;
        let reference = self
            .arena
            .allocate_data_list(ElementSize::Pointer, element_count)?;
        self.arena.emit_list(slot, reference)?;
        Ok(PointerListBuilder {
            arena: self.arena,
            reference,
        })
    }

    pub fn init_struct_list(
        &mut self,
        pointer_index: u16,
        element_count: u32,
        data_words: u16,
        pointer_count: u16,
    ) -> Result<StructListBuilder<'_>, ArenaError> {
        let slot = self.pointer_slot(pointer_index)?;
        let reference =
            self.arena
                .allocate_struct_list(element_count, data_words, pointer_count)?;
        self.arena.emit_list(slot, reference)?;
        Ok(StructListBuilder {
            arena: self.arena,
            reference,
        })
    }

    pub fn set_text(&mut self, pointer_index: u16, value: &str) -> Result<(), ArenaError> {
        let count = value
            .len()
            .checked_add(1)
            .and_then(|count| u32::try_from(count).ok())
            .ok_or(ArenaError::AllocationOverflow)?;
        let slot = self.pointer_slot(pointer_index)?;
        let reference = self.arena.allocate_data_list(ElementSize::Byte, count)?;
        self.arena.emit_list(slot, reference)?;
        let start = byte_offset(reference.content)?;
        let end = start
            .checked_add(value.len())
            .ok_or(ArenaError::AllocationOverflow)?;
        self.arena.segment_mut(reference.content.segment_id)?[start..end]
            .copy_from_slice(value.as_bytes());
        Ok(())
    }

    pub fn set_data(&mut self, pointer_index: u16, value: &[u8]) -> Result<(), ArenaError> {
        let count = u32::try_from(value.len()).map_err(|_| ArenaError::AllocationOverflow)?;
        let slot = self.pointer_slot(pointer_index)?;
        let reference = self.arena.allocate_data_list(ElementSize::Byte, count)?;
        self.arena.emit_list(slot, reference)?;
        let start = byte_offset(reference.content)?;
        let end = start
            .checked_add(value.len())
            .ok_or(ArenaError::AllocationOverflow)?;
        self.arena.segment_mut(reference.content.segment_id)?[start..end].copy_from_slice(value);
        Ok(())
    }

    pub fn set_capability(&mut self, pointer_index: u16, index: u32) -> Result<(), ArenaError> {
        let slot = self.pointer_slot(pointer_index)?;
        self.arena
            .write_pointer(slot, WirePointer::new_capability(index))
    }

    pub fn copy_pointer<B: TraversalBudget>(
        &mut self,
        pointer_index: u16,
        source: &MessageSegments<'_>,
        source_location: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<(), GraphError> {
        let slot = self.pointer_slot(pointer_index)?;
        self.arena
            .copy_pointer_at(slot, source, source_location, budget, nesting)
    }

    pub fn clear_pointer<B: TraversalBudget>(
        &mut self,
        pointer_index: u16,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<(), GraphError> {
        let slot = self.pointer_slot(pointer_index)?;
        self.arena.clear_pointer_at(slot, budget, nesting)
    }

    pub fn disown_struct(
        &mut self,
        pointer_index: u16,
    ) -> Result<Orphan<'_, StructOrphan>, GraphError> {
        let slot = self.pointer_slot(pointer_index)?;
        match self.arena.resolved_pointer(slot)? {
            ResolvedPointer::Null | ResolvedPointer::Struct(_) => {}
            ResolvedPointer::List(_) | ResolvedPointer::Capability(_) => {
                return Err(GraphError::ExpectedStruct);
            }
        }
        let detached = self.arena.detach_pointer(slot)?;
        Ok(Orphan {
            arena: self.arena,
            detached,
            adopted: false,
            marker: PhantomData,
        })
    }

    pub fn disown_list(
        &mut self,
        pointer_index: u16,
    ) -> Result<Orphan<'_, ListOrphan>, GraphError> {
        let slot = self.pointer_slot(pointer_index)?;
        match self.arena.resolved_pointer(slot)? {
            ResolvedPointer::Null | ResolvedPointer::List(_) => {}
            ResolvedPointer::Struct(_) | ResolvedPointer::Capability(_) => {
                return Err(GraphError::ExpectedList);
            }
        }
        let detached = self.arena.detach_pointer(slot)?;
        Ok(Orphan {
            arena: self.arena,
            detached,
            adopted: false,
            marker: PhantomData,
        })
    }

    fn data_bit_offset(&self, bit_offset: u32) -> Result<u64, ArenaError> {
        let available = u64::from(self.reference.data_words) * 64;
        if u64::from(bit_offset) >= available {
            return Err(ArenaError::IndexOutOfBounds {
                index: bit_offset,
                len: u32::from(self.reference.data_words) * 64,
            });
        }
        u64::from(self.reference.content.word_offset)
            .checked_mul(64)
            .and_then(|start| start.checked_add(u64::from(bit_offset)))
            .ok_or(ArenaError::AllocationOverflow)
    }

    #[inline]
    fn data_element_offset(&self, offset: u32, width: u32) -> Result<usize, ArenaError> {
        let relative = u64::from(offset)
            .checked_mul(u64::from(width))
            .ok_or(ArenaError::AllocationOverflow)?;
        let end = relative
            .checked_add(u64::from(width))
            .ok_or(ArenaError::AllocationOverflow)?;
        let available = u64::from(self.reference.data_words) * 8;
        if end > available {
            return Err(ArenaError::IndexOutOfBounds {
                index: offset,
                len: u32::try_from(available / u64::from(width)).unwrap_or(u32::MAX),
            });
        }
        byte_offset(self.reference.content)?
            .checked_add(usize::try_from(relative).map_err(|_| ArenaError::AllocationOverflow)?)
            .ok_or(ArenaError::AllocationOverflow)
    }

    fn pointer_slot(&self, index: u16) -> Result<WordOffset, ArenaError> {
        struct_pointer_slot(self.reference, index)
    }
}

mod sealed {
    pub trait Sealed {}
}

pub trait PrimitiveListValue: sealed::Sealed + Copy {
    const ELEMENT_SIZE: ElementSize;
    #[doc(hidden)]
    fn write_at(bytes: &mut [u8], bit_offset: u64, value: Self) -> Result<(), ArenaError>;
}

impl sealed::Sealed for () {}
impl PrimitiveListValue for () {
    const ELEMENT_SIZE: ElementSize = ElementSize::Void;

    fn write_at(_bytes: &mut [u8], _bit_offset: u64, _value: Self) -> Result<(), ArenaError> {
        Ok(())
    }
}

impl sealed::Sealed for bool {}
impl PrimitiveListValue for bool {
    const ELEMENT_SIZE: ElementSize = ElementSize::Bit;

    fn write_at(bytes: &mut [u8], bit_offset: u64, value: Self) -> Result<(), ArenaError> {
        let byte = usize::try_from(bit_offset / 8).map_err(|_| ArenaError::AllocationOverflow)?;
        let bit = u8::try_from(bit_offset % 8).map_err(|_| ArenaError::AllocationOverflow)?;
        if value {
            bytes[byte] |= 1 << bit;
        } else {
            bytes[byte] &= !(1 << bit);
        }
        Ok(())
    }
}

macro_rules! list_value {
    ($ty:ty, $size:ident, $bits:expr, $write:ident) => {
        impl sealed::Sealed for $ty {}
        impl PrimitiveListValue for $ty {
            const ELEMENT_SIZE: ElementSize = ElementSize::$size;

            fn write_at(bytes: &mut [u8], bit_offset: u64, value: Self) -> Result<(), ArenaError> {
                let byte =
                    usize::try_from(bit_offset / 8).map_err(|_| ArenaError::AllocationOverflow)?;
                $write(bytes, byte, value)?;
                Ok(())
            }
        }
    };
}

list_value!(u8, Byte, 8, write_u8);
list_value!(i8, Byte, 8, write_i8);
list_value!(u16, TwoBytes, 16, write_u16_le);
list_value!(i16, TwoBytes, 16, write_i16_le);
list_value!(u32, FourBytes, 32, write_u32_le);
list_value!(i32, FourBytes, 32, write_i32_le);
list_value!(u64, EightBytes, 64, write_u64_le);
list_value!(i64, EightBytes, 64, write_i64_le);
list_value!(f32, FourBytes, 32, write_f32_le);
list_value!(f64, EightBytes, 64, write_f64_le);

pub struct DataListBuilder<'arena, T> {
    arena: &'arena mut ExclusiveArena,
    reference: ListOffset,
    marker: PhantomData<T>,
}

impl<T: PrimitiveListValue> DataListBuilder<'_, T> {
    pub const fn offset(&self) -> ListOffset {
        self.reference
    }

    pub const fn len(&self) -> u32 {
        self.reference.element_count
    }

    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn set(&mut self, index: u32, value: T) -> Result<(), ArenaError> {
        check_index(index, self.len())?;
        let start = u64::from(self.reference.content.word_offset)
            .checked_mul(64)
            .ok_or(ArenaError::AllocationOverflow)?;
        let bits = element_bits(T::ELEMENT_SIZE);
        let offset = u64::from(index)
            .checked_mul(bits)
            .and_then(|relative| start.checked_add(relative))
            .ok_or(ArenaError::AllocationOverflow)?;
        T::write_at(
            self.arena.segment_mut(self.reference.content.segment_id)?,
            offset,
            value,
        )
    }
}

pub struct PointerListBuilder<'arena> {
    arena: &'arena mut ExclusiveArena,
    reference: ListOffset,
}

impl PointerListBuilder<'_> {
    pub const fn offset(&self) -> ListOffset {
        self.reference
    }

    pub const fn len(&self) -> u32 {
        self.reference.element_count
    }

    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn init_struct(
        &mut self,
        index: u32,
        data_words: u16,
        pointer_count: u16,
    ) -> Result<StructBuilder<'_>, ArenaError> {
        let slot = self.slot(index)?;
        let reference = self.arena.allocate_struct(data_words, pointer_count)?;
        self.arena.emit_struct(slot, reference)?;
        Ok(StructBuilder {
            arena: self.arena,
            reference,
        })
    }

    pub fn init_list<T: PrimitiveListValue>(
        &mut self,
        index: u32,
        element_count: u32,
    ) -> Result<DataListBuilder<'_, T>, ArenaError> {
        let slot = self.slot(index)?;
        let reference = self
            .arena
            .allocate_data_list(T::ELEMENT_SIZE, element_count)?;
        self.arena.emit_list(slot, reference)?;
        Ok(DataListBuilder {
            arena: self.arena,
            reference,
            marker: PhantomData,
        })
    }

    pub fn init_pointer_list(
        &mut self,
        index: u32,
        element_count: u32,
    ) -> Result<PointerListBuilder<'_>, ArenaError> {
        let slot = self.slot(index)?;
        let reference = self
            .arena
            .allocate_data_list(ElementSize::Pointer, element_count)?;
        self.arena.emit_list(slot, reference)?;
        Ok(PointerListBuilder {
            arena: self.arena,
            reference,
        })
    }

    pub fn init_struct_list(
        &mut self,
        index: u32,
        element_count: u32,
        data_words: u16,
        pointer_count: u16,
    ) -> Result<StructListBuilder<'_>, ArenaError> {
        let slot = self.slot(index)?;
        let reference =
            self.arena
                .allocate_struct_list(element_count, data_words, pointer_count)?;
        self.arena.emit_list(slot, reference)?;
        Ok(StructListBuilder {
            arena: self.arena,
            reference,
        })
    }

    pub fn set_text(&mut self, index: u32, value: &str) -> Result<(), ArenaError> {
        let slot = self.slot(index)?;
        let count = value
            .len()
            .checked_add(1)
            .and_then(|count| u32::try_from(count).ok())
            .ok_or(ArenaError::AllocationOverflow)?;
        let reference = self.arena.allocate_data_list(ElementSize::Byte, count)?;
        self.arena.emit_list(slot, reference)?;
        let start = byte_offset(reference.content)?;
        let end = start
            .checked_add(value.len())
            .ok_or(ArenaError::AllocationOverflow)?;
        self.arena.segment_mut(reference.content.segment_id)?[start..end]
            .copy_from_slice(value.as_bytes());
        Ok(())
    }

    pub fn set_data(&mut self, index: u32, value: &[u8]) -> Result<(), ArenaError> {
        let slot = self.slot(index)?;
        let count = u32::try_from(value.len()).map_err(|_| ArenaError::AllocationOverflow)?;
        let reference = self.arena.allocate_data_list(ElementSize::Byte, count)?;
        self.arena.emit_list(slot, reference)?;
        let start = byte_offset(reference.content)?;
        let end = start
            .checked_add(value.len())
            .ok_or(ArenaError::AllocationOverflow)?;
        self.arena.segment_mut(reference.content.segment_id)?[start..end].copy_from_slice(value);
        Ok(())
    }

    pub fn set_capability(&mut self, index: u32, capability: u32) -> Result<(), ArenaError> {
        let slot = self.slot(index)?;
        self.arena
            .write_pointer(slot, WirePointer::new_capability(capability))
    }

    pub fn copy_pointer<B: TraversalBudget>(
        &mut self,
        index: u32,
        source: &MessageSegments<'_>,
        source_location: WireLocation,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<(), GraphError> {
        let slot = self.slot(index)?;
        self.arena
            .copy_pointer_at(slot, source, source_location, budget, nesting)
    }

    pub fn clear_pointer<B: TraversalBudget>(
        &mut self,
        index: u32,
        budget: &B,
        nesting: NestingLimit,
    ) -> Result<(), GraphError> {
        let slot = self.slot(index)?;
        self.arena.clear_pointer_at(slot, budget, nesting)
    }

    fn slot(&self, index: u32) -> Result<WordOffset, ArenaError> {
        check_index(index, self.len())?;
        add_words(self.reference.content, u64::from(index))
    }
}

pub struct StructListBuilder<'arena> {
    arena: &'arena mut ExclusiveArena,
    reference: ListOffset,
}

impl StructListBuilder<'_> {
    pub const fn offset(&self) -> ListOffset {
        self.reference
    }

    pub const fn len(&self) -> u32 {
        self.reference.element_count
    }

    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&mut self, index: u32) -> Result<StructBuilder<'_>, ArenaError> {
        check_index(index, self.len())?;
        let (data_words, pointer_count) = self
            .reference
            .inline_struct_size
            .ok_or(ArenaError::AllocationOverflow)?;
        let step = u64::from(data_words) + u64::from(pointer_count);
        let content = add_words(
            self.reference.content,
            u64::from(index)
                .checked_mul(step)
                .ok_or(ArenaError::AllocationOverflow)?,
        )?;
        Ok(StructBuilder {
            arena: self.arena,
            reference: StructOffset {
                arena_id: self.reference.arena_id,
                content,
                data_words,
                pointer_count,
            },
        })
    }
}

fn check_index(index: u32, len: u32) -> Result<(), ArenaError> {
    if index < len {
        Ok(())
    } else {
        Err(ArenaError::IndexOutOfBounds { index, len })
    }
}

#[inline]
fn relative_offset(pointer: WordOffset, target: WordOffset) -> Result<i32, ArenaError> {
    if pointer.segment_id != target.segment_id {
        return Err(ArenaError::AllocationOverflow);
    }
    let value = i64::from(target.word_offset) - i64::from(pointer.word_offset) - 1;
    i32::try_from(value).map_err(|_| ArenaError::AllocationOverflow)
}

fn struct_pointer_slot(reference: StructOffset, index: u16) -> Result<WordOffset, ArenaError> {
    if index >= reference.pointer_count {
        return Err(ArenaError::PointerIndexOutOfBounds {
            index,
            len: reference.pointer_count,
        });
    }
    add_words(
        reference.content,
        u64::from(reference.data_words) + u64::from(index),
    )
}

fn add_words(offset: WordOffset, words: u64) -> Result<WordOffset, ArenaError> {
    let value = u64::from(offset.word_offset)
        .checked_add(words)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(ArenaError::AllocationOverflow)?;
    Ok(WordOffset {
        segment_id: offset.segment_id,
        word_offset: value,
    })
}

const fn word_offset(location: WireLocation) -> WordOffset {
    WordOffset {
        segment_id: location.segment_id,
        word_offset: location.word_offset,
    }
}

fn sub_word(offset: WordOffset) -> Result<WordOffset, ArenaError> {
    Ok(WordOffset {
        segment_id: offset.segment_id,
        word_offset: offset
            .word_offset
            .checked_sub(1)
            .ok_or(ArenaError::AllocationOverflow)?,
    })
}

#[inline]
fn byte_offset(offset: WordOffset) -> Result<usize, ArenaError> {
    usize::try_from(offset.word_offset)
        .ok()
        .and_then(|word| word.checked_mul(8))
        .ok_or(ArenaError::AllocationOverflow)
}

const fn wire_location(offset: WordOffset) -> WireLocation {
    WireLocation {
        segment_id: offset.segment_id,
        word_offset: offset.word_offset,
    }
}

fn wire_byte_offset(location: WireLocation) -> Result<usize, ArenaError> {
    usize::try_from(location.word_offset)
        .ok()
        .and_then(|word| word.checked_mul(8))
        .ok_or(ArenaError::AllocationOverflow)
}

fn add_wire_words(location: WireLocation, words: u64) -> Result<WireLocation, ArenaError> {
    let word_offset = u64::from(location.word_offset)
        .checked_add(words)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(ArenaError::AllocationOverflow)?;
    Ok(WireLocation {
        segment_id: location.segment_id,
        word_offset,
    })
}

fn sub_wire_word(location: WireLocation) -> Result<WireLocation, ArenaError> {
    Ok(WireLocation {
        segment_id: location.segment_id,
        word_offset: location
            .word_offset
            .checked_sub(1)
            .ok_or(ArenaError::AllocationOverflow)?,
    })
}

fn read_message_pointer(
    message: &MessageSegments<'_>,
    location: WireLocation,
) -> Result<WirePointer, GraphError> {
    let segment = message
        .segment(location.segment_id)
        .ok_or(ValidationError::UnknownSegment {
            segment_id: location.segment_id,
        })?;
    WirePointer::read_from(segment, wire_byte_offset(location)?)
        .map_err(|_| ValidationError::PointerOutOfBounds { location }.into())
}

fn source_word_is_zero(
    message: &MessageSegments<'_>,
    location: WireLocation,
) -> Result<bool, GraphError> {
    let segment = message
        .segment(location.segment_id)
        .ok_or(ValidationError::UnknownSegment {
            segment_id: location.segment_id,
        })?;
    let start = wire_byte_offset(location)?;
    let end = start.checked_add(8).ok_or(ArenaError::AllocationOverflow)?;
    let bytes = segment
        .get(start..end)
        .ok_or(ValidationError::ObjectOutOfBounds {
            location,
            words: 1,
            segment_words: u64::try_from(segment.len() / 8)
                .map_err(|_| ArenaError::AllocationOverflow)?,
        })?;
    Ok(bytes.iter().all(|byte| *byte == 0))
}

const fn root_offset() -> WordOffset {
    WordOffset {
        segment_id: 0,
        word_offset: 0,
    }
}

#[inline]
fn validate_segment_words(words: u32) -> Result<(), ArenaError> {
    if words == 0 || words > MAX_SINGLE_SEGMENT_WORDS {
        Err(ArenaError::InvalidWordLimit { requested: words })
    } else {
        Ok(())
    }
}

#[inline]
fn word_bytes(words: u32) -> Result<usize, ArenaError> {
    usize::try_from(words)
        .ok()
        .and_then(|words| words.checked_mul(8))
        .ok_or(ArenaError::AllocationOverflow)
}

const fn element_bits(size: ElementSize) -> u64 {
    match size {
        ElementSize::Void => 0,
        ElementSize::Bit => 1,
        ElementSize::Byte => 8,
        ElementSize::TwoBytes => 16,
        ElementSize::FourBytes => 32,
        ElementSize::EightBytes | ElementSize::Pointer => 64,
        ElementSize::InlineComposite => 0,
    }
}

fn list_words(size: ElementSize, count: u32) -> Result<u32, ArenaError> {
    let bits = u64::from(count)
        .checked_mul(element_bits(size))
        .ok_or(ArenaError::AllocationOverflow)?;
    let words = bits.checked_add(63).ok_or(ArenaError::AllocationOverflow)? / 64;
    u32::try_from(words).map_err(|_| ArenaError::AllocationOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        LocalTraversalBudget, MessageSegments, NestingLimit, ResolvedPointer, TraversalBudget,
        WireLocation,
    };

    const ROOT: WireLocation = WireLocation {
        segment_id: 0,
        word_offset: 0,
    };

    #[test]
    fn every_base_shape_builds_and_reads_through_the_checked_runtime() {
        let mut arena = ExclusiveArena::new(1, 128).expect("arena allocates");
        {
            let mut root = arena
                .init_root_struct(2, 7)
                .expect("root struct initializes");
            root.set_u64(0, 0x0123_4567_89ab_cdef, 0)
                .expect("scalar fits");
            root.set_bool(64, true, false).expect("bool fits");
            {
                let mut child = root.init_struct(0, 1, 0).expect("child initializes");
                child.set_u32(0, 42, 0).expect("child value fits");
            }
            root.set_text(1, "native").expect("text initializes");
            root.set_data(2, &[0, 1, 0xff]).expect("data initializes");
            {
                let mut values = root.init_list::<u16>(3, 3).expect("list initializes");
                values.set(0, 7).expect("element fits");
                values.set(1, 8).expect("element fits");
                values.set(2, 9).expect("element fits");
            }
            {
                let mut pointers = root
                    .init_pointer_list(4, 2)
                    .expect("pointer list initializes");
                pointers.set_text(0, "zero").expect("text element fits");
                let mut nested = pointers
                    .init_list::<u16>(1, 2)
                    .expect("nested list initializes");
                nested.set(0, 100).expect("nested value fits");
                nested.set(1, 200).expect("nested value fits");
            }
            {
                let mut structs = root
                    .init_struct_list(5, 2, 1, 1)
                    .expect("struct list initializes");
                {
                    let mut first = structs.get(0).expect("first struct exists");
                    first.set_u32(0, 11, 0).expect("first value fits");
                    first.set_text(0, "first").expect("first text fits");
                }
                {
                    let mut second = structs.get(1).expect("second struct exists");
                    second.set_u32(0, 22, 0).expect("second value fits");
                    second.set_text(0, "second").expect("second text fits");
                }
            }
            root.set_capability(6, 77).expect("capability fits");
        }

        let segment = arena.as_segment();
        let segments = MessageSegments::new(&[segment]).expect("built segment validates");
        let budget = LocalTraversalBudget::new(128);
        let root = segments
            .read_struct(ROOT, &budget, NestingLimit::new(8))
            .expect("root reads");
        assert_eq!(
            root.data_section()
                .expect("root data exists")
                .read_u64(0, 0),
            Ok(0x0123_4567_89ab_cdef)
        );
        assert_eq!(
            root.data_section()
                .expect("root data exists")
                .read_bool(64, false),
            Ok(true)
        );
        assert_eq!(
            root.read_struct(0, None)
                .expect("child reads")
                .data_section()
                .expect("child data exists")
                .read_u32(0, 0),
            Ok(42)
        );
        assert_eq!(
            root.read_text(1, None).expect("text reads").to_str(),
            Ok("native")
        );
        assert_eq!(
            root.read_data(2, None).expect("data reads").as_bytes(),
            &[0, 1, 0xff]
        );
        let values = root
            .read_list(3, None)
            .expect("values read")
            .as_primitive::<u16>()
            .expect("values have u16 layout");
        assert_eq!(
            values.iter().collect::<Result<Vec<_>, _>>(),
            Ok(vec![7, 8, 9])
        );

        let pointers = root
            .read_list(4, None)
            .expect("pointers read")
            .as_pointers()
            .expect("pointer layout matches");
        assert_eq!(
            pointers.read_text(0).expect("text element reads").to_str(),
            Ok("zero")
        );
        let nested = pointers
            .get_list(1)
            .expect("nested list reads")
            .as_primitive::<u16>()
            .expect("nested type matches");
        assert_eq!(nested.get(0), Ok(100));
        assert_eq!(nested.get(1), Ok(200));

        let structs = root
            .read_list(5, None)
            .expect("struct list reads")
            .as_structs()
            .expect("struct layout matches");
        let first = structs.get(0).expect("first struct reads");
        assert_eq!(
            first
                .data_section()
                .expect("first data exists")
                .read_u32(0, 0),
            Ok(11)
        );
        assert_eq!(
            first.read_text(0, None).expect("first text reads").to_str(),
            Ok("first")
        );
        assert_eq!(
            root.resolve_pointer(6, None)
                .expect("capability resolves")
                .value
                .pointer,
            ResolvedPointer::Capability(crate::CapabilityRef { index: 77 })
        );
        assert!(budget.remaining_words() < 128);
    }

    #[test]
    fn every_primitive_list_encoding_is_zeroed_and_writable() {
        let mut arena = ExclusiveArena::new(1, 64).expect("arena allocates");
        {
            let mut root = arena.init_root_struct(0, 8).expect("root initializes");
            root.init_list::<()>(0, 3).expect("void list initializes");
            let mut bits = root.init_list::<bool>(1, 3).expect("bit list initializes");
            bits.set(1, true).expect("bit writes");
            let mut bytes = root.init_list::<u8>(2, 2).expect("byte list initializes");
            bytes.set(1, 0xa5).expect("byte writes");
            let mut twos = root.init_list::<i16>(3, 2).expect("i16 list initializes");
            twos.set(0, -7).expect("i16 writes");
            let mut fours = root.init_list::<u32>(4, 2).expect("u32 list initializes");
            fours.set(1, 0x89ab_cdef).expect("u32 writes");
            let mut eights = root.init_list::<i64>(5, 2).expect("i64 list initializes");
            eights.set(0, -9).expect("i64 writes");
            let mut floats = root.init_list::<f32>(6, 1).expect("f32 list initializes");
            floats
                .set(0, f32::from_bits(0x7fc0_1234))
                .expect("f32 writes");
            let mut doubles = root.init_list::<f64>(7, 1).expect("f64 list initializes");
            doubles.set(0, -1.5).expect("f64 writes");
        }

        let segments = MessageSegments::new(&[arena.as_segment()]).expect("message validates");
        let budget = LocalTraversalBudget::new(64);
        let root = segments
            .read_struct(ROOT, &budget, NestingLimit::new(2))
            .expect("root reads");
        assert_eq!(
            root.read_list(0, None)
                .expect("void reads")
                .as_primitive::<()>()
                .expect("void type matches")
                .len(),
            3
        );
        assert_eq!(
            root.read_list(1, None)
                .expect("bits read")
                .as_primitive::<bool>()
                .expect("bit type matches")
                .iter()
                .collect::<Result<Vec<_>, _>>(),
            Ok(vec![false, true, false])
        );
        assert_eq!(
            root.read_list(2, None)
                .expect("bytes read")
                .as_primitive::<u8>()
                .expect("byte type matches")
                .iter()
                .collect::<Result<Vec<_>, _>>(),
            Ok(vec![0, 0xa5])
        );
        assert_eq!(
            root.read_list(3, None)
                .expect("i16s read")
                .as_primitive::<i16>()
                .expect("i16 type matches")
                .get(0),
            Ok(-7)
        );
        assert_eq!(
            root.read_list(4, None)
                .expect("u32s read")
                .as_primitive::<u32>()
                .expect("u32 type matches")
                .get(1),
            Ok(0x89ab_cdef)
        );
        assert_eq!(
            root.read_list(5, None)
                .expect("i64s read")
                .as_primitive::<i64>()
                .expect("i64 type matches")
                .get(0),
            Ok(-9)
        );
        assert_eq!(
            root.read_list(6, None)
                .expect("f32s read")
                .as_primitive::<f32>()
                .expect("f32 type matches")
                .get(0)
                .expect("f32 value reads")
                .to_bits(),
            0x7fc0_1234
        );
        assert_eq!(
            root.read_list(7, None)
                .expect("f64s read")
                .as_primitive::<f64>()
                .expect("f64 type matches")
                .get(0),
            Ok(-1.5)
        );
    }

    #[test]
    fn growth_is_zeroed_and_limits_fail_before_exposure() {
        let mut arena = ExclusiveArena::new(1, 4).expect("arena allocates");
        {
            let mut root = arena.init_root_struct(1, 1).expect("root grows arena");
            root.set_u8(0, 0xa5, 0).expect("first byte writes");
            assert_eq!(
                root.init_struct(0, 2, 0).map(|_| ()),
                Err(ArenaError::AllocationLimit {
                    requested: 5,
                    limit: 4,
                })
            );
        }
        assert_eq!(arena.word_len(), 3);
        assert_eq!(&arena.as_segment()[8..16], &[0xa5, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&arena.as_segment()[16..24], &[0; 8]);
        assert_eq!(
            arena.init_root_struct(0, 0).map(|_| ()),
            Err(ArenaError::AlreadyInitialized)
        );

        let mut retry = ExclusiveArena::new(1, 2).expect("small arena allocates");
        assert_eq!(
            retry.init_root_struct(2, 0).map(|_| ()),
            Err(ArenaError::AllocationLimit {
                requested: 3,
                limit: 2,
            })
        );
        assert!(retry.init_root_struct(1, 0).is_ok());

        let mut overflow = ExclusiveArena::new(1, 8).expect("arena allocates");
        assert_eq!(
            overflow
                .init_root_struct_list(u32::MAX, u16::MAX, u16::MAX)
                .map(|_| ()),
            Err(ArenaError::AllocationOverflow)
        );
    }

    #[test]
    fn reset_zeroes_storage_drops_extra_segments_and_invalidates_offsets() {
        let mut arena = ExclusiveArena::new_segmented(1, 2, 3, 5).expect("arena initializes");
        let old = {
            let mut root = arena.init_root_struct(2, 0).expect("root initializes");
            root.set_u64(0, u64::MAX, 0).expect("field writes");
            root.offset()
        };
        assert_eq!(arena.segment_count(), 3);
        assert!(
            arena
                .segments()
                .any(|segment| segment.iter().any(|byte| *byte != 0))
        );

        arena.reset().expect("arena resets");
        assert_eq!(arena.segment_count(), 1);
        assert_eq!(arena.word_len(), 1);
        assert_eq!(arena.as_segment(), &[0; 8]);

        let new = arena
            .init_root_struct(2, 0)
            .expect("a new root initializes after reset")
            .offset();
        assert_ne!(old.arena_id, new.arena_id);

        let mut retained = ExclusiveArena::new(8, 8).expect("retained arena initializes");
        retained
            .init_root_struct(7, 0)
            .expect("first root uses the complete segment")
            .set_u64(6, u64::MAX, 0)
            .expect("last word is writable");
        retained.reset().expect("retained arena resets");
        retained
            .init_root_struct(1, 0)
            .expect("smaller root reuses initialized storage")
            .set_u64(0, 42, 0)
            .expect("reused word is writable");
        assert_eq!(retained.word_len(), 2);
        assert_eq!(retained.as_segment().len(), 16);
        assert_eq!(
            retained.into_segment().expect("one segment remains").len(),
            16
        );
    }

    #[test]
    fn setters_and_elements_reject_out_of_bounds_indices() {
        let mut arena = ExclusiveArena::new(1, 8).expect("arena allocates");
        let mut root = arena.init_root_struct(1, 1).expect("root initializes");
        assert!(matches!(
            root.set_u64(1, 1, 0),
            Err(ArenaError::IndexOutOfBounds { .. })
        ));
        assert!(matches!(
            root.set_text(1, "missing"),
            Err(ArenaError::PointerIndexOutOfBounds { .. })
        ));
        let mut list = root.init_list::<u8>(0, 1).expect("list initializes");
        assert_eq!(
            list.set(1, 7),
            Err(ArenaError::IndexOutOfBounds { index: 1, len: 1 })
        );
    }

    #[test]
    fn tiny_segments_force_single_and_double_far_struct_pads() {
        for (next_words, double_far) in [(2, false), (1, true)] {
            let mut arena = ExclusiveArena::new_segmented(2, next_words, 4, 16)
                .expect("segmented arena initializes");
            {
                let mut root = arena.init_root_struct(0, 1).expect("root initializes");
                let mut child = root.init_struct(0, 1, 0).expect("child initializes");
                child.set_u32(0, 1234, 0).expect("child value writes");
            }

            let source = WirePointer::read_from(arena.segment(0).expect("segment zero"), 8)
                .expect("far pointer exists");
            let far = source.far_fields().expect("child uses a far pointer");
            assert_eq!(far.double_far, double_far);
            if double_far {
                assert_eq!(arena.segment_count(), 3);
                let first = WirePointer::read_from(
                    arena.segment(far.segment_id).expect("pad segment exists"),
                    usize::try_from(far.landing_pad_word).expect("offset fits") * 8,
                )
                .expect("double-far first word exists");
                assert!(first.far_fields().is_some());
            } else {
                assert_eq!(arena.segment_count(), 2);
                let pad = WirePointer::read_from(
                    arena.segment(far.segment_id).expect("pad segment exists"),
                    usize::try_from(far.landing_pad_word).expect("offset fits") * 8,
                )
                .expect("single landing pad exists");
                assert!(pad.struct_fields().is_some());
            }

            let owned = arena.into_segments();
            let borrowed = owned.iter().map(AsRef::as_ref).collect::<Vec<_>>();
            let segments = MessageSegments::new(&borrowed).expect("segments validate");
            let budget = LocalTraversalBudget::new(8);
            let root = segments
                .read_struct(ROOT, &budget, NestingLimit::new(2))
                .expect("root reads");
            assert_eq!(
                root.read_struct(0, None)
                    .expect("far child reads")
                    .data_section()
                    .expect("child data exists")
                    .read_u32(0, 0),
                Ok(1234)
            );
        }
    }

    #[test]
    fn tiny_segments_force_single_and_double_far_list_pads() {
        for (next_words, double_far) in [(2, false), (1, true)] {
            let mut arena = ExclusiveArena::new_segmented(2, next_words, 4, 16)
                .expect("segmented arena initializes");
            {
                let mut root = arena.init_root_struct(0, 1).expect("root initializes");
                let mut list = root.init_list::<u16>(0, 2).expect("list initializes");
                list.set(0, 5).expect("first value writes");
                list.set(1, 6).expect("second value writes");
            }
            let source = WirePointer::read_from(arena.segment(0).expect("segment zero"), 8)
                .expect("far pointer exists");
            assert_eq!(
                source
                    .far_fields()
                    .expect("list uses a far pointer")
                    .double_far,
                double_far
            );
            let owned = arena.into_segments();
            let borrowed = owned.iter().map(AsRef::as_ref).collect::<Vec<_>>();
            let segments = MessageSegments::new(&borrowed).expect("segments validate");
            let budget = LocalTraversalBudget::new(8);
            let root = segments
                .read_struct(ROOT, &budget, NestingLimit::new(2))
                .expect("root reads");
            let list = root
                .read_list(0, None)
                .expect("far list reads")
                .as_primitive::<u16>()
                .expect("list type matches");
            assert_eq!(list.iter().collect::<Result<Vec<_>, _>>(), Ok(vec![5, 6]));
        }
    }

    #[test]
    fn segmented_layout_is_deterministic_and_pad_limits_are_checked() {
        fn build() -> Vec<Box<[u8]>> {
            let mut arena =
                ExclusiveArena::new_segmented(2, 1, 8, 32).expect("segmented arena initializes");
            {
                let mut root = arena.init_root_struct(0, 2).expect("root initializes");
                let mut child = root.init_struct(0, 1, 0).expect("child initializes");
                child.set_u32(0, 9, 0).expect("child value writes");
                root.set_text(1, "deterministic").expect("text writes");
            }
            arena.into_segments()
        }
        assert_eq!(build(), build());

        assert!(matches!(
            ExclusiveArena::new_segmented(1, MAX_SINGLE_SEGMENT_WORDS + 1, 2, 2),
            Err(ArenaError::InvalidWordLimit { .. })
        ));

        let mut segment_limited =
            ExclusiveArena::new_segmented(2, 1, 2, 16).expect("arena initializes");
        let mut root = segment_limited
            .init_root_struct(0, 1)
            .expect("root initializes");
        assert_eq!(
            root.init_struct(0, 1, 0).map(|_| ()),
            Err(ArenaError::SegmentLimit {
                requested: 3,
                limit: 2,
            })
        );

        let mut word_limited =
            ExclusiveArena::new_segmented(2, 1, 4, 4).expect("arena initializes");
        let mut root = word_limited
            .init_root_struct(0, 1)
            .expect("root initializes");
        assert_eq!(
            root.init_struct(0, 1, 0).map(|_| ()),
            Err(ArenaError::AllocationLimit {
                requested: 5,
                limit: 4,
            })
        );
    }

    #[test]
    fn schema_independent_copy_rebuilds_and_clear_zeroes_a_multisegment_graph() {
        let mut source_arena =
            ExclusiveArena::new_segmented(2, 2, 16, 64).expect("source arena initializes");
        {
            let mut root = source_arena
                .init_root_struct(1, 3)
                .expect("source root initializes");
            root.set_u64(0, 0xfeed_face_cafe_beef, 0)
                .expect("source data writes");
            root.set_text(0, "copied").expect("source text writes");
            {
                let mut child = root.init_struct(1, 1, 1).expect("source child writes");
                child.set_u32(0, 77, 0).expect("child data writes");
                let mut values = child.init_list::<u16>(0, 2).expect("child list writes");
                values.set(0, 8).expect("value writes");
                values.set(1, 9).expect("value writes");
            }
            {
                let mut structs = root
                    .init_struct_list(2, 2, 1, 0)
                    .expect("struct list writes");
                structs
                    .get(0)
                    .expect("first exists")
                    .set_u32(0, 1, 0)
                    .expect("first writes");
                structs
                    .get(1)
                    .expect("second exists")
                    .set_u32(0, 2, 0)
                    .expect("second writes");
            }
        }
        let source_owned = source_arena.into_segments();
        let source_borrowed = source_owned.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        let source = MessageSegments::new(&source_borrowed).expect("source validates");

        let mut copied =
            ExclusiveArena::new_segmented(1, 3, 32, 128).expect("copy arena initializes");
        copied
            .copy_root(
                &source,
                ROOT,
                &LocalTraversalBudget::new(64),
                NestingLimit::new(8),
            )
            .expect("complete graph copies");
        let copied_refs = copied.segments().collect::<Vec<_>>();
        let copied_message = MessageSegments::new(&copied_refs).expect("copy validates");
        let budget = LocalTraversalBudget::new(64);
        let root = copied_message
            .read_struct(ROOT, &budget, NestingLimit::new(8))
            .expect("copy root reads");
        assert_eq!(
            root.data_section()
                .expect("copy data exists")
                .read_u64(0, 0),
            Ok(0xfeed_face_cafe_beef)
        );
        assert_eq!(
            root.read_text(0, None).expect("copy text reads").to_str(),
            Ok("copied")
        );
        assert_eq!(
            root.read_struct(1, None)
                .expect("copy child reads")
                .data_section()
                .expect("child data exists")
                .read_u32(0, 0),
            Ok(77)
        );
        assert_eq!(
            root.read_list(2, None)
                .expect("copy structs read")
                .as_structs()
                .expect("struct layout matches")
                .get(1)
                .expect("second copy exists")
                .data_section()
                .expect("second data exists")
                .read_u32(0, 0),
            Ok(2)
        );
        copied
            .clear_root(&LocalTraversalBudget::new(64), NestingLimit::new(8))
            .expect("complete graph clears");
        assert!(
            copied
                .segments()
                .all(|segment| segment.iter().all(|byte| *byte == 0))
        );
    }

    #[test]
    fn failed_cycle_copy_rolls_back_and_failed_cycle_clear_is_unchanged() {
        let cycle = WirePointer::new_struct(-1, 0, 1).expect("self pointer fits");
        let mut source_bytes = vec![0u8; 8];
        cycle
            .write_to(&mut source_bytes, 0)
            .expect("self pointer writes");
        let source = MessageSegments::new(&[&source_bytes]).expect("cycle validates shallowly");
        let mut destination =
            ExclusiveArena::new_segmented(1, 2, 16, 64).expect("destination initializes");
        let before = destination
            .segments()
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
        assert!(matches!(
            destination.copy_root(
                &source,
                ROOT,
                &LocalTraversalBudget::new(3),
                NestingLimit::new(10),
            ),
            Err(GraphError::Traversal(TraversalError::Budget(_)))
        ));
        assert_eq!(
            destination
                .segments()
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>(),
            before
        );

        let mut built_cycle = ExclusiveArena::new(1, 4).expect("cycle arena initializes");
        {
            let root = built_cycle
                .init_root_struct(0, 1)
                .expect("cycle root initializes");
            let slot = root.pointer_slot(0).expect("self slot exists");
            root.arena
                .write_pointer(slot, WirePointer::new_struct(-1, 0, 1).expect("cycle fits"))
                .expect("cycle writes");
        }
        let before = built_cycle
            .segments()
            .map(<[u8]>::to_vec)
            .collect::<Vec<_>>();
        assert!(
            built_cycle
                .clear_root(&LocalTraversalBudget::new(2), NestingLimit::new(10))
                .is_err()
        );
        assert_eq!(
            built_cycle
                .segments()
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>(),
            before
        );
    }

    #[test]
    fn overlapping_copy_targets_are_charged_per_reference() {
        let mut bytes = vec![0u8; 32];
        WirePointer::new_struct(0, 0, 2)
            .expect("root fits")
            .write_to(&mut bytes, 0)
            .expect("root writes");
        WirePointer::new_struct(1, 1, 0)
            .expect("first alias fits")
            .write_to(&mut bytes, 8)
            .expect("first alias writes");
        WirePointer::new_struct(0, 1, 0)
            .expect("second alias fits")
            .write_to(&mut bytes, 16)
            .expect("second alias writes");
        bytes[24..28].copy_from_slice(&101u32.to_le_bytes());
        let source = MessageSegments::new(&[&bytes]).expect("source validates");

        let mut insufficient = ExclusiveArena::new(1, 16).expect("arena initializes");
        assert!(matches!(
            insufficient.copy_root(
                &source,
                ROOT,
                &LocalTraversalBudget::new(3),
                NestingLimit::new(3),
            ),
            Err(GraphError::Traversal(TraversalError::Budget(_)))
        ));
        assert_eq!(insufficient.as_segment(), &[0; 8]);

        let mut exact = ExclusiveArena::new(1, 16).expect("arena initializes");
        let budget = LocalTraversalBudget::new(4);
        exact
            .copy_root(&source, ROOT, &budget, NestingLimit::new(3))
            .expect("exact repeated charge copies both aliases");
        assert_eq!(budget.remaining_words(), 0);
        let refs = exact.segments().collect::<Vec<_>>();
        let message = MessageSegments::new(&refs).expect("copy validates");
        let read_budget = LocalTraversalBudget::new(4);
        let root = message
            .read_struct(ROOT, &read_budget, NestingLimit::new(3))
            .expect("root reads");
        let first = root.read_struct(0, None).expect("first copy reads");
        let second = root.read_struct(1, None).expect("second copy reads");
        assert_ne!(first.reference(), second.reference());
        assert_eq!(
            first
                .data_section()
                .expect("first data exists")
                .read_u32(0, 0),
            Ok(101)
        );
        assert_eq!(
            second
                .data_section()
                .expect("second data exists")
                .read_u32(0, 0),
            Ok(101)
        );
    }

    #[test]
    fn typed_orphans_move_structs_and_lists_without_copying() {
        let mut arena = ExclusiveArena::new_segmented(2, 2, 16, 64).expect("arena initializes");
        let root_offset;
        let child_offset;
        let list_offset;
        {
            let mut root = arena.init_root_struct(0, 4).expect("root initializes");
            root_offset = root.offset();
            {
                let mut child = root.init_struct(0, 1, 0).expect("child initializes");
                child_offset = child.offset();
                child.set_u32(0, 55, 0).expect("child value writes");
            }
            {
                let mut list = root.init_list::<u16>(2, 2).expect("list initializes");
                list_offset = list.offset();
                list.set(0, 7).expect("list value writes");
                list.set(1, 8).expect("list value writes");
            }
            root.disown_struct(0)
                .expect("struct disowns")
                .adopt_into_struct(root_offset, 1)
                .expect("struct adopts");
            root.disown_list(2)
                .expect("list disowns")
                .adopt_into_struct(root_offset, 3)
                .expect("list adopts");
        }

        let refs = arena.segments().collect::<Vec<_>>();
        let message = MessageSegments::new(&refs).expect("moved graph validates");
        let budget = LocalTraversalBudget::new(32);
        let root = message
            .read_struct(ROOT, &budget, NestingLimit::new(4))
            .expect("root reads");
        assert!(
            root.read_struct(0, None)
                .expect("old field reads")
                .reference()
                .is_none()
        );
        let child = root.read_struct(1, None).expect("moved child reads");
        assert_eq!(
            child.reference().expect("child is non-null").content,
            wire_location(child_offset.content)
        );
        assert_eq!(
            child
                .data_section()
                .expect("child data exists")
                .read_u32(0, 0),
            Ok(55)
        );
        assert!(root.read_list(2, None).expect("old list reads").is_empty());
        let list = root.read_list(3, None).expect("moved list reads");
        assert_eq!(
            list.reference().expect("list is non-null").content,
            wire_location(list_offset.content)
        );
        assert_eq!(
            list.as_primitive::<u16>()
                .expect("list type matches")
                .iter()
                .collect::<Result<Vec<_>, _>>(),
            Ok(vec![7, 8])
        );
    }

    #[test]
    fn orphan_type_and_arena_checks_preserve_or_clear_storage_safely() {
        let mut foreign = ExclusiveArena::new(1, 8).expect("foreign arena initializes");
        let foreign_root = foreign
            .init_root_struct(0, 1)
            .expect("foreign root initializes")
            .offset();

        let mut arena = ExclusiveArena::new(1, 32).expect("arena initializes");
        let root_offset;
        {
            let mut root = arena.init_root_struct(0, 2).expect("root initializes");
            root_offset = root.offset();
            root.init_list::<u16>(0, 1).expect("list initializes");
            assert!(matches!(
                root.disown_struct(0),
                Err(GraphError::ExpectedStruct)
            ));
            {
                let mut child = root.init_struct(1, 1, 1).expect("child initializes");
                child.set_u64(0, u64::MAX, 0).expect("child data writes");
                child.set_text(0, "secret").expect("child text writes");
            }
            let orphan = root.disown_struct(1).expect("child disowns");
            assert_eq!(
                orphan.adopt_into_struct(foreign_root, 0),
                Err(GraphError::WrongArena)
            );
        }

        let root_slot = struct_pointer_slot(root_offset, 1).expect("root slot exists");
        assert!(arena.read_pointer(root_slot).expect("slot reads").is_null());
        assert!(
            arena
                .segments()
                .flat_map(<[u8]>::iter)
                .skip(16)
                .all(|byte| *byte == 0)
        );
    }
}
