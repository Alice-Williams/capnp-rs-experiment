//! Owned messages and stable, typed references to wire objects.
//!
//! This module implements ADR-0001. Owned segment buffers are immutable and
//! shared through `Arc`; retained references store only an owning message,
//! checked wire coordinates, a copied nesting limit, and a sealed type marker.
//! Dereferencing reconstructs a short-lived `MessageSegments` and reader, so a
//! native pointer or a borrow into the backing storage can never be retained.
//! All clones share one exact atomic traversal budget.
//!
//! These types deliberately do not provide mutation, schema-generated types,
//! capabilities, or per-object copies. Those belong to later milestones.

use alloc::{sync::Arc, vec::Vec};
use core::fmt;
use core::marker::PhantomData;

use crate::{
    GraphError, ListReadError, ListReader, ListRef, MessageSegments, NestingLimit, ResolvedPointer,
    SharedTraversalBudget, StructBuilder, StructReadError, StructReader, TraversalBudget,
    ValidationError, WireLocation,
};

/// Limits shared by every view and retained reference into an owned message.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReaderLimits {
    pub traversal_words: u64,
    pub nesting_levels: u32,
}

impl Default for ReaderLimits {
    fn default() -> Self {
        Self {
            traversal_words: 8 * 1024 * 1024,
            nesting_levels: 64,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnedReadError {
    Validation(ValidationError),
    Struct(StructReadError),
    List(ListReadError),
    Blob(crate::BlobError),
    ExpectedStruct,
    ExpectedList,
}

impl fmt::Display for OwnedReadError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl core::error::Error for OwnedReadError {
    fn source(&self) -> Option<&(dyn core::error::Error + 'static)> {
        match self {
            Self::Validation(error) => Some(error),
            Self::Struct(error) => Some(error),
            Self::List(error) => Some(error),
            Self::Blob(error) => Some(error),
            Self::ExpectedStruct | Self::ExpectedList => None,
        }
    }
}

impl From<ValidationError> for OwnedReadError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

impl From<StructReadError> for OwnedReadError {
    fn from(value: StructReadError) -> Self {
        Self::Struct(value)
    }
}

impl From<ListReadError> for OwnedReadError {
    fn from(value: ListReadError) -> Self {
        Self::List(value)
    }
}

impl From<crate::BlobError> for OwnedReadError {
    fn from(value: crate::BlobError) -> Self {
        Self::Blob(value)
    }
}

/// A non-owning message context whose readers cannot outlive caller storage.
///
/// ```compile_fail
/// use capnp_message::{BorrowedMessage, DataSection, ReaderLimits, WireLocation};
/// fn dangling() -> DataSection<'static> {
///     let bytes = [0u8; 8];
///     let message = BorrowedMessage::new(&[&bytes], ReaderLimits::default()).unwrap();
///     message.read_struct(WireLocation { segment_id: 0, word_offset: 0 })
///         .unwrap().data_section().unwrap()
/// }
/// ```
#[derive(Debug)]
pub struct BorrowedMessage<'data> {
    segments: MessageSegments<'data>,
    budget: crate::LocalTraversalBudget,
    nesting: NestingLimit,
}

impl<'data> BorrowedMessage<'data> {
    pub fn new(segments: &[&'data [u8]], limits: ReaderLimits) -> Result<Self, ValidationError> {
        Ok(Self {
            segments: MessageSegments::new(segments)?,
            budget: crate::LocalTraversalBudget::new(limits.traversal_words),
            nesting: NestingLimit::new(limits.nesting_levels),
        })
    }

    pub fn read_struct(
        &self,
        location: WireLocation,
    ) -> Result<StructReader<'_, 'data, crate::LocalTraversalBudget>, StructReadError> {
        self.segments
            .read_struct(location, &self.budget, self.nesting)
    }

    pub fn read_list(
        &self,
        location: WireLocation,
    ) -> Result<ListReader<'_, 'data, crate::LocalTraversalBudget>, ListReadError> {
        self.segments
            .read_list(location, &self.budget, self.nesting)
    }

    pub fn remaining_traversal_words(&self) -> u64 {
        self.budget.remaining_words()
    }
}

/// Immutable owned segment backing and the exact traversal budget shared by it.
#[derive(Debug)]
pub struct OwnedMessage {
    segments: Arc<[Arc<[u8]>]>,
    budget: SharedTraversalBudget,
    nesting: NestingLimit,
}

impl OwnedMessage {
    /// Takes ownership of immutable segment buffers without copying existing
    /// `Arc<[u8]>` inputs. Each segment is checked before the message is exposed.
    pub fn new<I, S>(segments: I, limits: ReaderLimits) -> Result<Arc<Self>, ValidationError>
    where
        I: IntoIterator<Item = S>,
        S: Into<Arc<[u8]>>,
    {
        let segments: Arc<[Arc<[u8]>]> = segments
            .into_iter()
            .map(Into::into)
            .collect::<Vec<_>>()
            .into();
        let borrowed = segments.iter().map(AsRef::as_ref).collect::<Vec<_>>();
        MessageSegments::new(&borrowed)?;
        Ok(Arc::new(Self {
            segments,
            budget: SharedTraversalBudget::new(limits.traversal_words),
            nesting: NestingLimit::new(limits.nesting_levels),
        }))
    }

    pub fn segment_count(&self) -> usize {
        self.segments.len()
    }

    pub fn segment(&self, id: u32) -> Option<&[u8]> {
        usize::try_from(id)
            .ok()
            .and_then(|index| self.segments.get(index).map(AsRef::as_ref))
    }

    pub fn remaining_traversal_words(&self) -> u64 {
        self.budget.remaining_words()
    }

    /// Creates a typed root reference after checking the root pointer's kind.
    pub fn root_struct(self: &Arc<Self>) -> Result<TypedMessage<StructObject>, OwnedReadError> {
        Ok(TypedMessage {
            root: ObjectRef::checked(
                Arc::clone(self),
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                self.nesting,
            )?,
        })
    }

    /// Creates a typed root reference after checking the root pointer's kind.
    pub fn root_list(self: &Arc<Self>) -> Result<TypedMessage<ListObject>, OwnedReadError> {
        Ok(TypedMessage {
            root: ObjectRef::checked(
                Arc::clone(self),
                WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                },
                self.nesting,
            )?,
        })
    }

    /// Retains the schema-independent root pointer after validating its wire
    /// kind. RPC promise transforms use this to follow pointer-section paths
    /// without interpreting application schemas.
    pub fn root_pointer(self: &Arc<Self>) -> Result<OwnedPointerRef, OwnedReadError> {
        retained_pointer(
            self,
            WireLocation {
                segment_id: 0,
                word_offset: 0,
            },
            self.nesting,
        )
    }

    /// Retains any validated pointer at an already-derived wire coordinate.
    pub fn pointer_at(
        self: &Arc<Self>,
        location: WireLocation,
        nesting: NestingLimit,
    ) -> Result<OwnedPointerRef, OwnedReadError> {
        retained_pointer(self, location, nesting)
    }

    #[inline(always)]
    fn borrowed_segments(&self) -> MessageSegments<'_> {
        MessageSegments::from_owned_segments(&self.segments)
    }
}

mod sealed {
    pub trait Sealed {}
}

/// A sealed marker for the wire kind an `ObjectRef` is allowed to open.
pub trait ObjectKind: sealed::Sealed {
    #[doc(hidden)]
    fn validate(pointer: ResolvedPointer) -> Result<(), OwnedReadError>;
}

#[derive(Debug)]
pub struct StructObject;

impl sealed::Sealed for StructObject {}
impl ObjectKind for StructObject {
    fn validate(pointer: ResolvedPointer) -> Result<(), OwnedReadError> {
        match pointer {
            ResolvedPointer::Null | ResolvedPointer::Struct(_) => Ok(()),
            ResolvedPointer::List(_) | ResolvedPointer::Capability(_) => {
                Err(OwnedReadError::ExpectedStruct)
            }
        }
    }
}

#[derive(Debug)]
pub struct ListObject;

impl sealed::Sealed for ListObject {}
impl ObjectKind for ListObject {
    fn validate(pointer: ResolvedPointer) -> Result<(), OwnedReadError> {
        match pointer {
            ResolvedPointer::Null | ResolvedPointer::List(_) => Ok(()),
            ResolvedPointer::Struct(_) | ResolvedPointer::Capability(_) => {
                Err(OwnedReadError::ExpectedList)
            }
        }
    }
}

/// A retained pointer whose concrete wire kind is known without schema input.
#[derive(Clone, Debug)]
pub enum OwnedPointerRef {
    Null,
    Struct(ObjectRef<StructObject>),
    List(ObjectRef<ListObject>),
    Capability(u32),
}

impl OwnedPointerRef {
    /// Copies this retained pointer into an exclusive struct builder.
    ///
    /// A null source is a no-op because dynamic builders only expose freshly
    /// initialized destination slots. Non-null graph traversal is charged to
    /// the source message's shared budget and retains the source nesting cap.
    pub fn copy_to_struct(
        &self,
        target: &mut StructBuilder<'_>,
        pointer_index: u16,
    ) -> Result<(), GraphError> {
        match self {
            Self::Null => Ok(()),
            Self::Capability(index) => target
                .set_capability(pointer_index, *index)
                .map_err(GraphError::from),
            Self::Struct(value) => value.copy_to_struct(target, pointer_index),
            Self::List(value) => value.copy_to_struct(target, pointer_index),
        }
    }
}

/// An owning, stable reference to a previously kind-checked wire location.
///
/// Its private fields prevent unchecked coordinates or arbitrary marker types
/// from being forged outside this crate.
///
/// ```compile_fail
/// use capnp_message::{ObjectRef, StructObject, WireLocation};
/// let forged = ObjectRef::<StructObject> {
///     message: todo!(),
///     location: WireLocation { segment_id: 0, word_offset: 99 },
///     nesting: todo!(),
///     marker: core::marker::PhantomData,
/// };
/// ```
pub struct ObjectRef<T: ObjectKind> {
    message: Arc<OwnedMessage>,
    location: WireLocation,
    nesting: NestingLimit,
    marker: PhantomData<fn() -> T>,
}

impl<T: ObjectKind> Clone for ObjectRef<T> {
    fn clone(&self) -> Self {
        Self {
            message: Arc::clone(&self.message),
            location: self.location,
            nesting: self.nesting,
            marker: PhantomData,
        }
    }
}

impl<T: ObjectKind> fmt::Debug for ObjectRef<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObjectRef")
            .field("location", &self.location)
            .field("nesting", &self.nesting)
            .finish_non_exhaustive()
    }
}

