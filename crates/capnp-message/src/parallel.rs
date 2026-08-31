//! Zero-copy planning for concurrent immutable reads.
//!
//! A list plan charges and validates its root target exactly once, then gives
//! workers immutable coordinate/range pairs. Reopening a partition does not
//! charge the already-approved list body again; pointer children still use the
//! message's shared exact budget. This is an optimization of one logical
//! dereference, not a traversal-limit bypass.
//!
//! Planning does not depend on Rayon. `ListPartition` and `SubtreeBatch` are
//! `Send + Sync` values suitable for any scoped executor, while
//! `map_reduce_scoped()` supplies a small standard-library implementation.
//! Mutation and parallel construction belong to M30.
//!
//! Compatibility is anchored in the pinned C++ traversal limiter and list
//! decoding rules. The partitioning policy itself is a native scheduling API:
//! it deliberately makes no wire-format changes and claims no compatibility
//! with a C++ executor or with Rayon scheduling details.

use alloc::vec::Vec;
use core::fmt;
use core::ops::Range;

use std::thread;

use crate::{
    ListObject, ListReader, ListRef, NestingLimit, ObjectKind, ObjectRef, OwnedReadError,
    SharedTraversalBudget,
};

/// Scheduling thresholds for list and subtree planning.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ParallelReadOptions {
    pub requested_workers: usize,
    pub min_parallel_items: u32,
    pub min_items_per_partition: u32,
}

impl Default for ParallelReadOptions {
    fn default() -> Self {
        Self {
            requested_workers: std::thread::available_parallelism().map_or(1, usize::from),
            min_parallel_items: 16 * 1024,
            min_items_per_partition: 4 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlanError {
    Read(OwnedReadError),
    ZeroWorkers,
    ZeroItemsPerPartition,
    RangeOverflow,
}

impl fmt::Display for PlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for PlanError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read(error) => Some(error),
            Self::ZeroWorkers | Self::ZeroItemsPerPartition | Self::RangeOverflow => None,
        }
    }
}

impl From<OwnedReadError> for PlanError {
    fn from(value: OwnedReadError) -> Self {
        Self::Read(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MapReduceError<E> {
    Map(E),
    WorkerPanicked { partition: usize },
}

impl<E: fmt::Display> fmt::Display for MapReduceError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Map(error) => write!(formatter, "parallel map failed: {error}"),
            Self::WorkerPanicked { partition } => {
                write!(formatter, "parallel partition {partition} panicked")
            }
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for MapReduceError<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Map(error) => Some(error),
            Self::WorkerPanicked { .. } => None,
        }
    }
}

/// One contiguous, non-overlapping range of a precharged immutable list.
#[derive(Clone, Debug)]
pub struct ListPartition {
    list: ObjectRef<ListObject>,
    reference: Option<ListRef>,
    nesting: NestingLimit,
    ordinal: usize,
    range: Range<u32>,
}

impl ListPartition {
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

    pub fn list(&self) -> &ObjectRef<ListObject> {
        &self.list
    }

    /// Opens the already-charged list while retaining shared accounting for
    /// every pointer followed from its elements.
    ///
    /// The reader cannot escape the callback:
    ///
    /// ```compile_fail
    /// use capnp_message::{ListPartition, ListReader, SharedTraversalBudget};
    /// fn escape(
    ///     partition: &ListPartition,
    /// ) -> ListReader<'static, 'static, SharedTraversalBudget> {
    ///     partition.with_reader(|reader, _| reader).unwrap()
    /// }
    /// ```
    pub fn with_reader<R>(
        &self,
        use_reader: impl for<'reader> FnOnce(
            ListReader<'reader, 'reader, SharedTraversalBudget>,
            Range<u32>,
        ) -> R,
    ) -> Result<R, OwnedReadError> {
        self.list
            .with_precharged_reader(self.reference, self.nesting, |reader| {
                use_reader(reader, self.range())
            })
    }
}

/// Deterministic balanced ranges over one shared immutable list.
#[derive(Clone, Debug)]
pub struct ListPartitionPlan {
    list_len: u32,
    charged_words: u64,
    partitions: Vec<ListPartition>,
}

impl ListPartitionPlan {
    pub fn new(
        list: ObjectRef<ListObject>,
        options: ParallelReadOptions,
    ) -> Result<Self, PlanError> {
        validate_options(options)?;
        let (reference, nesting, charged_words) = list.precharge_for_partitions()?;
        let list_len = reference.map_or(0, |value| value.element_count);
        let ranges = balanced_ranges(list_len, options)?;
        let partitions = ranges
            .into_iter()
            .enumerate()
            .map(|(ordinal, range)| ListPartition {
                list: list.clone(),
                reference,
                nesting,
                ordinal,
                range,
            })
            .collect();
        Ok(Self {
            list_len,
            charged_words,
            partitions,
        })
    }

