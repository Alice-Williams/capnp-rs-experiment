//! Schema-independent canonical Cap'n Proto messages.
//!
//! The compatibility oracle is the pinned C++ implementation's
//! `MessageReader::isCanonical()` and `layout.c++` canonical traversal. A
//! canonical message has exactly one segment, a root pointer in word zero,
//! objects allocated densely in pointer preorder, no far or capability
//! pointers, minimal struct sections, exact inline-composite element widths,
//! and zero padding after primitive-list elements. Canonicalization duplicates
//! aliases as required by preorder; traversal and output limits bound cycles
//! and amplification.
//!
//! This module deliberately does not provide schema-aware default elision,
//! packed framing, capability serialization, or cryptographic normalization.

use core::fmt;

use crate::{ExclusiveArena, GraphError, MessageSegments, NestingLimit, TraversalBudget};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CanonicalError {
    Graph(GraphError),
}

impl fmt::Display for CanonicalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Graph(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for CanonicalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Graph(error) => Some(error),
        }
    }
}

impl From<GraphError> for CanonicalError {
    fn from(value: GraphError) -> Self {
        Self::Graph(value)
    }
}

/// Rewrites a valid message into the canonical, unframed one-segment form.
///
/// `max_output_words` includes the root pointer word and is independent of the
/// source traversal budget. Capabilities have no canonical wire encoding and
/// therefore return [`GraphError::CapabilityNotCanonicalizable`].
///
/// ```
/// use capnp_message::{
///     LocalTraversalBudget, MessageSegments, NestingLimit, canonicalize, is_canonical,
/// };
///
/// let null_root = [0_u8; 8];
/// let message = MessageSegments::new(&[&null_root])?;
/// let output = canonicalize(
///     &message,
///     &LocalTraversalBudget::new(16),
///     NestingLimit::new(8),
///     16,
/// )?;
/// let normalized = MessageSegments::new(&[&output])?;
/// assert!(is_canonical(
///     &normalized,
///     &LocalTraversalBudget::new(16),
///     NestingLimit::new(8),
///     16,
/// )?);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn canonicalize<B: TraversalBudget>(
    source: &MessageSegments<'_>,
    budget: &B,
    nesting: NestingLimit,
    max_output_words: u32,
) -> Result<Box<[u8]>, CanonicalError> {
    ExclusiveArena::canonicalize_from(source, budget, nesting, max_output_words)
        .map_err(CanonicalError::from)
}