impl<T: ObjectKind> ObjectRef<T> {
    fn checked(
        message: Arc<OwnedMessage>,
        location: WireLocation,
        nesting: NestingLimit,
    ) -> Result<Self, OwnedReadError> {
        let segments = message.borrowed_segments();
        T::validate(segments.validate_pointer(location)?)?;
        drop(segments);
        Ok(Self {
            message,
            location,
            nesting,
            marker: PhantomData,
        })
    }

    pub const fn location(&self) -> WireLocation {
        self.location
    }

    pub fn message(&self) -> &Arc<OwnedMessage> {
        &self.message
    }

    pub fn same_object(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.message, &other.message) && self.location == other.location
    }

    fn copy_to_struct(
        &self,
        target: &mut StructBuilder<'_>,
        pointer_index: u16,
    ) -> Result<(), GraphError> {
        let segments = self.message.borrowed_segments();
        target.copy_pointer(
            pointer_index,
            &segments,
            self.location,
            &self.message.budget,
            self.nesting,
        )
    }
}

impl ObjectRef<StructObject> {
    /// Resolves and charges this struct once for repeated field access.
    ///
    /// The prepared view stores only immutable ownership and validated wire
    /// coordinates. It does not cache native pointers, and every pointer-valued
    /// child read still performs its own validation and traversal charge.
    pub fn prepare_reader(&self) -> Result<PreparedStructRef, OwnedReadError> {
        let (reference, nesting) =
            self.with_reader(|reader| (reader.reference(), reader.nesting_limit()))?;
        Ok(PreparedStructRef {
            message: Arc::clone(&self.message),
            reference,
            nesting,
        })
    }