    pub const fn list_len(&self) -> u32 {
        self.list_len
    }

    pub const fn charged_words(&self) -> u64 {
        self.charged_words
    }

    pub fn partitions(&self) -> &[ListPartition] {
        &self.partitions
    }

    pub fn into_partitions(self) -> Vec<ListPartition> {
        self.partitions
    }

    /// Maps partitions in scoped workers and reduces results in deterministic
    /// partition order. A one-partition plan executes entirely on the caller.
    pub fn map_reduce_scoped<R, E, M, I, Reduce>(
        &self,
        map: M,
        identity: I,
        reduce: Reduce,
    ) -> Result<R, MapReduceError<E>>
    where
        R: Send,
        E: Send,
        M: Fn(&ListPartition) -> Result<R, E> + Sync,
        I: Fn() -> R,
        Reduce: Fn(R, R) -> R,
    {
        if self.partitions.len() <= 1 {
            return self
                .partitions
                .first()
                .map_or_else(|| Ok(identity()), &map)
                .map_err(MapReduceError::Map);
        }
        thread::scope(|scope| {
            let handles = self
                .partitions
                .iter()
                .map(|partition| scope.spawn(|| map(partition)))
                .collect::<Vec<_>>();
            let mut output = identity();
            for (partition, handle) in handles.into_iter().enumerate() {
                let mapped = handle
                    .join()
                    .map_err(|_| MapReduceError::WorkerPanicked { partition })?
                    .map_err(MapReduceError::Map)?;
                output = reduce(output, mapped);
            }
            Ok(output)
        })
    }
}

fn validate_options(options: ParallelReadOptions) -> Result<(), PlanError> {
    if options.requested_workers == 0 {
        Err(PlanError::ZeroWorkers)
    } else if options.min_items_per_partition == 0 {
        Err(PlanError::ZeroItemsPerPartition)
    } else {
        Ok(())
    }
}

fn balanced_ranges(len: u32, options: ParallelReadOptions) -> Result<Vec<Range<u32>>, PlanError> {
    if len == 0 {
        return Ok(Vec::new());
    }
    let maximum_useful = len.div_ceil(options.min_items_per_partition);
    let desired = if len < options.min_parallel_items {
        1
    } else {
        options
            .requested_workers
            .min(usize::try_from(maximum_useful).map_err(|_| PlanError::RangeOverflow)?)
            .max(1)
    };
    let workers = u32::try_from(desired).map_err(|_| PlanError::RangeOverflow)?;
    let base = len / workers;
    let remainder = len % workers;
    let mut ranges = Vec::with_capacity(desired);
    let mut start = 0u32;
    for index in 0..workers {
        let count = base + u32::from(index < remainder);
        let end = start.checked_add(count).ok_or(PlanError::RangeOverflow)?;
        ranges.push(start..end);
        start = end;
    }
    Ok(ranges)
}

/// One independent retained subtree and its caller-supplied work estimate.
#[derive(Debug)]
pub struct SubtreeWork<T: ObjectKind> {
    object: ObjectRef<T>,
    estimated_words: u64,
    ordinal: usize,
}

impl<T: ObjectKind> Clone for SubtreeWork<T> {
    fn clone(&self) -> Self {
        Self {
            object: self.object.clone(),
            estimated_words: self.estimated_words,
            ordinal: self.ordinal,
        }
    }
}

impl<T: ObjectKind> SubtreeWork<T> {
    pub fn new(object: ObjectRef<T>, estimated_words: u64) -> Self {
        Self {
            object,
            estimated_words: estimated_words.max(1),
            ordinal: 0,
        }
    }

    pub fn object(&self) -> &ObjectRef<T> {
        &self.object
    }

    pub const fn estimated_words(&self) -> u64 {
        self.estimated_words
    }

    pub const fn ordinal(&self) -> usize {
        self.ordinal
    }
}

/// A deterministic work bin containing independent zero-copy subtrees.
#[derive(Clone, Debug)]
pub struct SubtreeBatch<T: ObjectKind> {
    items: Vec<SubtreeWork<T>>,
    estimated_words: u64,
}

impl<T: ObjectKind> SubtreeBatch<T> {
    pub fn items(&self) -> &[SubtreeWork<T>] {
        &self.items
    }

    pub const fn estimated_words(&self) -> u64 {
        self.estimated_words
    }
}

/// Greedy longest-first balancing for independently retained subtrees.
#[derive(Clone, Debug)]
pub struct SubtreePlan<T: ObjectKind> {
    batches: Vec<SubtreeBatch<T>>,
}

