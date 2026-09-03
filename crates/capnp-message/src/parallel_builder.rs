//! Opt-in partitioned construction for disjoint list work.
//!
//! This module implements the phases in ADR-0003 without making the ordinary
//! arena concurrent. Primitive partitions borrow disjoint byte ranges. Pointer
//! workers build private single-segment fragments, seal them, and return them
//! to a coordinator that links roots with single-far pointers in slot order.
//! Unwritten primitive bytes remain zero and absent worker lanes leave null
//! pointers, so cancellation and panic preserve valid defaults.
//!
//! The wire rules come from the pinned C++ `layout.c++` implementation. Lane
//! scheduling is native policy. Capabilities, arbitrary shared mutation,
//! multi-segment worker fragments, and canonical output are explicit non-goals.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::{vec, vec::Vec};
use core::fmt;
use core::marker::PhantomData;
use core::ops::Range;
use core::sync::atomic::{AtomicU64, Ordering};

use capnp_wire::ElementSize;

use crate::{
    ArenaError, ExclusiveArena, ListOffset, MessageSegments, PrimitiveListValue, ResolvedPointer,
    ValidationError, WireLocation,
};

static NEXT_BUILD_PLAN_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParallelBuildOptions {
    pub requested_workers: usize,
    pub min_parallel_items: u32,
    pub min_items_per_partition: u32,
}

impl Default for ParallelBuildOptions {
    fn default() -> Self {
        Self {
            requested_workers: std::thread::available_parallelism().map_or(1, usize::from),
            min_parallel_items: 16 * 1024,
            min_items_per_partition: 4 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ParallelBuildError {
    Arena(ArenaError),
    Validation(ValidationError),
    ZeroWorkers,
    ZeroItemsPerPartition,
    RangeOverflow,
    LanesAlreadyIssued,
    SlotOutOfRange { index: u32, range: Range<u32> },
    SlotAlreadyInitialized { index: u32 },
    WrongBuildPlan,
    DuplicateLane { ordinal: usize },
    CapabilityUnsupported { index: u32 },
    SegmentLimit { requested: u32, limit: u32 },
    AllocationLimit { requested: u64, limit: u64 },
}

impl fmt::Display for ParallelBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for ParallelBuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Arena(error) => Some(error),
            Self::Validation(error) => Some(error),
            Self::ZeroWorkers
            | Self::ZeroItemsPerPartition
            | Self::RangeOverflow
            | Self::LanesAlreadyIssued
            | Self::SlotOutOfRange { .. }
            | Self::SlotAlreadyInitialized { .. }
            | Self::WrongBuildPlan
            | Self::DuplicateLane { .. }
            | Self::CapabilityUnsupported { .. }
            | Self::SegmentLimit { .. }
            | Self::AllocationLimit { .. } => None,
        }
    }
}

impl From<ArenaError> for ParallelBuildError {
    fn from(value: ArenaError) -> Self {
        Self::Arena(value)
    }
}

impl From<ValidationError> for ParallelBuildError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

/// A primitive root-list builder whose split method yields disjoint storage.
///
/// A second split cannot overlap the first split while its partitions live:
///
/// ```compile_fail
/// use capnp_message::{ParallelBuildOptions, PartitionedPrimitiveList};
/// let mut builder = PartitionedPrimitiveList::<u64>::new(16).unwrap();
/// let first = builder.partitions(ParallelBuildOptions::default()).unwrap();
/// let second = builder.partitions(ParallelBuildOptions::default()).unwrap();
/// drop((first, second));
/// ```
#[derive(Debug)]
pub struct PartitionedPrimitiveList<T: PrimitiveListValue> {
    arena: ExclusiveArena,
    reference: ListOffset,
    marker: PhantomData<T>,
}

impl<T: PrimitiveListValue> PartitionedPrimitiveList<T> {
    pub fn new(element_count: u32) -> Result<Self, ParallelBuildError> {
        let content_words = primitive_list_words(T::ELEMENT_SIZE, element_count)?;
        let total_words = content_words
            .checked_add(1)
            .ok_or(ParallelBuildError::RangeOverflow)?;
        let mut arena = ExclusiveArena::new(total_words, total_words)?;
        let reference = arena.init_root_list::<T>(element_count)?.offset();
        Ok(Self {
            arena,
            reference,
            marker: PhantomData,
        })
    }