    /// Opens a reader only for the duration of `use_reader`.
    ///
    /// ```compile_fail
    /// use capnp_message::{OwnedMessage, ReaderLimits};
    /// let message = OwnedMessage::new(vec![vec![0u8; 8]], ReaderLimits::default()).unwrap();
    /// let root = message.root_struct().unwrap();
    /// let escaped = root.root().with_reader(|reader| reader).unwrap();
    /// ```
    pub fn with_reader<R>(
        &self,
        use_reader: impl for<'reader> FnOnce(StructReader<'reader, 'reader, SharedTraversalBudget>) -> R,
    ) -> Result<R, OwnedReadError> {
        let segments = self.message.borrowed_segments();
        let reader = segments.read_struct(self.location, &self.message.budget, self.nesting)?;
        Ok(use_reader(reader))
    }

    /// Retains a struct-valued pointer field without copying its target bytes.
    pub fn child_struct(&self, index: u16) -> Result<Option<Self>, OwnedReadError> {
        let (location, nesting) = self.with_reader(|reader| {
            Ok::<_, StructReadError>((reader.pointer_location(index)?, reader.nesting()))
        })??;
        let Some(location) = location else {
            return Ok(None);
        };
        let segments = self.message.borrowed_segments();
        let pointer = segments.validate_pointer(location)?;
        drop(segments);
        match pointer {
            ResolvedPointer::Null => Ok(None),
            ResolvedPointer::Struct(_) => Ok(Some(Self::checked(
                Arc::clone(&self.message),
                location,
                nesting,
            )?)),
            ResolvedPointer::List(_) | ResolvedPointer::Capability(_) => {
                Err(OwnedReadError::ExpectedStruct)
            }
        }
    }

