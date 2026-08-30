#![doc = "Safe Cap'n Proto messages, readers, builders, and traversal budgets."]

mod blob;
mod budget;
mod builder;
mod canonical;
mod list;
#[cfg(target_has_atomic = "64")]
mod owned;
mod primitive;
mod structure;
mod validation;

#[cfg(test)]
mod m07_oracle_tests;
#[cfg(test)]
mod m08_evolution_tests;
#[cfg(test)]
mod m09_list_tests;

pub use blob::{BlobError, DataReader, TextReader};
#[cfg(target_has_atomic = "64")]
pub use budget::SharedTraversalBudget;
pub use budget::{
    BudgetExhausted, LocalTraversalBudget, NestingLimit, NestingLimitExceeded, TraversalBudget,
};
pub use builder::{
    ArenaError, DataListBuilder, ExclusiveArena, GraphError, ListOffset, ListOrphan, Orphan,
    OrphanKind, PointerListBuilder, PrimitiveListValue, StructBuilder, StructListBuilder,
    StructOffset, StructOrphan, WordOffset,
};
pub use canonical::{CanonicalError, canonicalize, is_canonical};
pub use list::{
    EnumListIter, EnumListReader, ListReadError, ListReader, PointerListIter, PointerListReader,
    PrimitiveListElement, PrimitiveListIter, PrimitiveListReader, StructElementReader,
    StructListIter, StructListReader,
};
#[cfg(target_has_atomic = "64")]
pub use owned::{
    BorrowedMessage, ListObject, ObjectKind, ObjectRef, OwnedMessage, OwnedReadError, ReaderLimits,
    StructObject, TypedMessage,
};
pub use primitive::{DataSection, EnumValue, PrimitiveError, PrimitiveType, PrimitiveValue};
pub use structure::{
    PointerDefault, ResolvedPointerField, StructReadError, StructReader, UnionDiscriminant,
    UnionValue,
};
pub use validation::{
    BoundedPointer, CapabilityRef, ListRef, MessageSegments, ResolvedPointer, StructRef,
    TraversalError, TraversalStats, ValidationError, WireLocation,
};

#[cfg(test)]
#[allow(dead_code)]
mod m02_design_prototype {
    use core::marker::PhantomData;
    use std::sync::Arc;

    struct OwnedMessage {
        segments: Arc<[Arc<[u8]>]>,
    }

    #[derive(Clone, Copy)]
    struct WireLocation {
        segment: u32,
        word_offset: u32,
    }

    struct ObjectRef<T> {
        message: Arc<OwnedMessage>,
        location: WireLocation,
        marker: PhantomData<T>,
    }

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn owned_reader_shapes_derive_thread_safety() {
        assert_send_sync::<OwnedMessage>();
        assert_send_sync::<ObjectRef<()>>();
    }
}