    pub const fn len(&self) -> u32 {
        self.reference.element_count()
    }

    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn partitions(
        &mut self,
        options: ParallelBuildOptions,
    ) -> Result<Vec<PrimitiveBuildPartition<'_, T>>, ParallelBuildError> {
        let ranges = primitive_ranges(self.len(), T::ELEMENT_SIZE, options)?;
        let bits = element_bits(T::ELEMENT_SIZE);
        let storage = self.arena.primitive_list_storage_mut::<T>(self.reference)?;
        let mut remaining = storage;
        let mut byte_cursor = 0usize;
        let mut output = Vec::with_capacity(ranges.len());
        for (ordinal, range) in ranges.into_iter().enumerate() {
            let byte_start = bit_to_byte_floor(u64::from(range.start) * bits)?;
            let byte_end = bit_to_byte_ceil(u64::from(range.end) * bits)?;
            if byte_start != byte_cursor {
                return Err(ParallelBuildError::RangeOverflow);
            }
            let take = byte_end
                .checked_sub(byte_start)
                .ok_or(ParallelBuildError::RangeOverflow)?;
            let (bytes, tail) = remaining.split_at_mut(take);
            remaining = tail;
            byte_cursor = byte_end;
            output.push(PrimitiveBuildPartition {
                bytes,
                range,
                ordinal,
                marker: PhantomData,
            });
        }
        Ok(output)
    }

    pub fn finish(self) -> Result<Vec<Box<[u8]>>, ParallelBuildError> {
        Ok(vec![self.arena.into_segment()?])
    }
}

/// One uniquely borrowed primitive-list range.
#[derive(Debug)]
pub struct PrimitiveBuildPartition<'storage, T: PrimitiveListValue> {
    bytes: &'storage mut [u8],
    range: Range<u32>,
    ordinal: usize,
    marker: PhantomData<T>,
}

impl<T: PrimitiveListValue> PrimitiveBuildPartition<'_, T> {
    pub const fn ordinal(&self) -> usize {
        self.ordinal
    }

    pub fn range(&self) -> Range<u32> {
        self.range.clone()
    }

    pub fn len(&self) -> u32 {
        self.range.end - self.range.start
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    pub fn set(&mut self, local_index: u32, value: T) -> Result<(), ParallelBuildError> {
        if local_index >= self.len() {
            return Err(ParallelBuildError::SlotOutOfRange {
                index: local_index,
                range: 0..self.len(),
            });
        }
        let bit_offset = u64::from(local_index)
            .checked_mul(element_bits(T::ELEMENT_SIZE))
            .ok_or(ParallelBuildError::RangeOverflow)?;
        T::write_at(self.bytes, bit_offset, value)?;
        Ok(())
    }

    pub fn set_global(&mut self, index: u32, value: T) -> Result<(), ParallelBuildError> {
        let local = index.checked_sub(self.range.start).ok_or_else(|| {
            ParallelBuildError::SlotOutOfRange {
                index,
                range: self.range(),
            }
        })?;
        if index >= self.range.end {
            return Err(ParallelBuildError::SlotOutOfRange {
                index,
                range: self.range(),
            });
        }
        self.set(local, value)
    }
}

#[derive(Debug)]
struct SealedFragment {
    bytes: Box<[u8]>,
}

/// A worker-owned allocation lane for one non-overlapping pointer-list range.
///
/// Lanes are deliberately not cloneable:
///
/// ```compile_fail
/// use capnp_message::{ParallelBuildOptions, PartitionedPointerList};
/// let mut builder = PartitionedPointerList::new(8, 9, 128).unwrap();
/// let lane = builder.lanes(ParallelBuildOptions::default()).unwrap().remove(0);
/// let overlap = lane.clone();
/// drop((lane, overlap));
/// ```
#[derive(Debug)]
pub struct BuildLane {
    plan_id: u64,
    ordinal: usize,
    range: Range<u32>,
    fragments: Vec<Option<SealedFragment>>,
}

impl BuildLane {
    pub const fn ordinal(&self) -> usize {
        self.ordinal
    }

    pub fn range(&self) -> Range<u32> {
        self.range.clone()
    }

    pub fn len(&self) -> u32 {
        self.range.end - self.range.start
    }

    pub fn is_empty(&self) -> bool {
        self.range.is_empty()
    }