    /// Retains a list-valued pointer field without copying its target bytes.
    pub fn child_list(&self, index: u16) -> Result<Option<ObjectRef<ListObject>>, OwnedReadError> {
        let (location, nesting) = self.with_reader(|reader| {
            Ok::<_, StructReadError>((reader.pointer_location(index)?, reader.nesting()))
        })??;
        let Some(location) = location else {
            return Ok(None);
        };
        let segments = self.message.borrowed_segments();
        let pointer = segments.validate_pointer(location)?;
        drop(segments);
        match pointer {
            ResolvedPointer::Null => Ok(None),
            ResolvedPointer::List(_) => Ok(Some(ObjectRef::checked(
                Arc::clone(&self.message),
                location,
                nesting,
            )?)),
            ResolvedPointer::Struct(_) | ResolvedPointer::Capability(_) => {
                Err(OwnedReadError::ExpectedList)
            }
        }
    }

    pub fn child_pointer(&self, index: u16) -> Result<OwnedPointerRef, OwnedReadError> {
        let (location, nesting) = self.with_reader(|reader| {
            Ok::<_, StructReadError>((reader.pointer_location(index)?, reader.nesting()))
        })??;
        let Some(location) = location else {
            return Ok(OwnedPointerRef::Null);
        };
        retained_pointer(&self.message, location, nesting)
    }
}

/// An owned struct view whose root or parent pointer was validated and charged once.
#[derive(Clone, Debug)]
pub struct PreparedStructRef {
    message: Arc<OwnedMessage>,
    reference: Option<crate::StructRef>,
    nesting: NestingLimit,
}

