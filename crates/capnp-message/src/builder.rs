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
//! Deep copying, orphans, canonicalization, generated setters, and parallel
//! construction are later milestones.

use core::fmt;
use core::marker::PhantomData;

use capnp_wire::{
    ElementSize, WireError, WirePointer, write_f32_le, write_f64_le, write_i8, write_i16_le,
    write_i32_le, write_i64_le, write_u8, write_u16_le, write_u32_le, write_u64_le,
};

/// One segment cannot exceed the span addressable by every signed positional pointer.
pub const MAX_SINGLE_SEGMENT_WORDS: u32 = 1 << 29;

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

impl std::error::Error for ArenaError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
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

#[derive(Debug)]
struct SegmentStorage {
    bytes: Vec<u8>,
    word_limit: u32,
}

/// A growable, exclusively borrowed arena with deterministic segment policy.
///
/// `new()` retains M11's single-segment behavior. `new_segmented()` fixes the
/// first and preferred later segment sizes so tests and callers can force
/// landing-pad placement without depending on allocator capacity.
#[derive(Debug)]
pub struct ExclusiveArena {
    segments: Vec<SegmentStorage>,
    next_segment_words: u32,
    max_segments: u32,
    max_total_words: u64,
    root_initialized: bool,
}

impl ExclusiveArena {
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