    /// Builds one private single-segment root and installs it transactionally.
    /// A closure error or panic drops the private arena and leaves the slot null.
    pub fn build_fragment(
        &mut self,
        index: u32,
        max_words: u32,
        build: impl FnOnce(&mut ExclusiveArena) -> Result<(), ArenaError>,
    ) -> Result<(), ParallelBuildError> {
        let relative = self.relative_index(index)?;
        if self.fragments[relative].is_some() {
            return Err(ParallelBuildError::SlotAlreadyInitialized { index });
        }
        let mut arena = ExclusiveArena::new(1, max_words)?;
        build(&mut arena)?;
        let bytes = arena.into_segment()?;
        if validate_fragment(&bytes)? {
            self.fragments[relative] = Some(SealedFragment { bytes });
        }
        Ok(())
    }

    fn relative_index(&self, index: u32) -> Result<usize, ParallelBuildError> {
        if !self.range.contains(&index) {
            return Err(ParallelBuildError::SlotOutOfRange {
                index,
                range: self.range(),
            });
        }
        usize::try_from(index - self.range.start).map_err(|_| ParallelBuildError::RangeOverflow)
    }

    pub fn seal(self) -> SealedBuildLane {
        SealedBuildLane {
            plan_id: self.plan_id,
            ordinal: self.ordinal,
            range: self.range,
            fragments: self.fragments,
        }
    }
}

/// An immutable worker result accepted only by its originating coordinator.
#[derive(Debug)]
#[must_use = "sealed lanes must be returned to their partitioned builder"]
pub struct SealedBuildLane {
    plan_id: u64,
    ordinal: usize,
    range: Range<u32>,
    fragments: Vec<Option<SealedFragment>>,
}

impl SealedBuildLane {
    pub const fn ordinal(&self) -> usize {
        self.ordinal
    }

    pub fn range(&self) -> Range<u32> {
        self.range.clone()
    }
}

/// Coordinator for a root pointer list and independently allocated fragments.
#[derive(Debug)]
pub struct PartitionedPointerList {
    plan_id: u64,
    parent: ExclusiveArena,
    reference: ListOffset,
    expected_ranges: Vec<Range<u32>>,
    lanes_issued: bool,
    max_segments: u32,
    max_total_words: u64,
}

impl PartitionedPointerList {
    pub fn new(
        element_count: u32,
        max_segments: u32,
        max_total_words: u64,
    ) -> Result<Self, ParallelBuildError> {
        if max_segments == 0 {
            return Err(ArenaError::InvalidSegmentLimit {
                requested: max_segments,
            }
            .into());
        }
        let parent_words = element_count
            .checked_add(1)
            .ok_or(ParallelBuildError::RangeOverflow)?;
        if u64::from(parent_words) > max_total_words {
            return Err(ParallelBuildError::AllocationLimit {
                requested: u64::from(parent_words),
                limit: max_total_words,
            });
        }
        let plan_id = NEXT_BUILD_PLAN_ID.fetch_add(1, Ordering::Relaxed);
        if plan_id == u64::MAX {
            return Err(ParallelBuildError::RangeOverflow);
        }
        let mut parent = ExclusiveArena::new(parent_words, parent_words)?;
        let reference = parent.init_root_pointer_list(element_count)?.offset();
        Ok(Self {
            plan_id,
            parent,
            reference,
            expected_ranges: Vec::new(),
            lanes_issued: false,
            max_segments,
            max_total_words,
        })
    }

    pub const fn len(&self) -> u32 {
        self.reference.element_count()
    }

    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn lanes(
        &mut self,
        options: ParallelBuildOptions,
    ) -> Result<Vec<BuildLane>, ParallelBuildError> {
        if self.lanes_issued {
            return Err(ParallelBuildError::LanesAlreadyIssued);
        }
        let ranges = balanced_ranges(self.len(), options)?;
        let mut lanes = Vec::with_capacity(ranges.len());
        for (ordinal, range) in ranges.iter().cloned().enumerate() {
            let count = usize::try_from(range.end - range.start)
                .map_err(|_| ParallelBuildError::RangeOverflow)?;
            let mut fragments = Vec::with_capacity(count);
            fragments.resize_with(count, || None);
            lanes.push(BuildLane {
                plan_id: self.plan_id,
                ordinal,
                range,
                fragments,
            });
        }
        self.expected_ranges = ranges;
        self.lanes_issued = true;
        Ok(lanes)
    }