impl PreparedStructRef {
    /// Borrows the validated data section directly from immutable message storage.
    #[inline(always)]
    pub fn data_section(&self) -> Result<crate::DataSection<'_>, StructReadError> {
        let Some(reference) = self.reference else {
            return Ok(crate::DataSection::from_validated_bytes(&[]));
        };
        let segment = self.message.segment(reference.content.segment_id).ok_or(
            StructReadError::UnknownSegment {
                segment_id: reference.content.segment_id,
            },
        )?;
        let start = usize::try_from(reference.content.word_offset)
            .map_err(|_| StructReadError::RangeOverflow)?
            .checked_mul(8)
            .ok_or(StructReadError::RangeOverflow)?;
        let bytes = usize::from(reference.data_words)
            .checked_mul(8)
            .ok_or(StructReadError::RangeOverflow)?;
        let end = start
            .checked_add(bytes)
            .ok_or(StructReadError::RangeOverflow)?;
        let data = segment
            .get(start..end)
            .ok_or(StructReadError::DataOutOfBounds {
                location: reference.content,
                data_words: reference.data_words,
                segment_bytes: segment.len(),
            })?;
        Ok(crate::DataSection::from_validated_bytes(data))
    }

    /// Opens a short-lived general reader without resolving or charging the
    /// already prepared struct pointer again.
    pub fn with_reader<R>(
        &self,
        use_reader: impl for<'reader> FnOnce(StructReader<'reader, 'reader, SharedTraversalBudget>) -> R,
    ) -> R {
        let segments = self.message.borrowed_segments();
        use_reader(StructReader::from_prevalidated(
            &segments,
            &self.message.budget,
            self.reference,
            self.nesting,
        ))
    }
}

impl ObjectRef<ListObject> {
    pub(crate) fn precharge_for_partitions(
        &self,
    ) -> Result<(Option<ListRef>, NestingLimit, u64), OwnedReadError> {
        let segments = self.message.borrowed_segments();
        let bounded = segments
            .validate_pointer_with_limits(self.location, &self.message.budget, self.nesting)
            .map_err(ListReadError::from)?;
        match bounded.pointer {
            ResolvedPointer::Null => Ok((None, bounded.child_nesting, bounded.charged_words)),
            ResolvedPointer::List(reference) => Ok((
                Some(reference),
                bounded.child_nesting,
                bounded.charged_words,
            )),
            ResolvedPointer::Struct(_) | ResolvedPointer::Capability(_) => {
                Err(OwnedReadError::ExpectedList)
            }
        }
    }

    pub(crate) fn with_precharged_reader<R>(
        &self,
        reference: Option<ListRef>,
        nesting: NestingLimit,
        use_reader: impl for<'reader> FnOnce(ListReader<'reader, 'reader, SharedTraversalBudget>) -> R,
    ) -> Result<R, OwnedReadError> {
        let segments = self.message.borrowed_segments();
        let reader =
            ListReader::from_precharged(&segments, &self.message.budget, reference, nesting);
        Ok(use_reader(reader))
    }

    /// Opens a reader only for the duration of `use_reader`.
    pub fn with_reader<R>(
        &self,
        use_reader: impl for<'reader> FnOnce(ListReader<'reader, 'reader, SharedTraversalBudget>) -> R,
    ) -> Result<R, OwnedReadError> {
        let segments = self.message.borrowed_segments();
        let reader = segments.read_list(self.location, &self.message.budget, self.nesting)?;
        Ok(use_reader(reader))
    }

