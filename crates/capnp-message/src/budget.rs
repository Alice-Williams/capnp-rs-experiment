//! Exact traversal-work and per-reader nesting limits.
//!
//! The behavior follows the read-limiter and amplified-list rules in the
//! pinned C++ Cap'n Proto implementation, strengthened by ADR-0002 so a shared
//! limit is exact under concurrency. A charge either deducts its entire word
//! count or leaves the balance unchanged. Physical message-size and allocation
//! limits remain separate concerns owned by framing and arena code.
//!
//! Nesting is deliberately a copied value. Descending one reader cannot consume
//! another branch's depth. Targets without 64-bit atomics expose only the local
//! budget; neither budget attempts leases or approximate per-thread accounting.

use core::cell::Cell;
use core::fmt;

#[cfg(all(target_has_atomic = "64", not(feature = "loom-tests")))]
use core::sync::atomic::{AtomicU64, Ordering};
#[cfg(all(target_has_atomic = "64", feature = "loom-tests"))]
use loom::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BudgetExhausted {
    pub requested_words: u64,
    pub remaining_words: u64,
}

impl fmt::Display for BudgetExhausted {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "traversal requires {} words but only {} remain",
            self.requested_words, self.remaining_words
        )
    }
}

impl core::error::Error for BudgetExhausted {}

mod sealed {
    pub trait Sealed {}
}

/// A hard word-denominated traversal limit.
///
/// This trait is sealed so callers cannot accidentally substitute an
/// approximate implementation at a security boundary.
pub trait TraversalBudget: sealed::Sealed {
    fn try_charge(&self, words: u64) -> Result<(), BudgetExhausted>;
    fn remaining_words(&self) -> u64;
}

/// Exact, non-atomic accounting for a single-threaded reader context.
///
/// ```
/// use capnp_message::{LocalTraversalBudget, TraversalBudget};
/// let budget = LocalTraversalBudget::new(2);
/// budget.try_charge(2)?;
/// assert_eq!(budget.remaining_words(), 0);
/// # Ok::<(), capnp_message::BudgetExhausted>(())
/// ```
///
/// A local budget cannot accidentally cross a shared-reader boundary:
///
/// ```compile_fail
/// use capnp_message::LocalTraversalBudget;
/// fn require_sync<T: Sync>() {}
/// require_sync::<LocalTraversalBudget>();
/// ```
#[derive(Debug)]
pub struct LocalTraversalBudget {
    remaining: Cell<u64>,
}

impl LocalTraversalBudget {
    pub const fn new(limit_words: u64) -> Self {
        Self {
            remaining: Cell::new(limit_words),
        }
    }
}

impl sealed::Sealed for LocalTraversalBudget {}

impl TraversalBudget for LocalTraversalBudget {
    fn try_charge(&self, words: u64) -> Result<(), BudgetExhausted> {
        let remaining = self.remaining.get();
        match remaining.checked_sub(words) {
            Some(next) => {
                self.remaining.set(next);
                Ok(())
            }
            None => Err(BudgetExhausted {
                requested_words: words,
                remaining_words: remaining,
            }),
        }
    }

    fn remaining_words(&self) -> u64 {
        self.remaining.get()
    }
}

/// Exact accounting shared by concurrent immutable readers.
///
/// `AcqRel` on a successful update and `Acquire` on failure make a completed
/// deduction visible before another reader observes its balance. The compare-
/// exchange loop supplied by `fetch_update` cannot admit a partial charge.
///
/// ```
/// use capnp_message::SharedTraversalBudget;
/// fn require_send_sync<T: Send + Sync>() {}
/// require_send_sync::<SharedTraversalBudget>();
/// ```
#[cfg(target_has_atomic = "64")]
#[derive(Debug)]
pub struct SharedTraversalBudget {
    remaining: AtomicU64,
}

#[cfg(target_has_atomic = "64")]
impl SharedTraversalBudget {
    pub fn new(limit_words: u64) -> Self {
        Self {
            remaining: AtomicU64::new(limit_words),
        }
    }
}