impl<T: ObjectKind> SubtreePlan<T> {
    pub fn new(
        mut items: Vec<SubtreeWork<T>>,
        options: ParallelReadOptions,
    ) -> Result<Self, PlanError> {
        validate_options(options)?;
        if items.is_empty() {
            return Ok(Self {
                batches: Vec::new(),
            });
        }
        for (ordinal, item) in items.iter_mut().enumerate() {
            item.ordinal = ordinal;
        }
        let minimum_items = usize::try_from(options.min_items_per_partition)
            .map_err(|_| PlanError::RangeOverflow)?;
        let threshold =
            usize::try_from(options.min_parallel_items).map_err(|_| PlanError::RangeOverflow)?;
        let maximum_useful = items.len().div_ceil(minimum_items);
        let worker_count = if items.len() < threshold {
            1
        } else {
            options.requested_workers.min(maximum_useful).max(1)
        };
        items.sort_by(|left, right| {
            right
                .estimated_words
                .cmp(&left.estimated_words)
                .then_with(|| left.ordinal.cmp(&right.ordinal))
        });
        let mut batches = (0..worker_count)
            .map(|_| SubtreeBatch {
                items: Vec::new(),
                estimated_words: 0,
            })
            .collect::<Vec<_>>();
        for item in items {
            let target = batches
                .iter()
                .enumerate()
                .min_by_key(|(index, batch)| (batch.estimated_words, *index))
                .map(|(index, _)| index)
                .ok_or(PlanError::RangeOverflow)?;
            batches[target].estimated_words = batches[target]
                .estimated_words
                .saturating_add(item.estimated_words);
            batches[target].items.push(item);
        }
        for batch in &mut batches {
            batch.items.sort_by_key(|item| item.ordinal);
        }
        Ok(Self { batches })
    }

    pub fn batches(&self) -> &[SubtreeBatch<T>] {
        &self.batches
    }

    pub fn into_batches(self) -> Vec<SubtreeBatch<T>> {
        self.batches
    }
}

#[cfg(test)]
mod tests {
    use alloc::string::ToString;
    use alloc::sync::Arc;

    use super::*;
    use crate::{ExclusiveArena, OwnedMessage, ReaderLimits, TraversalBudget};

    fn u64_list(len: u32, traversal_words: u64) -> ObjectRef<ListObject> {
        let arena_words = len.checked_add(1).expect("test list fits the arena");
        let mut arena = ExclusiveArena::new(arena_words, arena_words).expect("arena");
        {
            let mut values = arena.init_root_list::<u64>(len).expect("root list");
            for index in 0..len {
                values.set(index, u64::from(index)).expect("set element");
            }
        }
        OwnedMessage::new(
            arena.into_segments(),
            ReaderLimits {
                traversal_words,
                nesting_levels: 8,
            },
        )
        .expect("owned message")
        .root_list()
        .expect("list root")
        .into_root()
    }

    fn options(workers: usize, threshold: u32, minimum: u32) -> ParallelReadOptions {
        ParallelReadOptions {
            requested_workers: workers,
            min_parallel_items: threshold,
            min_items_per_partition: minimum,
        }
    }

    #[test]
    fn balanced_partitions_cover_each_index_once_and_small_inputs_stay_serial() {
        let small =
            ListPartitionPlan::new(u64_list(15, 15), options(4, 16, 2)).expect("small plan");
        assert_eq!(small.partitions().len(), 1);
        assert_eq!(small.partitions()[0].range(), 0..15);

        let plan =
            ListPartitionPlan::new(u64_list(19, 19), options(4, 16, 2)).expect("parallel plan");
        assert_eq!(plan.partitions().len(), 4);
        let ranges = plan
            .partitions()
            .iter()
            .map(ListPartition::range)
            .collect::<Vec<_>>();
        assert_eq!(ranges, [0..5, 5..10, 10..15, 15..19]);
        assert_eq!(plan.charged_words(), 19);
    }

    #[test]
    fn scoped_map_reduce_is_zero_copy_and_charges_the_list_once() {
        let list = u64_list(100_000, 100_000);
        let message = Arc::clone(list.message());
        let plan = ListPartitionPlan::new(list, options(4, 1, 1)).expect("plan");
        assert_eq!(message.remaining_traversal_words(), 0);
        let sum = plan
            .map_reduce_scoped(
                |partition| {
                    partition
                        .with_reader(|reader, range| {
                            let values = reader.as_primitive::<u64>()?;
                            range
                                .map(|index| values.get(index))
                                .try_fold(0u64, |sum, value| {
                                    sum.checked_add(value?)
                                        .ok_or(crate::ListReadError::RangeOverflow)
                                })
                        })
                        .map_err(|error| error.to_string())?
                        .map_err(|error| error.to_string())
                },
                || 0u64,
                |left, right| left + right,
            )
            .expect("parallel sum");
        assert_eq!(sum, 4_999_950_000);
        assert_eq!(message.remaining_traversal_words(), 0);
    }