    pub fn with_text<R>(
        &self,
        use_reader: impl for<'reader> FnOnce(crate::TextReader<'reader>) -> R,
    ) -> Result<R, OwnedReadError> {
        let segments = self.message.borrowed_segments();
        let reader = segments.read_text(self.location, &self.message.budget, self.nesting)?;
        Ok(use_reader(reader))
    }

    pub fn with_data<R>(
        &self,
        use_reader: impl for<'reader> FnOnce(crate::DataReader<'reader>) -> R,
    ) -> Result<R, OwnedReadError> {
        let segments = self.message.borrowed_segments();
        let reader = segments.read_data(self.location, &self.message.budget, self.nesting)?;
        Ok(use_reader(reader))
    }

    /// Retains an inline-composite (or legally upgraded) struct-list element.
    pub fn struct_element(&self, index: u32) -> Result<StructElementRef, OwnedReadError> {
        self.with_reader(|reader| reader.as_structs()?.get(index).map(|_| ()))??;
        Ok(StructElementRef {
            list: self.clone(),
            index,
        })
    }

    /// Retains a struct-valued pointer-list element.
    pub fn pointer_struct(
        &self,
        index: u32,
    ) -> Result<Option<ObjectRef<StructObject>>, OwnedReadError> {
        let (location, nesting) =
            self.with_reader(|reader| reader.as_pointers()?.element_location(index))??;
        let segments = self.message.borrowed_segments();
        let pointer = segments.validate_pointer(location)?;
        drop(segments);
        match pointer {
            ResolvedPointer::Null => Ok(None),
            ResolvedPointer::Struct(_) => Ok(Some(ObjectRef::checked(
                Arc::clone(&self.message),
                location,
                nesting,
            )?)),
            ResolvedPointer::List(_) | ResolvedPointer::Capability(_) => {
                Err(OwnedReadError::ExpectedStruct)
            }
        }
    }

    /// Retains a list-valued pointer-list element.
    pub fn pointer_list(
        &self,
        index: u32,
    ) -> Result<Option<ObjectRef<ListObject>>, OwnedReadError> {
        let (location, nesting) =
            self.with_reader(|reader| reader.as_pointers()?.element_location(index))??;
        let segments = self.message.borrowed_segments();
        let pointer = segments.validate_pointer(location)?;
        drop(segments);
        match pointer {
            ResolvedPointer::Null => Ok(None),
            ResolvedPointer::List(_) => Ok(Some(ObjectRef::checked(
                Arc::clone(&self.message),
                location,
                nesting,
            )?)),
            ResolvedPointer::Struct(_) | ResolvedPointer::Capability(_) => {
                Err(OwnedReadError::ExpectedList)
            }
        }
    }

    pub fn pointer_element(&self, index: u32) -> Result<OwnedPointerRef, OwnedReadError> {
        let (location, nesting) =
            self.with_reader(|reader| reader.as_pointers()?.element_location(index))??;
        retained_pointer(&self.message, location, nesting)
    }
}

/// An owning coordinate for one struct-list element.
#[derive(Clone, Debug)]
pub struct StructElementRef {
    list: ObjectRef<ListObject>,
    index: u32,
}

impl StructElementRef {
    pub const fn index(&self) -> u32 {
        self.index
    }

    pub fn list(&self) -> &ObjectRef<ListObject> {
        &self.list
    }

    pub fn with_reader<R>(
        &self,
        use_reader: impl for<'reader> FnOnce(
            crate::StructElementReader<'reader, 'reader, SharedTraversalBudget>,
        ) -> R,
    ) -> Result<R, OwnedReadError> {
        Ok(self.list.with_reader(|reader| {
            let structs = reader.as_structs()?;
            Ok::<_, ListReadError>(use_reader(structs.get(self.index)?))
        })??)
    }

    pub fn child_struct(
        &self,
        pointer_index: u16,
    ) -> Result<Option<ObjectRef<StructObject>>, OwnedReadError> {
        let (location, nesting) = self.with_reader(|reader| {
            Ok::<_, ListReadError>((reader.pointer_location(pointer_index)?, reader.nesting()))
        })??;
        let Some(location) = location else {
            return Ok(None);
        };
        let segments = self.list.message.borrowed_segments();
        let pointer = segments.validate_pointer(location)?;
        drop(segments);
        match pointer {
            ResolvedPointer::Null => Ok(None),
            ResolvedPointer::Struct(_) => Ok(Some(ObjectRef::checked(
                Arc::clone(&self.list.message),
                location,
                nesting,
            )?)),
            ResolvedPointer::List(_) | ResolvedPointer::Capability(_) => {
                Err(OwnedReadError::ExpectedStruct)
            }
        }
    }

    pub fn child_list(
        &self,
        pointer_index: u16,
    ) -> Result<Option<ObjectRef<ListObject>>, OwnedReadError> {
        let (location, nesting) = self.with_reader(|reader| {
            Ok::<_, ListReadError>((reader.pointer_location(pointer_index)?, reader.nesting()))
        })??;
        let Some(location) = location else {
            return Ok(None);
        };
        let segments = self.list.message.borrowed_segments();
        let pointer = segments.validate_pointer(location)?;
        drop(segments);
        match pointer {
            ResolvedPointer::Null => Ok(None),
            ResolvedPointer::List(_) => Ok(Some(ObjectRef::checked(
                Arc::clone(&self.list.message),
                location,
                nesting,
            )?)),
            ResolvedPointer::Struct(_) | ResolvedPointer::Capability(_) => {
                Err(OwnedReadError::ExpectedList)
            }
        }
    }

    pub fn child_pointer(&self, pointer_index: u16) -> Result<OwnedPointerRef, OwnedReadError> {
        let (location, nesting) = self.with_reader(|reader| {
            Ok::<_, ListReadError>((reader.pointer_location(pointer_index)?, reader.nesting()))
        })??;
        let Some(location) = location else {
            return Ok(OwnedPointerRef::Null);
        };
        retained_pointer(&self.list.message, location, nesting)
    }
}