#[cfg(target_has_atomic = "64")]
impl sealed::Sealed for SharedTraversalBudget {}

#[cfg(target_has_atomic = "64")]
impl TraversalBudget for SharedTraversalBudget {
    fn try_charge(&self, words: u64) -> Result<(), BudgetExhausted> {
        self.remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
                remaining.checked_sub(words)
            })
            .map(|_| ())
            .map_err(|remaining| BudgetExhausted {
                requested_words: words,
                remaining_words: remaining,
            })
    }

    fn remaining_words(&self) -> u64 {
        self.remaining.load(Ordering::Acquire)
    }
}

/// Immutable depth remaining for one reader branch.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NestingLimit(u32);

impl NestingLimit {
    pub const fn new(levels: u32) -> Self {
        Self(levels)
    }

    pub const fn remaining(self) -> u32 {
        self.0
    }

    pub const fn descend(self) -> Result<Self, NestingLimitExceeded> {
        match self.0.checked_sub(1) {
            Some(remaining) => Ok(Self(remaining)),
            None => Err(NestingLimitExceeded),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NestingLimitExceeded;

impl fmt::Display for NestingLimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("message is too deeply nested or contains a cycle")
    }
}

impl core::error::Error for NestingLimitExceeded {}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn local_charge_is_complete_or_unchanged() {
        let budget = LocalTraversalBudget::new(5);
        assert_eq!(budget.try_charge(3), Ok(()));
        assert_eq!(budget.remaining_words(), 2);
        assert_eq!(
            budget.try_charge(3),
            Err(BudgetExhausted {
                requested_words: 3,
                remaining_words: 2,
            })
        );
        assert_eq!(budget.remaining_words(), 2);
        assert_eq!(budget.try_charge(2), Ok(()));
        assert_eq!(budget.remaining_words(), 0);
    }

    #[test]
    fn sibling_nesting_limits_are_independent_values() {
        let parent = NestingLimit::new(2);
        let left = parent.descend().expect("left branch has one level");
        let right = parent.descend().expect("right branch has one level");
        assert_eq!(left.remaining(), 1);
        assert_eq!(right.remaining(), 1);
        assert_eq!(left.descend().expect("last level").remaining(), 0);
        assert_eq!(right.remaining(), 1);
        assert_eq!(NestingLimit::new(0).descend(), Err(NestingLimitExceeded));
    }

    #[cfg(all(target_has_atomic = "64", not(feature = "loom-tests")))]
    #[test]
    fn concurrent_charges_never_exceed_the_shared_limit() {
        use std::sync::Arc;

        let budget = Arc::new(SharedTraversalBudget::new(1_000));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let budget = Arc::clone(&budget);
                std::thread::spawn(move || {
                    let mut successes = 0;
                    while budget.try_charge(3).is_ok() {
                        successes += 1;
                    }
                    successes
                })
            })
            .collect();
        let successes: u64 = handles
            .into_iter()
            .map(|handle| handle.join().expect("worker did not panic"))
            .sum();
        assert_eq!(successes * 3 + budget.remaining_words(), 1_000);
        assert!(budget.remaining_words() < 3);
    }

    #[cfg(all(target_has_atomic = "64", feature = "loom-tests"))]
    #[test]
    fn loom_proves_competing_charges_preserve_the_hard_limit() {
        use loom::sync::Arc;
        use loom::thread;

        loom::model(|| {
            let budget = Arc::new(SharedTraversalBudget::new(3));
            let first = {
                let budget = Arc::clone(&budget);
                thread::spawn(move || budget.try_charge(2).is_ok())
            };
            let second = {
                let budget = Arc::clone(&budget);
                thread::spawn(move || budget.try_charge(2).is_ok())
            };
            let successes = u64::from(first.join().expect("first worker did not panic"))
                + u64::from(second.join().expect("second worker did not panic"));
            assert_eq!(successes, 1);
            assert_eq!(successes * 2 + budget.remaining_words(), 3);
        });
    }
}