    /// Finalizes sealed lanes in slot order. Missing lanes and slots stay null.
    pub fn finish(
        mut self,
        lanes: impl IntoIterator<Item = SealedBuildLane>,
    ) -> Result<Vec<Box<[u8]>>, ParallelBuildError> {
        let mut seen_lanes = BTreeSet::new();
        let mut fragments = BTreeMap::new();
        for lane in lanes {
            if lane.plan_id != self.plan_id
                || self.expected_ranges.get(lane.ordinal) != Some(&lane.range)
            {
                return Err(ParallelBuildError::WrongBuildPlan);
            }
            if !seen_lanes.insert(lane.ordinal) {
                return Err(ParallelBuildError::DuplicateLane {
                    ordinal: lane.ordinal,
                });
            }
            for (relative, fragment) in lane.fragments.into_iter().enumerate() {
                if let Some(fragment) = fragment {
                    let relative =
                        u32::try_from(relative).map_err(|_| ParallelBuildError::RangeOverflow)?;
                    let slot = lane
                        .range
                        .start
                        .checked_add(relative)
                        .ok_or(ParallelBuildError::RangeOverflow)?;
                    if fragments.insert(slot, fragment).is_some() {
                        return Err(ParallelBuildError::SlotAlreadyInitialized { index: slot });
                    }
                }
            }
        }

        let requested_segments = u32::try_from(fragments.len())
            .ok()
            .and_then(|count| count.checked_add(1))
            .ok_or(ParallelBuildError::RangeOverflow)?;
        if requested_segments > self.max_segments {
            return Err(ParallelBuildError::SegmentLimit {
                requested: requested_segments,
                limit: self.max_segments,
            });
        }
        let requested_words =
            fragments
                .values()
                .try_fold(self.parent.word_len(), |total, item| {
                    let words = u64::try_from(item.bytes.len() / 8)
                        .map_err(|_| ParallelBuildError::RangeOverflow)?;
                    total
                        .checked_add(words)
                        .ok_or(ParallelBuildError::RangeOverflow)
                })?;
        if requested_words > self.max_total_words {
            return Err(ParallelBuildError::AllocationLimit {
                requested: requested_words,
                limit: self.max_total_words,
            });
        }

        let mut children = Vec::with_capacity(fragments.len());
        for (slot, fragment) in fragments {
            let segment_id = u32::try_from(children.len())
                .ok()
                .and_then(|count| count.checked_add(1))
                .ok_or(ParallelBuildError::RangeOverflow)?;
            self.parent
                .set_pointer_list_far(self.reference, slot, segment_id)?;
            children.push(fragment.bytes);
        }
        let mut output = Vec::with_capacity(
            usize::try_from(requested_segments).map_err(|_| ParallelBuildError::RangeOverflow)?,
        );
        output.push(self.parent.into_segment()?);
        output.extend(children);
        Ok(output)
    }
}

fn validate_options(options: ParallelBuildOptions) -> Result<(), ParallelBuildError> {
    if options.requested_workers == 0 {
        Err(ParallelBuildError::ZeroWorkers)
    } else if options.min_items_per_partition == 0 {
        Err(ParallelBuildError::ZeroItemsPerPartition)
    } else {
        Ok(())
    }
}

fn balanced_ranges(
    len: u32,
    options: ParallelBuildOptions,
) -> Result<Vec<Range<u32>>, ParallelBuildError> {
    validate_options(options)?;
    if len == 0 {
        return Ok(Vec::new());
    }
    let maximum_useful = len.div_ceil(options.min_items_per_partition);
    let desired = if len < options.min_parallel_items {
        1
    } else {
        options
            .requested_workers
            .min(usize::try_from(maximum_useful).map_err(|_| ParallelBuildError::RangeOverflow)?)
            .max(1)
    };
    ranges_for_units(len, desired)
}