fn retained_pointer(
    message: &Arc<OwnedMessage>,
    location: WireLocation,
    nesting: NestingLimit,
) -> Result<OwnedPointerRef, OwnedReadError> {
    let segments = message.borrowed_segments();
    let pointer = segments.validate_pointer(location)?;
    drop(segments);
    Ok(match pointer {
        ResolvedPointer::Null => OwnedPointerRef::Null,
        ResolvedPointer::Struct(_) => {
            OwnedPointerRef::Struct(ObjectRef::checked(Arc::clone(message), location, nesting)?)
        }
        ResolvedPointer::List(_) => {
            OwnedPointerRef::List(ObjectRef::checked(Arc::clone(message), location, nesting)?)
        }
        ResolvedPointer::Capability(value) => OwnedPointerRef::Capability(value.index),
    })
}

/// A message whose root wire kind is fixed in its Rust type.
pub struct TypedMessage<T: ObjectKind> {
    root: ObjectRef<T>,
}

impl<T: ObjectKind> Clone for TypedMessage<T> {
    fn clone(&self) -> Self {
        Self {
            root: self.root.clone(),
        }
    }
}

impl<T: ObjectKind> fmt::Debug for TypedMessage<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TypedMessage")
            .field("root", &self.root)
            .finish()
    }
}

impl<T: ObjectKind> TypedMessage<T> {
    pub const fn root(&self) -> &ObjectRef<T> {
        &self.root
    }

    pub fn into_root(self) -> ObjectRef<T> {
        self.root
    }