/// Checks whether `source` is already its exact canonical normal form.
///
/// A capability pointer is well-formed but never canonical, so it produces
/// `Ok(false)`. Malformed input and exhausted safety limits remain errors.
pub fn is_canonical<B: TraversalBudget>(
    source: &MessageSegments<'_>,
    budget: &B,
    nesting: NestingLimit,
    max_output_words: u32,
) -> Result<bool, CanonicalError> {
    if source.segment_count() != 1 {
        return Ok(false);
    }
    let canonical = match canonicalize(source, budget, nesting, max_output_words) {
        Ok(canonical) => canonical,
        Err(CanonicalError::Graph(GraphError::CapabilityNotCanonicalizable { .. })) => {
            return Ok(false);
        }
        Err(error) => return Err(error),
    };
    Ok(source.segment(0) == Some(canonical.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use capnp_wire::{ElementSize, WirePointer};

    use crate::{LocalTraversalBudget, WireLocation};

    const ROOT: WireLocation = WireLocation {
        segment_id: 0,
        word_offset: 0,
    };

    fn message(segment: &[u8]) -> MessageSegments<'_> {
        MessageSegments::new(&[segment]).expect("test segment is aligned")
    }

    fn check(segment: &[u8]) -> bool {
        is_canonical(
            &message(segment),
            &LocalTraversalBudget::new(1_000),
            NestingLimit::new(100),
            1_000,
        )
        .expect("test message is valid")
    }

    fn normalize(segment: &[u8]) -> Box<[u8]> {
        canonicalize(
            &message(segment),
            &LocalTraversalBudget::new(1_000),
            NestingLimit::new(100),
            1_000,
        )
        .expect("test message canonicalizes")
    }

    fn write(words: &mut [u8], index: usize, pointer: WirePointer) {
        pointer
            .write_to(words, index * 8)
            .expect("test pointer fits");
    }

    #[test]
    fn canonicalization_preserves_values_and_is_idempotent() {
        let mut arena = ExclusiveArena::new(1, 64).expect("arena allocates");
        {
            let mut root = arena.init_root_struct(3, 2).expect("root initializes");
            root.set_u64(0, 0xfeed_face_dead_beef, 0)
                .expect("value fits");
            let mut bits = root.init_list::<bool>(0, 9).expect("bit list initializes");
            bits.set(0, true).expect("bit fits");
            bits.set(8, true).expect("bit fits");
        }
        let source = arena.into_segment().expect("one segment");
        assert!(!check(&source));

        let first = normalize(&source);
        assert!(check(&first));
        let second = normalize(&first);
        assert_eq!(first, second);

        let parsed = message(&first);
        let root = parsed
            .validate_pointer(ROOT)
            .expect("root validates")
            .structure()
            .expect("root is a struct");
        assert_eq!(root.data_words, 1);
        assert_eq!(root.pointer_count, 1);
        assert_eq!(&first[8..16], &0xfeed_face_dead_beef_u64.to_le_bytes());
        let list_location = WireLocation {
            segment_id: 0,
            word_offset: root.content.word_offset + u32::from(root.data_words),
        };
        let bits = parsed
            .validate_pointer(list_location)
            .expect("list validates")
            .list()
            .expect("field is a list");
        assert_eq!(bits.element_size, ElementSize::Bit);
        assert_eq!(bits.element_count, 9);
        let bits_start =
            usize::try_from(bits.content.word_offset).expect("test offset fits usize") * 8;
        assert_eq!(first[bits_start], 1);
        assert_eq!(first[bits_start + 1], 1);
    }

    #[test]
    fn checker_rejects_noncanonical_struct_layout_rules() {
        let mut wrong_empty = [0_u8; 24];
        write(
            &mut wrong_empty,
            0,
            WirePointer::new_struct(1, 0, 0).expect("pointer encodes"),
        );
        assert!(!check(&wrong_empty));
        assert_eq!(
            normalize(&wrong_empty).as_ref(),
            WirePointer::empty_struct().to_le_bytes().as_slice()
        );

        let mut gap = [0_u8; 24];
        write(
            &mut gap,
            0,
            WirePointer::new_struct(1, 1, 0).expect("pointer encodes"),
        );
        gap[16] = 7;
        assert!(!check(&gap));

        let mut arena = ExclusiveArena::new(1, 16).expect("arena allocates");
        arena.init_root_struct(2, 2).expect("root initializes");
        let untrimmed = arena.into_segment().expect("one segment");
        assert!(!check(&untrimmed));
        assert_eq!(normalize(&untrimmed).len(), 8);

        let mut trailing = normalize(&wrong_empty).into_vec();
        trailing.extend_from_slice(&[0; 8]);
        assert!(!check(&trailing));
    }

    #[test]
    fn checker_rejects_pointer_order_multiple_segments_and_capabilities() {
        let mut arena = ExclusiveArena::new(1, 32).expect("arena allocates");
        {
            let mut root = arena.init_root_struct(0, 2).expect("root initializes");
            root.init_struct(1, 1, 0).expect("second child initializes");
            root.init_struct(0, 1, 0).expect("first child initializes");
        }
        let reversed = arena.into_segment().expect("one segment");
        assert!(!check(&reversed));
        assert!(check(&normalize(&reversed)));

        let mut far = [0_u8; 24];
        write(
            &mut far,
            0,
            WirePointer::new_far(false, 1, 0).expect("far pointer encodes"),
        );
        write(
            &mut far,
            1,
            WirePointer::new_struct(0, 1, 0).expect("landing tag encodes"),
        );
        far[16] = 5;
        assert!(!check(&far));
        assert!(check(&normalize(&far)));

        let empty = [0_u8; 8];
        let segments = MessageSegments::new(&[&empty, &empty]).expect("segments validate");
        assert!(
            !is_canonical(
                &segments,
                &LocalTraversalBudget::new(100),
                NestingLimit::new(10),
                100,
            )
            .expect("multiple segments are simply noncanonical")
        );

        let mut capability = [0_u8; 8];
        write(&mut capability, 0, WirePointer::new_capability(42));
        assert!(!check(&capability));
        assert!(matches!(
            canonicalize(
                &message(&capability),
                &LocalTraversalBudget::new(100),
                NestingLimit::new(10),
                100,
            ),
            Err(CanonicalError::Graph(
                GraphError::CapabilityNotCanonicalizable { index: 42 }
            ))
        ));
    }

    #[test]
    fn primitive_and_bit_padding_are_zeroed() {
        let mut bytes = [0_u8; 16];
        write(
            &mut bytes,
            0,
            WirePointer::new_list(0, ElementSize::Byte, 1).expect("pointer encodes"),
        );
        bytes[8] = 0x5a;
        bytes[9] = 1;
        assert!(!check(&bytes));
        let canonical = normalize(&bytes);
        assert_eq!(&canonical[8..], &[0x5a, 0, 0, 0, 0, 0, 0, 0]);

        write(
            &mut bytes,
            0,
            WirePointer::new_list(0, ElementSize::Bit, 1).expect("pointer encodes"),
        );
        bytes[8] = 0xff;
        bytes[9..].fill(0);
        assert!(!check(&bytes));
        assert_eq!(normalize(&bytes)[8], 1);
    }

    #[test]
    fn inline_composite_uses_exact_common_element_width() {
        let mut arena = ExclusiveArena::new(1, 64).expect("arena allocates");
        {
            let mut list = arena
                .init_root_struct_list(2, 2, 2)
                .expect("list initializes");
            let mut first = list.get(0).expect("element exists");
            first.set_u64(0, 9, 0).expect("value fits");
            first.init_struct(0, 0, 0).expect("pointer initializes");
        }
        let source = arena.into_segment().expect("one segment");
        assert!(!check(&source));
        let canonical = normalize(&source);
        assert!(check(&canonical));
        let list = message(&canonical)
            .validate_pointer(ROOT)
            .expect("root validates")
            .list()
            .expect("root is a list");
        assert_eq!(list.inline_struct_size, Some((1, 1)));
        assert_eq!(list.content_words, 4);

        let mut inaccurate = canonical.into_vec();
        let root = WirePointer::read_from(&inaccurate, 0).expect("root reads");
        write(
            &mut inaccurate,
            0,
            WirePointer::new_list(
                root.positional_offset().expect("list offset"),
                ElementSize::InlineComposite,
                5,
            )
            .expect("pointer encodes"),
        );
        inaccurate.extend_from_slice(&[0; 8]);
        assert!(!check(&inaccurate));
    }

    #[test]
    fn output_and_traversal_limits_bound_canonicalization() {
        let mut arena = ExclusiveArena::new(1, 16).expect("arena allocates");
        arena
            .init_root_struct(2, 0)
            .expect("root initializes")
            .set_u64(0, 1, 0)
            .expect("value fits");
        let source = arena.into_segment().expect("one segment");
        assert!(
            canonicalize(
                &message(&source),
                &LocalTraversalBudget::new(100),
                NestingLimit::new(10),
                1,
            )
            .is_err()
        );
        assert!(
            canonicalize(
                &message(&source),
                &LocalTraversalBudget::new(0),
                NestingLimit::new(10),
                100,
            )
            .is_err()
        );
    }
}
