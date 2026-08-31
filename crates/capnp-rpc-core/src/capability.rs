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
//! Promise descriptors, promised answers, resolution, embargo, third-party
//! handoff, and attached resources are intentionally not implemented here.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{CapDescriptor, ReleaseMessage};

static NEXT_HOSTED_IDENTITY: AtomicU64 = AtomicU64::new(1);

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutgoingCapability {
    None,
    Hosted(HostedCapability),
    /// A capability imported from this peer and now sent back to it.
    ReceiverHosted(u32),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReceivedCapability {
    None,
    /// A settled capability hosted by the peer, addressed by its export ID.
    Imported(u32),
    /// A capability previously exported by this connection endpoint.
    Hosted(HostedCapability),
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
    capability: HostedCapability,
    references: u64,
}

#[derive(Clone, Copy)]
struct ImportEntry {
    references: u64,
}

/// Connection-local import/export tables. The connection actor is the sole
/// mutable owner; handles exchange commands rather than locking this state.
#[derive(Clone)]
pub struct CapabilityTables {
    imports: BTreeMap<u32, ImportEntry>,
    exports: BTreeMap<u32, ExportEntry>,
    export_by_identity: BTreeMap<u64, u32>,
    max_imports: usize,
    max_exports: usize,
}

impl CapabilityTables {
    pub fn new(max_imports: usize, max_exports: usize) -> Self {
        Self {
            imports: BTreeMap::new(),
            exports: BTreeMap::new(),
            export_by_identity: BTreeMap::new(),
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
                if self.stats().export_references == u64::MAX {
                    return Err(CapabilityError::ReferenceCountOverflow);
                }
                if let Some(id) = self.export_by_identity.get(&capability.identity()).copied() {
                    let entry = self
                        .exports
                        .get_mut(&id)
                        .ok_or(CapabilityError::UnknownExport(id))?;
                    entry.references = entry
                        .references
                        .checked_add(1)
                        .ok_or(CapabilityError::ReferenceCountOverflow)?;
                    return Ok(CapDescriptor::SenderHosted(id));
                }
                if self.exports.len() >= self.max_exports {
                    return Err(CapabilityError::ExportLimit {
                        limit: self.max_exports,
                    });
                }
                let id = lowest_free_id(&self.exports)?;
                self.exports.insert(
                    id,
                    ExportEntry {
                        capability: capability.clone(),
                        references: 1,
                    },
                );
                self.export_by_identity.insert(capability.identity(), id);
                Ok(CapDescriptor::SenderHosted(id))
            }
            OutgoingCapability::ReceiverHosted(id) => {
                if !self.imports.contains_key(id) {
                    return Err(CapabilityError::UnknownImport(*id));
                }
                Ok(CapDescriptor::ReceiverHosted(*id))
            }
        }
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
                    self.imports.insert(*id, ImportEntry { references: 1 });
                }
                Ok(ReceivedCapability::Imported(*id))
            }
            CapDescriptor::ReceiverHosted(id) => self
                .exports
                .get(id)
                .map(|entry| ReceivedCapability::Hosted(entry.capability.clone()))
                .ok_or(CapabilityError::UnknownExport(*id)),
        }
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
            self.export_by_identity.remove(&entry.capability.identity());
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

    pub fn clear(&mut self) {
        self.imports.clear();
        self.exports.clear();
        self.export_by_identity.clear();
    }
}

fn lowest_free_id<T>(values: &BTreeMap<u32, T>) -> Result<u32, CapabilityError> {
    let mut candidate = 0_u32;
    for id in values.keys().copied() {
        if id != candidate {
            break;
        }
        candidate = candidate
            .checked_add(1)
            .ok_or(CapabilityError::ExportIdExhausted)?;
    }
    Ok(candidate)
}

fn reference_total(mut values: impl Iterator<Item = u64>) -> u64 {
    values.try_fold(0_u64, u64::checked_add).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

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
}