    pub fn message(&self) -> &Arc<OwnedMessage> {
        self.root.message()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use capnp_wire::WirePointer;

    fn nested_struct_message(limits: ReaderLimits) -> Arc<OwnedMessage> {
        let mut bytes = vec![0u8; 32];
        WirePointer::new_struct(0, 1, 1)
            .expect("root pointer fits")
            .write_to(&mut bytes, 0)
            .expect("root pointer writes");
        bytes[8..16].copy_from_slice(&42u64.to_le_bytes());
        WirePointer::new_struct(0, 1, 0)
            .expect("child pointer fits")
            .write_to(&mut bytes, 16)
            .expect("child pointer writes");
        bytes[24..32].copy_from_slice(&99u64.to_le_bytes());
        OwnedMessage::new(vec![bytes], limits).expect("owned message is valid")
    }

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn public_owned_shapes_are_send_sync_by_representation() {
        assert_send_sync::<OwnedMessage>();
        assert_send_sync::<ObjectRef<StructObject>>();
        assert_send_sync::<ObjectRef<ListObject>>();
        assert_send_sync::<PreparedStructRef>();
        assert_send_sync::<TypedMessage<StructObject>>();
    }

    #[test]
    fn prepared_struct_charges_once_and_reuses_validated_coordinates() {
        let message = nested_struct_message(ReaderLimits {
            traversal_words: 2,
            nesting_levels: 4,
        });
        let root = message.root_struct().expect("root validates").into_root();
        let prepared = root.prepare_reader().expect("root precharges");
        assert_eq!(message.remaining_traversal_words(), 0);
        assert_eq!(
            prepared
                .data_section()
                .expect("prepared data section")
                .read_u64(0, 0)
                .expect("first prepared read"),
            42
        );
        assert_eq!(
            prepared
                .data_section()
                .expect("prepared data section repeats")
                .read_u64(0, 0)
                .expect("second prepared read"),
            42
        );
        assert_eq!(message.remaining_traversal_words(), 0);
    }

    #[test]
    fn a_retained_child_owns_the_original_backing_without_copying() {
        let message = nested_struct_message(ReaderLimits {
            traversal_words: 8,
            nesting_levels: 4,
        });
        let backing = message.segment(0).expect("segment exists").as_ptr();
        let typed = message.root_struct().expect("root is a struct");
        assert_eq!(
            typed
                .root()
                .with_reader(|reader| {
                    reader
                        .data_section()?
                        .read_u64(0, 0)
                        .map_err(StructReadError::from)
                })
                .expect("reader opens")
                .expect("root value reads"),
            42
        );
        let child = typed
            .root()
            .child_struct(0)
            .expect("child validates")
            .expect("pointer field exists");
        drop(typed);
        drop(message);

        assert_eq!(
            child
                .message()
                .segment(0)
                .expect("segment remains")
                .as_ptr(),
            backing
        );
        assert_eq!(
            child
                .with_reader(|reader| {
                    reader
                        .data_section()?
                        .read_u64(0, 0)
                        .map_err(StructReadError::from)
                })
                .expect("retained reader opens")
                .expect("retained value reads"),
            99
        );
    }

    #[test]
    fn wrong_root_and_child_kinds_are_rejected_before_exposure() {
        let message = nested_struct_message(ReaderLimits::default());
        assert!(matches!(
            message.root_list(),
            Err(OwnedReadError::ExpectedList)
        ));
        let root = message.root_struct().expect("root is a struct");
        assert!(matches!(
            root.root().child_list(0),
            Err(OwnedReadError::ExpectedList)
        ));
    }

    #[test]
    fn concurrent_clones_share_one_exact_budget() {
        let message = nested_struct_message(ReaderLimits {
            traversal_words: 2,
            nesting_levels: 4,
        });
        let root = message.root_struct().expect("root validates").into_root();
        let first = {
            let root = root.clone();
            std::thread::spawn(move || root.with_reader(|_| ()).is_ok())
        };
        let second = std::thread::spawn(move || root.with_reader(|_| ()).is_ok());
        let successes = usize::from(first.join().expect("first joins"))
            + usize::from(second.join().expect("second joins"));
        assert_eq!(successes, 1);
        assert_eq!(message.remaining_traversal_words(), 0);
    }

    #[test]
    fn borrowed_messages_use_local_exact_budgeting() {
        let bytes = [0u8; 8];
        let message = BorrowedMessage::new(
            &[&bytes],
            ReaderLimits {
                traversal_words: 0,
                nesting_levels: 0,
            },
        )
        .expect("borrowed message validates");
        assert!(
            message
                .read_struct(WireLocation {
                    segment_id: 0,
                    word_offset: 0,
                })
                .is_ok()
        );
        assert_eq!(message.remaining_traversal_words(), 0);
    }

    #[test]
    fn owned_list_roots_keep_arc_segments_and_open_typed_views() {
        let mut bytes = vec![0u8; 16];
        WirePointer::new_list(0, capnp_wire::ElementSize::FourBytes, 2)
            .expect("list pointer fits")
            .write_to(&mut bytes, 0)
            .expect("list pointer writes");
        bytes[8..12].copy_from_slice(&11u32.to_le_bytes());
        bytes[12..16].copy_from_slice(&22u32.to_le_bytes());
        let segment: Arc<[u8]> = bytes.into();
        let original_backing = segment.as_ptr();
        let message = OwnedMessage::new(
            [Arc::clone(&segment)],
            ReaderLimits {
                traversal_words: 1,
                nesting_levels: 1,
            },
        )
        .expect("owned list message validates");
        assert_eq!(
            message.segment(0).expect("segment exists").as_ptr(),
            original_backing
        );
        let typed = message.root_list().expect("root is a list");
        assert_eq!(
            typed
                .root()
                .with_reader(|reader| {
                    let values = reader.as_primitive::<u32>()?;
                    Ok::<_, ListReadError>((values.get(0)?, values.get(1)?))
                })
                .expect("list reader opens")
                .expect("list values read"),
            (11, 22)
        );
    }
}