    fn new_with_policy(
        initial_capacity_words: u32,
        first_word_limit: u32,
        next_segment_words: u32,
        max_segments: u32,
        max_total_words: u64,
    ) -> Result<Self, ArenaError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(word_bytes(initial_capacity_words)?)
            .map_err(|_| ArenaError::AllocationFailed)?;
        bytes.resize(8, 0);
        Ok(Self {
            segments: vec![SegmentStorage {
                bytes,
                word_limit: first_word_limit,
            }],
            next_segment_words,
            max_segments,
            max_total_words,
            root_initialized: false,
        })
    }

    pub fn word_len(&self) -> u64 {
        self.segments
            .iter()
            .map(|segment| (segment.bytes.len() / 8) as u64)
            .sum()
    }

    pub const fn max_words(&self) -> u64 {
        self.max_total_words
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn segment(&self, id: u32) -> Option<&[u8]> {
        usize::try_from(id).ok().and_then(|index| {
            self.segments
                .get(index)
                .map(|segment| segment.bytes.as_slice())
        })
    }

    pub fn segments(&self) -> impl ExactSizeIterator<Item = &[u8]> {
        self.segments.iter().map(|segment| segment.bytes.as_slice())
    }

    pub fn as_segment(&self) -> &[u8] {
        self.segment(0).expect("an arena always has segment zero")
    }

    pub fn into_segment(self) -> Result<Box<[u8]>, ArenaError> {
        if self.segments.len() != 1 {
            return Err(ArenaError::MultipleSegments);
        }
        Ok(self
            .segments
            .into_iter()
            .next()
            .expect("an arena always has segment zero")
            .bytes
            .into_boxed_slice())
    }

    pub fn into_segments(self) -> Vec<Box<[u8]>> {
        self.segments
            .into_iter()
            .map(|segment| segment.bytes.into_boxed_slice())
            .collect()
    }

    /// Initializes the root once and returns the arena's exclusive struct view.
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

    fn require_uninitialized_root(&self) -> Result<(), ArenaError> {
        if self.root_initialized {
            return Err(ArenaError::AlreadyInitialized);
        }
        Ok(())
    }

    fn allocate_words(&mut self, words: u64) -> Result<WordOffset, ArenaError> {
        let words_u32 = u32::try_from(words).map_err(|_| ArenaError::AllocationOverflow)?;
        if words_u32 > MAX_SINGLE_SEGMENT_WORDS {
            return Err(ArenaError::AllocationOverflow);
        }
        let last =
            u32::try_from(self.segments.len() - 1).map_err(|_| ArenaError::AllocationOverflow)?;
        if let Some(location) = self.try_allocate_in_segment(last, words_u32)? {
            return Ok(location);
        }
        self.allocate_new_segment(words_u32)
    }

    fn try_allocate_in_segment(
        &mut self,
        segment_id: u32,
        words: u32,
    ) -> Result<Option<WordOffset>, ArenaError> {
        let index = usize::try_from(segment_id).map_err(|_| ArenaError::AllocationOverflow)?;
        let segment = self
            .segments
            .get(index)
            .ok_or(ArenaError::AllocationOverflow)?;
        let current =
            u32::try_from(segment.bytes.len() / 8).map_err(|_| ArenaError::AllocationOverflow)?;
        let end = current
            .checked_add(words)
            .ok_or(ArenaError::AllocationOverflow)?;
        if end > segment.word_limit {
            return Ok(None);
        }
        self.ensure_total_limit(u64::from(words))?;
        let new_len = word_bytes(end)?;
        let segment = &mut self.segments[index];
        segment
            .bytes
            .try_reserve_exact(new_len.saturating_sub(segment.bytes.len()))
            .map_err(|_| ArenaError::AllocationFailed)?;
        segment.bytes.resize(new_len, 0);
        Ok(Some(WordOffset {
            segment_id,
            word_offset: current,
        }))
    }

    fn allocate_new_segment(&mut self, words: u32) -> Result<WordOffset, ArenaError> {
        self.ensure_total_limit(u64::from(words))?;
        let requested_segments = u32::try_from(self.segments.len())
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
        let segment_id =
            u32::try_from(self.segments.len()).map_err(|_| ArenaError::AllocationOverflow)?;
        self.segments.push(SegmentStorage { bytes, word_limit });
        Ok(WordOffset {
            segment_id,
            word_offset: 0,
        })
    }

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

    fn allocate_struct(
        &mut self,
        data_words: u16,
        pointer_count: u16,
    ) -> Result<StructOffset, ArenaError> {
        let words = u64::from(data_words) + u64::from(pointer_count);
        let content = self.allocate_words(words)?;
        Ok(StructOffset {
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
            pointer_target,
            content,
            element_size: ElementSize::InlineComposite,
            element_count,
            content_words: content_words_u32,
            inline_struct_size: Some((data_words, pointer_count)),
        })
    }

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

    fn write_pointer(
        &mut self,
        location: WordOffset,
        pointer: WirePointer,
    ) -> Result<(), ArenaError> {
        let segment = self.segment_mut(location.segment_id)?;
        pointer.write_to(segment, byte_offset(location)?)?;
        Ok(())
    }

    fn segment_mut(&mut self, segment_id: u32) -> Result<&mut [u8], ArenaError> {
        usize::try_from(segment_id)
            .ok()
            .and_then(|index| self.segments.get_mut(index))
            .map(|segment| segment.bytes.as_mut_slice())
            .ok_or(ArenaError::AllocationOverflow)
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

    pub fn clear_pointer(&mut self, pointer_index: u16) -> Result<(), ArenaError> {
        let slot = self.pointer_slot(pointer_index)?;
        self.arena.write_pointer(slot, WirePointer::NULL)
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
        if index >= self.reference.pointer_count {
            return Err(ArenaError::PointerIndexOutOfBounds {
                index,
                len: self.reference.pointer_count,
            });
        }
        add_words(
            self.reference.content,
            u64::from(self.reference.data_words) + u64::from(index),
        )
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

fn relative_offset(pointer: WordOffset, target: WordOffset) -> Result<i32, ArenaError> {
    if pointer.segment_id != target.segment_id {
        return Err(ArenaError::AllocationOverflow);
    }
    let value = i64::from(target.word_offset) - i64::from(pointer.word_offset) - 1;
    i32::try_from(value).map_err(|_| ArenaError::AllocationOverflow)
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

fn byte_offset(offset: WordOffset) -> Result<usize, ArenaError> {
    usize::try_from(offset.word_offset)
        .ok()
        .and_then(|word| word.checked_mul(8))
        .ok_or(ArenaError::AllocationOverflow)
}

const fn root_offset() -> WordOffset {
    WordOffset {
        segment_id: 0,
        word_offset: 0,
    }
}

fn validate_segment_words(words: u32) -> Result<(), ArenaError> {
    if words == 0 || words > MAX_SINGLE_SEGMENT_WORDS {
        Err(ArenaError::InvalidWordLimit { requested: words })
    } else {
        Ok(())
    }
}

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
}
