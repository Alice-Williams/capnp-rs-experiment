//! Settled capability identity and exact Level-1 reference accounting.
//!
//! Compatibility is defined by the pinned `ExportId`, `Payload`,
//! `CapDescriptor`, and `Release` declarations in `rpc.capnp` at commit
//! `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`. Every transmitted
//! `senderHosted` descriptor adds exactly one export/import reference, even
//! when an ID is duplicated in one payload. `receiverHosted` returns a
//! capability to its owner and does not add an export reference. A zero-count
//! or excessive `Release` is a protocol error. Disconnect clears all four
//! tables without emitting traffic.
//!
//! M35 adds `receiverAnswer` as a non-refcounted routing descriptor. M36 adds
//! `senderPromise`, exact promise references, and released-ID tombstones that
//! prevent reuse until a late resolver is observed. Route resolution and
//! embargo ordering remain owned exclusively by the connection actor;
//! M43 layers attached-resource ownership onto ordinary descriptors while
//! third-party handoff remains a later milestone.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{AttachedResource, CapDescriptor, PromisedAnswer, ReleaseMessage};

static NEXT_HOSTED_IDENTITY: AtomicU64 = AtomicU64::new(1);
static NEXT_PROMISE_IDENTITY: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct HostedCapability {
    identity: Arc<HostedIdentity>,
}

#[derive(Debug)]
struct HostedIdentity {
    id: u64,
}

impl HostedCapability {
    pub fn new() -> Result<Self, CapabilityError> {
        let id = NEXT_HOSTED_IDENTITY
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| CapabilityError::IdentityExhausted)?;
        Ok(Self {
            identity: Arc::new(HostedIdentity { id }),
        })
    }

    pub fn identity(&self) -> u64 {
        self.identity.id
    }
}

impl fmt::Debug for HostedCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("HostedCapability")
            .field("identity", &self.identity())
            .finish_non_exhaustive()
    }
}

impl PartialEq for HostedCapability {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }
}

impl Eq for HostedCapability {}

#[derive(Clone)]
pub struct PromiseCapability {
    identity: Arc<PromiseIdentity>,
}

#[derive(Debug)]
struct PromiseIdentity {
    id: u64,
}

impl PromiseCapability {
    pub fn new() -> Result<Self, CapabilityError> {
        let id = NEXT_PROMISE_IDENTITY
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                value.checked_add(1)
            })
            .map_err(|_| CapabilityError::IdentityExhausted)?;
        Ok(Self {
            identity: Arc::new(PromiseIdentity { id }),
        })
    }

    pub fn identity(&self) -> u64 {
        self.identity.id
    }
}

impl fmt::Debug for PromiseCapability {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PromiseCapability")
            .field("identity", &self.identity())
            .finish_non_exhaustive()
    }
}

impl PartialEq for PromiseCapability {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.identity, &other.identity)
    }
}

impl Eq for PromiseCapability {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutgoingCapability {
    None,
    Hosted(HostedCapability),
    Promise(PromiseCapability),
    ReceiverAnswer(PromisedAnswer),
    /// A capability imported from this peer and now sent back to it.
    ReceiverHosted(u32),
    Attached {
        capability: Box<OutgoingCapability>,
        resource: AttachedResource,
    },
}

impl OutgoingCapability {
    pub fn with_attachment(self, resource: AttachedResource) -> Self {
        let capability = match self {
            Self::Attached { capability, .. } => capability,
            capability => Box::new(capability),
        };
        Self::Attached {
            capability,
            resource,
        }
    }

    pub fn capability(&self) -> &OutgoingCapability {
        match self {
            Self::Attached { capability, .. } => capability,
            capability => capability,
        }
    }