    #[test]
    fn repeated_parallel_reads_do_not_refund_or_overcharge_the_precharged_body() {
        let list = u64_list(1024, 1024);
        let message = Arc::clone(list.message());
        let plan = ListPartitionPlan::new(list, options(8, 1, 1)).expect("plan");
        thread::scope(|scope| {
            for partition in plan.partitions() {
                scope.spawn(move || {
                    for _ in 0..32 {
                        partition
                            .with_reader(|reader, range| {
                                let values = reader.as_primitive::<u64>().expect("u64 list");
                                for index in range {
                                    assert_eq!(values.get(index), Ok(u64::from(index)));
                                }
                            })
                            .expect("partition read");
                    }
                });
            }
        });
        assert_eq!(message.remaining_traversal_words(), 0);
    }

    #[test]
    fn subtree_planning_is_deterministic_balanced_and_keeps_shared_objects() {
        let list = u64_list(8, 8);
        let items = [8, 7, 6, 5, 4, 3, 2, 1]
            .into_iter()
            .map(|weight| SubtreeWork::new(list.clone(), weight))
            .collect();
        let plan = SubtreePlan::new(items, options(4, 1, 1)).expect("subtree plan");
        assert_eq!(plan.batches().len(), 4);
        let loads = plan
            .batches()
            .iter()
            .map(SubtreeBatch::estimated_words)
            .collect::<Vec<_>>();
        assert_eq!(loads, [9, 9, 9, 9]);
        for item in plan.batches().iter().flat_map(|batch| batch.items()) {
            assert!(item.object().same_object(&list));
        }
    }

    #[test]
    fn invalid_options_fail_before_charging() {
        let list = u64_list(4, 4);
        let message = Arc::clone(list.message());
        assert_eq!(
            ListPartitionPlan::new(list.clone(), options(0, 1, 1)).expect_err("zero workers"),
            PlanError::ZeroWorkers
        );
        assert_eq!(message.remaining_traversal_words(), 4);
        assert_eq!(
            ListPartitionPlan::new(list, options(1, 1, 0)).expect_err("zero minimum"),
            PlanError::ZeroItemsPerPartition
        );
        assert_eq!(message.remaining_traversal_words(), 4);
        assert_eq!(crate::SharedTraversalBudget::new(1).remaining_words(), 1);
    }

    #[test]
    fn public_plans_and_partitions_are_send_and_sync() {
        fn require_send_sync<T: Send + Sync>() {}
        require_send_sync::<ListPartition>();
        require_send_sync::<ListPartitionPlan>();
        require_send_sync::<SubtreeWork<ListObject>>();
        require_send_sync::<SubtreeBatch<ListObject>>();
        require_send_sync::<SubtreePlan<ListObject>>();
    }

    #[test]
    fn scoped_map_reduce_reports_the_panicking_partition() {
        let plan = ListPartitionPlan::new(u64_list(8, 8), options(4, 1, 1)).expect("plan");
        let error = plan
            .map_reduce_scoped(
                |partition| {
                    assert_ne!(partition.ordinal(), 2, "intentional worker panic");
                    Ok::<_, ()>(partition.len())
                },
                || 0,
                |left, right| left + right,
            )
            .expect_err("panic is reported");
        assert_eq!(error, MapReduceError::WorkerPanicked { partition: 2 });
    }

    #[cfg(all(target_has_atomic = "64", feature = "loom-tests"))]
    #[test]
    fn loom_parallel_plan_preserves_precharge_and_nested_budget() {
        use loom::sync::Arc as LoomArc;
        use loom::thread as loom_thread;

        loom::model(|| {
            let budget = LoomArc::new(SharedTraversalBudget::new(7));
            budget.try_charge(4).expect("root list is charged once");
            let left = {
                let budget = LoomArc::clone(&budget);
                loom_thread::spawn(move || budget.try_charge(2).is_ok())
            };
            let right = {
                let budget = LoomArc::clone(&budget);
                loom_thread::spawn(move || budget.try_charge(2).is_ok())
            };
            let nested_successes = u64::from(left.join().expect("left worker"))
                + u64::from(right.join().expect("right worker"));
            assert_eq!(4 + nested_successes * 2 + budget.remaining_words(), 7);
            assert_eq!(nested_successes, 1);
        });
    }
}