fn primitive_ranges(
    len: u32,
    size: ElementSize,
    options: ParallelBuildOptions,
) -> Result<Vec<Range<u32>>, ParallelBuildError> {
    validate_options(options)?;
    if len == 0 {
        return Ok(Vec::new());
    }
    if size == ElementSize::Void {
        return Ok(core::iter::once(0..len).collect());
    }
    if size != ElementSize::Bit {
        return balanced_ranges(len, options);
    }
    let bytes = len.div_ceil(8);
    let minimum_bytes = options.min_items_per_partition.div_ceil(8);
    let maximum_useful = bytes.div_ceil(minimum_bytes);
    let desired = if len < options.min_parallel_items {
        1
    } else {
        options
            .requested_workers
            .min(usize::try_from(maximum_useful).map_err(|_| ParallelBuildError::RangeOverflow)?)
            .max(1)
    };
    let byte_ranges = ranges_for_units(bytes, desired)?;
    Ok(byte_ranges
        .into_iter()
        .map(|range| range.start.saturating_mul(8).min(len)..range.end.saturating_mul(8).min(len))
        .collect())
}

fn ranges_for_units(units: u32, desired: usize) -> Result<Vec<Range<u32>>, ParallelBuildError> {
    let workers = u32::try_from(desired).map_err(|_| ParallelBuildError::RangeOverflow)?;
    let base = units / workers;
    let remainder = units % workers;
    let mut ranges = Vec::with_capacity(desired);
    let mut start = 0u32;
    for index in 0..workers {
        let count = base + u32::from(index < remainder);
        let end = start
            .checked_add(count)
            .ok_or(ParallelBuildError::RangeOverflow)?;
        ranges.push(start..end);
        start = end;
    }
    Ok(ranges)
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

fn primitive_list_words(size: ElementSize, count: u32) -> Result<u32, ParallelBuildError> {
    let bits = u64::from(count)
        .checked_mul(element_bits(size))
        .ok_or(ParallelBuildError::RangeOverflow)?;
    u32::try_from(bits.div_ceil(64)).map_err(|_| ParallelBuildError::RangeOverflow)
}

fn bit_to_byte_floor(bits: u64) -> Result<usize, ParallelBuildError> {
    usize::try_from(bits / 8).map_err(|_| ParallelBuildError::RangeOverflow)
}

fn bit_to_byte_ceil(bits: u64) -> Result<usize, ParallelBuildError> {
    usize::try_from(bits.div_ceil(8)).map_err(|_| ParallelBuildError::RangeOverflow)
}

fn validate_fragment(bytes: &[u8]) -> Result<bool, ParallelBuildError> {
    let segments = MessageSegments::new(&[bytes])?;
    let mut work = vec![WireLocation {
        segment_id: 0,
        word_offset: 0,
    }];
    let mut visited = BTreeSet::new();
    let mut root_non_null = false;
    while let Some(location) = work.pop() {
        if !visited.insert((location.segment_id, location.word_offset)) {
            continue;
        }
        let resolved = segments.validate_pointer(location)?;
        if location.segment_id == 0 && location.word_offset == 0 {
            root_non_null = !matches!(resolved, ResolvedPointer::Null);
        }
        match resolved {
            ResolvedPointer::Null => {}
            ResolvedPointer::Capability(capability) => {
                return Err(ParallelBuildError::CapabilityUnsupported {
                    index: capability.index,
                });
            }
            ResolvedPointer::Struct(reference) => {
                let first = add_location(reference.content, u64::from(reference.data_words))?;
                for index in 0..reference.pointer_count {
                    work.push(add_location(first, u64::from(index))?);
                }
            }
            ResolvedPointer::List(reference) if reference.element_size == ElementSize::Pointer => {
                for index in 0..reference.element_count {
                    work.push(add_location(reference.content, u64::from(index))?);
                }
            }
            ResolvedPointer::List(reference)
                if reference.element_size == ElementSize::InlineComposite =>
            {
                let (data_words, pointer_count) = reference
                    .inline_struct_size
                    .ok_or(ValidationError::InvalidInlineCompositeTag)?;
                let step = u64::from(data_words) + u64::from(pointer_count);
                for element in 0..reference.element_count {
                    let base = u64::from(element)
                        .checked_mul(step)
                        .and_then(|value| value.checked_add(u64::from(data_words)))
                        .ok_or(ParallelBuildError::RangeOverflow)?;
                    for pointer in 0..pointer_count {
                        work.push(add_location(reference.content, base + u64::from(pointer))?);
                    }
                }
            }
            ResolvedPointer::List(_) => {}
        }
    }
    Ok(root_non_null)
}

fn add_location(location: WireLocation, words: u64) -> Result<WireLocation, ParallelBuildError> {
    let word_offset = u64::from(location.word_offset)
        .checked_add(words)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or(ParallelBuildError::RangeOverflow)?;
    Ok(WireLocation {
        segment_id: location.segment_id,
        word_offset,
    })
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;
    use core::panic::AssertUnwindSafe;

    use super::*;
    use crate::{OwnedMessage, ReaderLimits};

    fn options(workers: usize, threshold: u32, minimum: u32) -> ParallelBuildOptions {
        ParallelBuildOptions {
            requested_workers: workers,
            min_parallel_items: threshold,
            min_items_per_partition: minimum,
        }
    }

    #[test]
    fn primitive_partitions_write_disjoint_ranges_and_leave_defaults() {
        let mut builder = PartitionedPrimitiveList::<u64>::new(10_000).expect("builder");
        std::thread::scope(|scope| {
            for mut partition in builder.partitions(options(4, 1, 1)).expect("partitions") {
                scope.spawn(move || {
                    for index in partition.range() {
                        if index % 3 != 0 {
                            partition.set_global(index, u64::from(index)).expect("set");
                        }
                    }
                });
            }
        });
        let message = OwnedMessage::new(
            builder.finish().expect("finish"),
            ReaderLimits {
                traversal_words: 10_000,
                nesting_levels: 8,
            },
        )
        .expect("message");
        message
            .root_list()
            .expect("root")
            .into_root()
            .with_reader(|reader| {
                let values = reader.as_primitive::<u64>().expect("u64");
                for index in 0..10_000 {
                    let expected = if index % 3 == 0 { 0 } else { u64::from(index) };
                    assert_eq!(values.get(index), Ok(expected));
                }
            })
            .expect("read");
    }

    #[test]
    fn pointer_lanes_seal_and_finalize_in_slot_order() {
        let mut builder = PartitionedPointerList::new(8, 9, 128).expect("builder");
        let lanes = builder.lanes(options(4, 1, 1)).expect("lanes");
        let sealed = std::thread::scope(|scope| {
            lanes
                .into_iter()
                .map(|mut lane| {
                    scope.spawn(move || {
                        for index in lane.range() {
                            lane.build_fragment(index, 8, |arena| {
                                let mut root = arena.init_root_struct(1, 0)?;
                                root.set_u64(0, u64::from(index) * 11, 0)
                            })?;
                        }
                        Ok::<_, ParallelBuildError>(lane.seal())
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|handle| handle.join().expect("worker").expect("lane"))
                .collect::<Vec<_>>()
        });
        let segments = builder.finish(sealed.into_iter().rev()).expect("finish");
        assert_eq!(segments.len(), 9);
        let message = OwnedMessage::new(segments, ReaderLimits::default()).expect("message");
        message
            .root_list()
            .expect("root")
            .into_root()
            .with_reader(|reader| {
                let pointers = reader.as_pointers().expect("pointers");
                for index in 0..8 {
                    let value = pointers
                        .get_struct(index)
                        .expect("struct")
                        .data_section()
                        .expect("data")
                        .read_u64(0, 0)
                        .expect("u64");
                    assert_eq!(value, u64::from(index) * 11);
                }
            })
            .expect("read");
    }

    #[test]
    #[allow(clippy::panic)]
    fn panic_and_missing_lanes_leave_null_defaults() {
        let mut builder = PartitionedPointerList::new(4, 5, 64).expect("builder");
        let mut lanes = builder.lanes(options(2, 1, 1)).expect("lanes");
        let mut first = lanes.remove(0);
        let panic = std::panic::catch_unwind(AssertUnwindSafe(|| {
            first
                .build_fragment(0, 8, |arena| {
                    let _root = arena.init_root_struct(1, 0)?;
                    panic!("cancel worker before seal");
                })
                .expect("unreachable");
        }));
        assert!(panic.is_err());
        first
            .build_fragment(1, 8, |arena| {
                let mut root = arena.init_root_struct(1, 0)?;
                root.set_u64(0, 99, 0)
            })
            .expect("surviving slot");
        let segments = builder.finish([first.seal()]).expect("finish partial");
        let message = OwnedMessage::new(segments, ReaderLimits::default()).expect("message");
        message
            .root_list()
            .expect("root")
            .into_root()
            .with_reader(|reader| {
                let pointers = reader.as_pointers().expect("pointers");
                assert!(matches!(
                    pointers.get(0).expect("slot zero").value.pointer,
                    ResolvedPointer::Null
                ));
                assert_eq!(
                    pointers
                        .get_struct(1)
                        .expect("struct")
                        .data_section()
                        .expect("data")
                        .read_u64(0, 0),
                    Ok(99)
                );
                assert!(matches!(
                    pointers.get(2).expect("missing lane").value.pointer,
                    ResolvedPointer::Null
                ));
                assert!(matches!(
                    pointers.get(3).expect("missing lane").value.pointer,
                    ResolvedPointer::Null
                ));
            })
            .expect("read");
    }

    #[test]
    fn capabilities_and_duplicate_slots_are_rejected() {
        let mut builder = PartitionedPointerList::new(1, 2, 16).expect("builder");
        let mut lane = builder
            .lanes(options(1, 1, 1))
            .expect("lanes")
            .pop()
            .expect("lane");
        assert_eq!(
            lane.build_fragment(0, 8, |arena| {
                let mut root = arena.init_root_struct(0, 1)?;
                root.set_capability(0, 42)
            }),
            Err(ParallelBuildError::CapabilityUnsupported { index: 42 })
        );
        lane.build_fragment(0, 8, |arena| {
            let _root = arena.init_root_struct(0, 0)?;
            Ok(())
        })
        .expect("first value");
        assert_eq!(
            lane.build_fragment(0, 8, |_| Ok(())),
            Err(ParallelBuildError::SlotAlreadyInitialized { index: 0 })
        );
    }

    #[test]
    fn public_worker_types_are_send_without_unsafe_traits() {
        fn require_send<T: Send>() {}
        require_send::<PrimitiveBuildPartition<'static, u64>>();
        require_send::<BuildLane>();
        require_send::<SealedBuildLane>();
        require_send::<PartitionedPointerList>();
        require_send::<PartitionedPrimitiveList<u64>>();
        require_send::<Arc<[u8]>>();
    }

    #[cfg(miri)]
    #[test]
    fn miri_disjoint_primitive_partitions_do_not_alias() {
        let mut builder = PartitionedPrimitiveList::<u64>::new(64).expect("builder");
        std::thread::scope(|scope| {
            for mut partition in builder.partitions(options(4, 1, 1)).expect("partitions") {
                scope.spawn(move || {
                    for index in partition.range() {
                        partition.set_global(index, u64::from(index)).expect("set");
                    }
                });
            }
        });
        assert_eq!(builder.finish().expect("finish").len(), 1);
    }

    #[cfg(all(target_has_atomic = "64", feature = "loom-tests"))]
    #[test]
    fn loom_lane_arrival_order_finalizes_deterministically() {
        use loom::sync::{Arc as LoomArc, Mutex as LoomMutex};
        use loom::thread as loom_thread;

        loom::model(|| {
            let mut builder = PartitionedPointerList::new(2, 3, 32).expect("builder");
            let lanes = builder.lanes(options(2, 1, 1)).expect("lanes");
            let completed = LoomArc::new(LoomMutex::new(Vec::new()));
            let handles = lanes
                .into_iter()
                .map(|mut lane| {
                    let completed = LoomArc::clone(&completed);
                    loom_thread::Builder::new()
                        .stack_size(64 * 1024)
                        .spawn(move || {
                            let index = lane.range().start;
                            lane.build_fragment(index, 4, |arena| {
                                let mut root = arena.init_root_struct(1, 0)?;
                                root.set_u64(0, u64::from(index) + 7, 0)
                            })
                            .expect("fragment");
                            completed.lock().expect("lock").push(lane.seal());
                        })
                        .expect("worker spawns")
                })
                .collect::<Vec<_>>();
            for handle in handles {
                handle.join().expect("worker");
            }
            let completed = LoomArc::try_unwrap(completed)
                .expect("workers released results")
                .into_inner()
                .expect("lock");
            let output = builder.finish(completed).expect("finish");
            let borrowed = output.iter().map(AsRef::as_ref).collect::<Vec<_>>();
            let segments = MessageSegments::new(&borrowed).expect("segments");
            let root = segments
                .validate_pointer(WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                })
                .expect("root")
                .list()
                .expect("list");
            for index in 0u32..2 {
                assert!(matches!(
                    segments
                        .validate_pointer(
                            add_location(root.content, u64::from(index)).expect("slot")
                        )
                        .expect("fragment"),
                    ResolvedPointer::Struct(_)
                ));
            }
        });
    }
}
