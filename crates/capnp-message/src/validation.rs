use core::fmt;

use capnp_wire::{ElementSize, PointerKind, WirePointer};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WireLocation {
    pub segment_id: u32,
    pub word_offset: u32,
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
    FarPointerRequiresLandingPadValidation,
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

impl std::error::Error for ValidationError {}

/// Immutable borrowed segments addressed only by stable coordinates.
#[derive(Debug)]
pub struct MessageSegments<'a> {
    segments: Box<[&'a [u8]]>,
}

impl<'a> MessageSegments<'a> {
    pub fn new(segments: &[&'a [u8]]) -> Result<Self, ValidationError> {
        if segments.is_empty() {
            return Err(ValidationError::NoSegments);
        }
        if segments.len() > u32::MAX as usize {
            return Err(ValidationError::TooManySegments);
        }
        for (index, segment) in segments.iter().enumerate() {
            if segment.len() % 8 != 0 {
                return Err(ValidationError::SegmentNotWordAligned {
                    segment_id: u32::try_from(index)
                        .map_err(|_| ValidationError::TooManySegments)?,
                    bytes: segment.len(),
                });
            }
        }
        Ok(Self {
            segments: segments.into(),
        })
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn segment(&self, id: u32) -> Option<&'a [u8]> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.segments.get(index).copied())
    }

    /// Validates a non-far pointer and returns coordinates, never cached native pointers.
    /// Far landing pads are added in the next M05 slice.
    pub fn validate_pointer(
        &self,
        location: WireLocation,
    ) -> Result<ResolvedPointer, ValidationError> {
        let pointer = self.read_pointer(location)?;
        if pointer.is_null() {
            return Ok(ResolvedPointer::Null);
        }
        match pointer.kind() {
            PointerKind::Struct => self.validate_struct(location, pointer),
            PointerKind::List => self.validate_list(location, pointer),
            PointerKind::Far => Err(ValidationError::FarPointerRequiresLandingPadValidation),
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

    fn validate_struct(
        &self,
        pointer_location: WireLocation,
        pointer: WirePointer,
    ) -> Result<ResolvedPointer, ValidationError> {
        let fields = pointer
            .struct_fields()
            .expect("struct discriminator was checked");
        let content = positional_target(pointer_location, fields.offset)?;
        let words = u64::from(fields.data_words) + u64::from(fields.pointer_count);
        self.check_range(content, words)?;
        Ok(ResolvedPointer::Struct(StructRef {
            content,
            data_words: fields.data_words,
            pointer_count: fields.pointer_count,
        }))
    }

    fn validate_list(
        &self,
        pointer_location: WireLocation,
        pointer: WirePointer,
    ) -> Result<ResolvedPointer, ValidationError> {
        let fields = pointer
            .list_fields()
            .expect("list discriminator was checked");
        let target = positional_target(pointer_location, fields.offset)?;
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

    fn read_pointer(&self, location: WireLocation) -> Result<WirePointer, ValidationError> {
        self.check_range(location, 1)?;
        let segment = self
            .segment(location.segment_id)
            .ok_or(ValidationError::UnknownSegment {
                segment_id: location.segment_id,
            })?;
        let byte_offset = usize::try_from(u64::from(location.word_offset) * 8)
            .map_err(|_| ValidationError::TargetWordOverflow)?;
        WirePointer::read_from(segment, byte_offset)
            .map_err(|_| ValidationError::PointerOutOfBounds { location })
    }

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

    fn with_pointer(pointer: WirePointer, trailing_words: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; (trailing_words + 1) * 8];
        pointer.write_to(&mut bytes, 0).expect("pointer word fits");
        bytes
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
}