    pub fn attached_resource(&self) -> Option<&AttachedResource> {
        match self {
            Self::Attached { resource, .. } => Some(resource),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceivedCapability {
    None,
    /// A settled capability hosted by the peer, addressed by its export ID.
    Imported(u32),
    PromiseImported(u32),
    /// A capability previously exported by this connection endpoint.
    Hosted(HostedCapability),
    ExportedPromise(PromiseCapability),
    ReceiverAnswer(PromisedAnswer),
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CapabilityStats {
    pub active_imports: usize,
    pub active_exports: usize,
    pub import_references: u64,
    pub export_references: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CapabilityError {
    IdentityExhausted,
    ImportLimit { limit: usize },
    ExportLimit { limit: usize },
    ExportIdExhausted,
    ReferenceCountOverflow,
    AttachedResourceLimit { limit: usize },
    UnknownImport(u32),
    UnknownExport(u32),
    ExcessRelease { id: u32, requested: u32, held: u64 },
    UnsupportedDescriptor(&'static str),
}

impl fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for CapabilityError {}

#[derive(Clone)]
struct ExportEntry {
    capability: ExportCapability,
    references: u64,
}

#[derive(Clone)]
enum ExportCapability {
    Hosted(HostedCapability),
    Promise(PromiseCapability),
}

impl ExportCapability {
    fn key(&self) -> ExportIdentity {
        match self {
            Self::Hosted(capability) => ExportIdentity::Hosted(capability.identity()),
            Self::Promise(capability) => ExportIdentity::Promise(capability.identity()),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum ExportIdentity {
    Hosted(u64),
    Promise(u64),
}

#[derive(Clone)]
struct ImportEntry {
    references: u64,
    promise: bool,
    attachment: Option<AttachedResource>,
}

/// Connection-local import/export tables. The connection actor is the sole
/// mutable owner; handles exchange commands rather than locking this state.
#[derive(Clone)]
pub struct CapabilityTables {
    imports: BTreeMap<u32, ImportEntry>,
    exports: BTreeMap<u32, ExportEntry>,
    export_by_identity: BTreeMap<ExportIdentity, u32>,
    reserved_export_ids: BTreeSet<u32>,
    max_imports: usize,
    max_exports: usize,
}

impl CapabilityTables {
    pub fn new(max_imports: usize, max_exports: usize) -> Self {
        Self {
            imports: BTreeMap::new(),
            exports: BTreeMap::new(),
            export_by_identity: BTreeMap::new(),
            reserved_export_ids: BTreeSet::new(),
            max_imports,
            max_exports,
        }
    }

    pub fn stats(&self) -> CapabilityStats {
        CapabilityStats {
            active_imports: self.imports.len(),
            active_exports: self.exports.len(),
            import_references: reference_total(self.imports.values().map(|value| value.references)),
            export_references: reference_total(self.exports.values().map(|value| value.references)),
        }
    }

    pub fn describe(
        &mut self,
        capability: &OutgoingCapability,
    ) -> Result<CapDescriptor, CapabilityError> {
        match capability {
            OutgoingCapability::None => Ok(CapDescriptor::None),
            OutgoingCapability::Hosted(capability) => {
                self.describe_export(ExportCapability::Hosted(capability.clone()))
            }
            OutgoingCapability::Promise(capability) => {
                self.describe_export(ExportCapability::Promise(capability.clone()))
            }
            OutgoingCapability::ReceiverHosted(id) => {
                if !self.imports.contains_key(id) {
                    return Err(CapabilityError::UnknownImport(*id));
                }
                Ok(CapDescriptor::ReceiverHosted(*id))
            }
            OutgoingCapability::ReceiverAnswer(promised_answer) => {
                Ok(CapDescriptor::ReceiverAnswer(promised_answer.clone()))
            }
            OutgoingCapability::Attached { .. } => Err(CapabilityError::UnsupportedDescriptor(
                "attached capabilities require describe_all_with_resources",
            )),
        }
    }

    fn describe_export(
        &mut self,
        capability: ExportCapability,
    ) -> Result<CapDescriptor, CapabilityError> {
        let key = capability.key();
        if self.stats().export_references == u64::MAX {
            return Err(CapabilityError::ReferenceCountOverflow);
        }
        if let Some(id) = self.export_by_identity.get(&key).copied() {
            let entry = self
                .exports
                .get_mut(&id)
                .ok_or(CapabilityError::UnknownExport(id))?;
            entry.references = entry
                .references
                .checked_add(1)
                .ok_or(CapabilityError::ReferenceCountOverflow)?;
            return Ok(match capability {
                ExportCapability::Hosted(_) => CapDescriptor::SenderHosted(id),
                ExportCapability::Promise(_) => CapDescriptor::SenderPromise(id),
            });
        }
        if self.exports.len() >= self.max_exports {
            return Err(CapabilityError::ExportLimit {
                limit: self.max_exports,
            });
        }
        let id = lowest_free_export_id(&self.exports, &self.reserved_export_ids)?;
        self.exports.insert(
            id,
            ExportEntry {
                capability: capability.clone(),
                references: 1,
            },
        );
        self.export_by_identity.insert(key, id);
        Ok(match capability {
            ExportCapability::Hosted(_) => CapDescriptor::SenderHosted(id),
            ExportCapability::Promise(_) => CapDescriptor::SenderPromise(id),
        })
    }

    /// Describes a whole payload transactionally. Quota or overflow errors do
    /// not leave a prefix of the payload accounted as exported.
    pub fn describe_all(
        &mut self,
        capabilities: &[OutgoingCapability],
    ) -> Result<Vec<CapDescriptor>, CapabilityError> {
        let snapshot = self.clone();
        let result: Result<Vec<_>, CapabilityError> = capabilities
            .iter()
            .map(|capability| self.describe(capability))
            .collect();
        if result.is_err() {
            *self = snapshot;
        }
        result
    }

    /// Describes a complete payload and assigns one distinct `attachedFd`
    /// index to every attached capability. The returned transport resources
    /// retain their underlying owners until the envelope write completes.
    pub fn describe_all_with_resources(
        &mut self,
        capabilities: &[OutgoingCapability],
    ) -> Result<(Vec<CapDescriptor>, Vec<crate::OwnedResource>), CapabilityError> {
        const MAX_ATTACHED_RESOURCES: usize = u8::MAX as usize;
        let snapshot = self.clone();
        let mut descriptors = Vec::with_capacity(capabilities.len());
        let mut resources = Vec::new();
        for capability in capabilities {
            let mut descriptor = match self.describe(capability.capability()) {
                Ok(descriptor) => descriptor,
                Err(error) => {
                    *self = snapshot;
                    return Err(error);
                }
            };
            if let Some(resource) = capability.attached_resource() {
                if resources.len() >= MAX_ATTACHED_RESOURCES {
                    *self = snapshot;
                    return Err(CapabilityError::AttachedResourceLimit {
                        limit: MAX_ATTACHED_RESOURCES,
                    });
                }
                let index = u8::try_from(resources.len()).map_err(|_| {
                    CapabilityError::AttachedResourceLimit {
                        limit: MAX_ATTACHED_RESOURCES,
                    }
                })?;
                descriptor = descriptor.with_attachment(index, Some(resource.clone()));
                resources.push(resource.clone().into_transport_resource());
            }
            descriptors.push(descriptor);
        }
        Ok((descriptors, resources))
    }

    pub fn receive(
        &mut self,
        descriptor: &CapDescriptor,
    ) -> Result<ReceivedCapability, CapabilityError> {
        match descriptor {
            CapDescriptor::None => Ok(ReceivedCapability::None),
            CapDescriptor::SenderHosted(id) => {
                if self.stats().import_references == u64::MAX {
                    return Err(CapabilityError::ReferenceCountOverflow);
                }
                if let Some(entry) = self.imports.get_mut(id) {
                    if entry.promise {
                        return Err(CapabilityError::UnsupportedDescriptor(
                            "senderHosted reuses a promise import ID",
                        ));
                    }
                    entry.references = entry
                        .references
                        .checked_add(1)
                        .ok_or(CapabilityError::ReferenceCountOverflow)?;
                } else {
                    if self.imports.len() >= self.max_imports {
                        return Err(CapabilityError::ImportLimit {
                            limit: self.max_imports,
                        });
                    }
                    self.imports.insert(
                        *id,
                        ImportEntry {
                            references: 1,
                            promise: false,
                            attachment: None,
                        },
                    );
                }
                Ok(ReceivedCapability::Imported(*id))
            }
            CapDescriptor::SenderPromise(id) => {
                if self.stats().import_references == u64::MAX {
                    return Err(CapabilityError::ReferenceCountOverflow);
                }
                if let Some(entry) = self.imports.get_mut(id) {
                    if !entry.promise {
                        return Err(CapabilityError::UnsupportedDescriptor(
                            "senderPromise reuses a settled import ID",
                        ));
                    }
                    entry.references = entry
                        .references
                        .checked_add(1)
                        .ok_or(CapabilityError::ReferenceCountOverflow)?;
                } else {
                    if self.imports.len() >= self.max_imports {
                        return Err(CapabilityError::ImportLimit {
                            limit: self.max_imports,
                        });
                    }
                    self.imports.insert(
                        *id,
                        ImportEntry {
                            references: 1,
                            promise: true,
                            attachment: None,
                        },
                    );
                }
                Ok(ReceivedCapability::PromiseImported(*id))
            }
            CapDescriptor::ReceiverHosted(id) => self
                .exports
                .get(id)
                .map(|entry| match &entry.capability {
                    ExportCapability::Hosted(capability) => {
                        ReceivedCapability::Hosted(capability.clone())
                    }
                    ExportCapability::Promise(capability) => {
                        ReceivedCapability::ExportedPromise(capability.clone())
                    }
                })
                .ok_or(CapabilityError::UnknownExport(*id)),
            CapDescriptor::ReceiverAnswer(promised_answer) => {
                Ok(ReceivedCapability::ReceiverAnswer(promised_answer.clone()))
            }
            CapDescriptor::ThirdPartyHosted(third_party) => {
                // A Level-3 pickup may run concurrently, but the vine is an
                // ordinary sender export and remains the fail-safe route.
                self.receive(&CapDescriptor::SenderHosted(third_party.vine_id))
            }
            CapDescriptor::Attached {
                descriptor,
                resource,
                ..
            } => {
                let received = self.receive(descriptor)?;
                if let (
                    Some(resource),
                    ReceivedCapability::Imported(id) | ReceivedCapability::PromiseImported(id),
                ) = (resource, &received)
                {
                    let entry = self
                        .imports
                        .get_mut(id)
                        .ok_or(CapabilityError::UnknownImport(*id))?;
                    if entry.attachment.is_none() {
                        entry.attachment = Some(resource.clone());
                    }
                }
                Ok(received)
            }
        }
    }

    pub fn import_attachment(&self, id: u32) -> Option<AttachedResource> {
        self.imports
            .get(&id)
            .and_then(|entry| entry.attachment.clone())
    }

    pub fn release_import(
        &mut self,
        id: u32,
        count: u32,
    ) -> Result<ReleaseMessage, CapabilityError> {
        let entry = self
            .imports
            .get_mut(&id)
            .ok_or(CapabilityError::UnknownImport(id))?;
        let count_u64 = u64::from(count);
        if count == 0 || count_u64 > entry.references {
            return Err(CapabilityError::ExcessRelease {
                id,
                requested: count,
                held: entry.references,
            });
        }
        entry.references -= count_u64;
        if entry.references == 0 {
            self.imports.remove(&id);
        }
        Ok(ReleaseMessage {
            id,
            reference_count: count,
        })
    }

    pub fn apply_implicit_import_releases(&mut self, ids: &[u32]) -> Result<(), CapabilityError> {
        let mut counts = BTreeMap::<u32, u32>::new();
        for id in ids {
            let count = counts.entry(*id).or_default();
            *count = count
                .checked_add(1)
                .ok_or(CapabilityError::ReferenceCountOverflow)?;
        }
        for (id, count) in &counts {
            let held = self
                .imports
                .get(id)
                .map(|entry| entry.references)
                .ok_or(CapabilityError::UnknownImport(*id))?;
            if u64::from(*count) > held {
                return Err(CapabilityError::ExcessRelease {
                    id: *id,
                    requested: *count,
                    held,
                });
            }
        }
        for (id, count) in counts {
            let entry = self
                .imports
                .get_mut(&id)
                .ok_or(CapabilityError::UnknownImport(id))?;
            entry.references -= u64::from(count);
            if entry.references == 0 {
                self.imports.remove(&id);
            }
        }
        Ok(())
    }

    pub fn apply_release(&mut self, release: ReleaseMessage) -> Result<(), CapabilityError> {
        let entry = self
            .exports
            .get_mut(&release.id)
            .ok_or(CapabilityError::UnknownExport(release.id))?;
        let count = u64::from(release.reference_count);
        if release.reference_count == 0 || count > entry.references {
            return Err(CapabilityError::ExcessRelease {
                id: release.id,
                requested: release.reference_count,
                held: entry.references,
            });
        }
        entry.references -= count;
        if entry.references == 0 {
            let entry = self
                .exports
                .remove(&release.id)
                .ok_or(CapabilityError::UnknownExport(release.id))?;
            self.export_by_identity.remove(&entry.capability.key());
            if matches!(entry.capability, ExportCapability::Promise(_)) {
                self.reserved_export_ids.insert(release.id);
            }
        }
        Ok(())
    }

    pub fn apply_implicit_releases(&mut self, ids: &[u32]) -> Result<(), CapabilityError> {
        let mut counts = BTreeMap::<u32, u32>::new();
        for id in ids {
            let count = counts.entry(*id).or_default();
            *count = count
                .checked_add(1)
                .ok_or(CapabilityError::ReferenceCountOverflow)?;
        }
        for (id, count) in &counts {
            let held = self
                .exports
                .get(id)
                .map(|entry| entry.references)
                .ok_or(CapabilityError::UnknownExport(*id))?;
            if u64::from(*count) > held {
                return Err(CapabilityError::ExcessRelease {
                    id: *id,
                    requested: *count,
                    held,
                });
            }
        }
        for (id, count) in counts {
            self.apply_release(ReleaseMessage {
                id,
                reference_count: count,
            })?;
        }
        Ok(())
    }

    pub fn contains_import(&self, id: u32) -> bool {
        self.imports.contains_key(&id)
    }

    pub fn is_promise_import(&self, id: u32) -> bool {
        self.imports.get(&id).is_some_and(|entry| entry.promise)
    }

    pub fn promise_export_id(&self, identity: u64) -> Option<u32> {
        self.export_by_identity
            .get(&ExportIdentity::Promise(identity))
            .copied()
    }

    pub fn contains_export(&self, id: u32) -> bool {
        self.exports.contains_key(&id)
    }

    pub fn release_promise_reservation(&mut self, id: u32) {
        self.reserved_export_ids.remove(&id);
    }

    pub fn clear(&mut self) {
        self.imports.clear();
        self.exports.clear();
        self.export_by_identity.clear();
        self.reserved_export_ids.clear();
    }
}

fn lowest_free_export_id<T>(
    values: &BTreeMap<u32, T>,
    reserved: &BTreeSet<u32>,
) -> Result<u32, CapabilityError> {
    let mut candidate = 0_u32;
    loop {
        if !values.contains_key(&candidate) && !reserved.contains(&candidate) {
            return Ok(candidate);
        }
        candidate = candidate
            .checked_add(1)
            .ok_or(CapabilityError::ExportIdExhausted)?;
    }
}

fn reference_total(mut values: impl Iterator<Item = u64>) -> u64 {
    values.try_fold(0_u64, u64::checked_add).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::{
        OwnedResource, ThirdPartyCapDescriptor, ThirdPartyToContact, bind_attached_resources,
    };

    #[test]
    fn duplicate_descriptors_share_identity_but_count_exactly() {
        let hosted = HostedCapability::new().expect("identity");
        let mut sender = CapabilityTables::new(8, 8);
        let first = sender
            .describe(&OutgoingCapability::Hosted(hosted.clone()))
            .expect("first export");
        let second = sender
            .describe(&OutgoingCapability::Hosted(hosted))
            .expect("duplicate export");
        assert_eq!(first, second);
        assert_eq!(sender.stats().export_references, 2);

        let mut receiver = CapabilityTables::new(8, 8);
        assert_eq!(
            receiver.receive(&first),
            Ok(ReceivedCapability::Imported(0))
        );
        assert_eq!(
            receiver.receive(&second),
            Ok(ReceivedCapability::Imported(0))
        );
        assert_eq!(receiver.stats().import_references, 2);
        let release = receiver.release_import(0, 2).expect("batched release");
        sender.apply_release(release).expect("exact release");
        assert_eq!(sender.stats(), CapabilityStats::default());
        assert_eq!(receiver.stats(), CapabilityStats::default());
    }

    #[test]
    fn receiver_hosted_round_trip_preserves_local_identity() {
        let hosted = HostedCapability::new().expect("identity");
        let mut owner = CapabilityTables::new(8, 8);
        let descriptor = owner
            .describe(&OutgoingCapability::Hosted(hosted.clone()))
            .expect("exports");
        let mut peer = CapabilityTables::new(8, 8);
        let ReceivedCapability::Imported(id) = peer.receive(&descriptor).expect("imports") else {
            panic!("import")
        };
        let returned = peer
            .describe(&OutgoingCapability::ReceiverHosted(id))
            .expect("returns");
        assert_eq!(
            owner.receive(&returned),
            Ok(ReceivedCapability::Hosted(hosted))
        );
        assert_eq!(owner.stats().export_references, 1);
    }

    #[test]
    fn third_party_descriptor_keeps_the_vine_as_an_exact_fallback_import() {
        let mut receiver = CapabilityTables::new(8, 8);
        let descriptor = CapDescriptor::ThirdPartyHosted(ThirdPartyCapDescriptor {
            id: ThirdPartyToContact::null(),
            vine_id: 6,
        });
        assert_eq!(
            receiver.receive(&descriptor),
            Ok(ReceivedCapability::Imported(6))
        );
        assert_eq!(receiver.stats().import_references, 1);
        assert_eq!(
            receiver.release_import(6, 1),
            Ok(ReleaseMessage {
                id: 6,
                reference_count: 1,
            })
        );
        assert_eq!(receiver.stats(), CapabilityStats::default());
    }

    #[test]
    fn promise_descriptors_count_references_and_reserve_released_ids() {
        let promise = PromiseCapability::new().expect("identity");
        let mut owner = CapabilityTables::new(8, 8);
        assert_eq!(
            owner
                .describe(&OutgoingCapability::Promise(promise.clone()))
                .expect("first promise export"),
            CapDescriptor::SenderPromise(0)
        );
        assert_eq!(
            owner
                .describe(&OutgoingCapability::Promise(promise.clone()))
                .expect("duplicate promise export"),
            CapDescriptor::SenderPromise(0)
        );
        assert_eq!(owner.promise_export_id(promise.identity()), Some(0));

        let mut peer = CapabilityTables::new(8, 8);
        assert_eq!(
            peer.receive(&CapDescriptor::SenderPromise(0)),
            Ok(ReceivedCapability::PromiseImported(0))
        );
        assert!(peer.is_promise_import(0));
        owner
            .apply_release(ReleaseMessage {
                id: 0,
                reference_count: 2,
            })
            .expect("release promise");
        assert!(!owner.contains_export(0));

        let hosted = HostedCapability::new().expect("hosted identity");
        assert_eq!(
            owner
                .describe(&OutgoingCapability::Hosted(hosted))
                .expect("reserved ID skipped"),
            CapDescriptor::SenderHosted(1)
        );
        owner.release_promise_reservation(0);
        let other = HostedCapability::new().expect("other hosted identity");
        assert_eq!(
            owner
                .describe(&OutgoingCapability::Hosted(other))
                .expect("released reservation reused"),
            CapDescriptor::SenderHosted(0)
        );
    }

    #[test]
    fn quotas_and_invalid_releases_are_complete_or_unchanged() {
        let hosted = HostedCapability::new().expect("identity");
        let mut table = CapabilityTables::new(1, 1);
        table
            .describe(&OutgoingCapability::Hosted(hosted))
            .expect("first export");
        let other = HostedCapability::new().expect("identity");
        assert!(matches!(
            table.describe(&OutgoingCapability::Hosted(other)),
            Err(CapabilityError::ExportLimit { limit: 1 })
        ));
        assert_eq!(table.stats().export_references, 1);
        assert!(matches!(
            table.apply_release(ReleaseMessage {
                id: 0,
                reference_count: 2
            }),
            Err(CapabilityError::ExcessRelease { held: 1, .. })
        ));
        assert_eq!(table.stats().export_references, 1);
        table.clear();
        assert_eq!(table.stats(), CapabilityStats::default());
    }

    #[test]
    fn attached_capabilities_move_resources_and_preserve_first_import_owner() {
        let hosted = HostedCapability::new().expect("identity");
        let first = AttachedResource::new(11_u32, 4);
        let second = AttachedResource::new(22_u32, 4);
        let outgoing = [
            OutgoingCapability::Hosted(hosted.clone()).with_attachment(first.clone()),
            OutgoingCapability::Hosted(hosted).with_attachment(second.clone()),
        ];
        let mut sender = CapabilityTables::new(8, 8);
        assert!(matches!(
            sender.describe(&outgoing[0]),
            Err(CapabilityError::UnsupportedDescriptor(_))
        ));
        let (mut descriptors, resources) = sender
            .describe_all_with_resources(&outgoing)
            .expect("attached payload");
        assert_eq!(resources.len(), 2);
        assert_eq!(descriptors[0].resource_index(), Some(0));
        assert_eq!(descriptors[1].resource_index(), Some(1));
        assert_eq!(
            bind_attached_resources(&mut descriptors, resources).attached,
            2
        );

        let mut receiver = CapabilityTables::new(8, 8);
        assert_eq!(
            receiver.receive(&descriptors[0]),
            Ok(ReceivedCapability::Imported(0))
        );
        assert_eq!(
            receiver.receive(&descriptors[1]),
            Ok(ReceivedCapability::Imported(0))
        );
        let imported = receiver.import_attachment(0).expect("first attachment");
        assert!(imported.same_identity(&first));
        assert_eq!(imported.with::<u32, _>(|value| *value), Some(11));
        assert!(!imported.same_identity(&second));
    }

    #[test]
    fn attached_resource_index_quota_is_transactional() {
        let capabilities = (0..=u8::MAX)
            .map(|value| {
                OutgoingCapability::None.with_attachment(AttachedResource::new(u16::from(value), 0))
            })
            .collect::<Vec<_>>();
        let mut table = CapabilityTables::new(8, 8);
        assert!(matches!(
            table.describe_all_with_resources(&capabilities),
            Err(CapabilityError::AttachedResourceLimit { limit: 255 })
        ));
        assert_eq!(table.stats(), CapabilityStats::default());

        let mut descriptors = vec![CapDescriptor::None.with_attachment(0, None)];
        let stats = bind_attached_resources(
            &mut descriptors,
            vec![OwnedResource::new(AttachedResource::new(1_u8, 0), 0)],
        );
        assert_eq!(stats.attached, 1);
    }
}
