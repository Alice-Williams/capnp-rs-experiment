#![doc = "Safe Cap'n Proto messages, readers, builders, and traversal budgets."]

mod validation;

pub use validation::{
    CapabilityRef, ListRef, MessageSegments, ResolvedPointer, StructRef, ValidationError,
    WireLocation,
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
