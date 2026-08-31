//! Integration of streaming, cancellation, authenticated handoff, and persistence.

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::sync::{Arc, Mutex};

use capnp_compat::{ByteSink, ByteStream};
use capnp_message::{ExclusiveArena, ReaderLimits};
use capnp_rpc::{
    AuthenticatedOwner, AuthenticatedVatId, DistributedJoin, GrantRecord, HandoffPlan,
    IntroducedConnection, Introduction, IssuedGrant, JoinCandidate, JoinLimits, JoinNetwork,
    JoinStart, JoinedCapability, Level3Limits, Level3Network, Level3Router, NewJoin,
    PersistenceLimits, PersistenceManager, PersistenceRealm, PersistentResolver, PersistentStore,
    RealmIssuedGrant, RealmValidatedSturdyRef, SaveOptions, StoredGrant, SturdyRef,
    ValidatedSturdyRef,
};
use capnp_rpc_core::{
    JoinKeyPart, JoinResult, ThirdPartyCompletion, ThirdPartyToAwait, ThirdPartyToContact,
};
use capnp_schema::OpaquePointer;

use crate::ExampleResult;

/// Evidence emitted by the platform integration scenario.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformRun {
    pub streamed_bytes: Vec<u8>,
    pub clean_ends: usize,
    pub cancellations: usize,
    pub direct_handoff: bool,
    pub distributed_equality: bool,
    pub restored_object: u64,
    pub original_connection: u64,
    pub restored_connection: u64,
}

/// Runs the non-calculator platform facilities required by the M47 example.
pub fn run() -> ExampleResult<PlatformRun> {
    let recording = Arc::new(Mutex::new(Recording::default()));
    let mut stream = ByteStream::new(RecordingSink(Arc::clone(&recording)));
    stream.write(b"ordered ")?;
    stream.write(b"stream")?;
    stream.end()?;
    let mut canceled = ByteStream::new(RecordingSink(Arc::clone(&recording)));
    canceled.write(b" discarded")?;
    canceled.cancel()?;

    let mut router = Level3Router::<_, u64>::new(DemoNetwork::default(), Level3Limits::default());
    let direct_handoff = matches!(
        router.plan_introduction(&1, &2)?,
        HandoffPlan::Introduce { .. }
    );
    let mut equality = DistributedJoin::new(DemoJoinNetwork, JoinLimits::default());
    let distributed_equality = matches!(
        equality.begin(vec![JoinCandidate::Local(7), JoinCandidate::Local(7)])?,
        JoinStart::Direct(JoinedCapability::Local(7))
    );

    let original = DurableCapability {
        object: 7,
        connection: 44,
    };
    let mut manager = persistence_manager(100);
    let token = manager.save(
        &original,
        SaveOptions {
            seal_for: Some(9),
            expires_at: Some(500),
        },
        100,
    )?;
    let (realm, store, _) = manager.into_parts();
    let mut restarted = PersistenceManager::new(
        realm,
        store,
        DemoResolver {
            next_connection: 900,
        },
        PersistenceLimits::default(),
    );
    let restored =
        restarted.restore(&token, Some(&AuthenticatedOwner::new_authenticated(9)), 200)?;

    let recording = recording
        .lock()
        .map_err(|_| io::Error::other("stream recording lock poisoned"))?;
    Ok(PlatformRun {
        streamed_bytes: recording.bytes.clone(),
        clean_ends: recording.ends,
        cancellations: recording.cancellations,
        direct_handoff,
        distributed_equality,
        restored_object: restored.object,
        original_connection: original.connection,
        restored_connection: restored.connection,
    })
}

#[derive(Debug, Default)]
struct Recording {
    bytes: Vec<u8>,
    ends: usize,
    cancellations: usize,
}

#[derive(Clone, Debug)]
struct RecordingSink(Arc<Mutex<Recording>>);

impl ByteSink for RecordingSink {
    type Error = io::Error;

    fn write(&mut self, bytes: &[u8]) -> Result<(), Self::Error> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("stream recording lock poisoned"))?
            .bytes
            .extend_from_slice(bytes);
        Ok(())
    }

    fn end(&mut self) -> Result<(), Self::Error> {
        self.0
            .lock()
            .map_err(|_| io::Error::other("stream recording lock poisoned"))?
            .ends += 1;
        Ok(())
    }

    fn start_tls(&mut self, _expected_server_hostname: &str) -> Result<(), Self::Error> {
        Ok(())
    }

    fn cancel(&mut self) {
        if let Ok(mut recording) = self.0.lock() {
            recording.cancellations += 1;
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DemoError {
    InvalidToken,
    Unsupported,
}

impl fmt::Display for DemoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for DemoError {}

#[derive(Debug)]
struct DemoJoinNetwork;

impl JoinNetwork for DemoJoinNetwork {
    type Connection = u8;
    type VatId = &'static str;
    type Object = u64;
    type JoinSession = ();
    type Error = DemoError;

    fn authenticated_peer(
        &self,
        _connection: &Self::Connection,
    ) -> Result<AuthenticatedVatId<Self::VatId>, Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn begin_join(&mut self, _count: usize) -> Result<NewJoin<Self::JoinSession>, Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn accept_join_part(
        &mut self,
        _source: &Self::Connection,
        _object: &Self::Object,
        _part: &JoinKeyPart,
    ) -> Result<JoinResult, Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn forward_join_part(
        &mut self,
        _source: &Self::Connection,
        _destination: &Self::Connection,
        _part: &JoinKeyPart,
    ) -> Result<JoinKeyPart, Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn relay_join_result(
        &mut self,
        _source: &Self::Connection,
        _destination: &Self::Connection,
        _result: &JoinResult,
    ) -> Result<JoinResult, Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn add_join_result(
        &mut self,
        _session: &Self::JoinSession,
        _path_index: usize,
        _source: &Self::Connection,
        _result: &JoinResult,
    ) -> Result<(), Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn connect_join(
        &mut self,
        _session: &Self::JoinSession,
    ) -> Result<IntroducedConnection<Self::Connection>, Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn cancel_join(&mut self, _session: &Self::JoinSession) -> Result<(), Self::Error> {
        Err(DemoError::Unsupported)
    }
}

#[derive(Debug, Default)]
struct DemoNetwork {
    next_token: u64,
}

impl DemoNetwork {
    fn pointer(&mut self) -> Result<OpaquePointer, DemoError> {
        self.next_token = self.next_token.saturating_add(1);
        let mut arena = ExclusiveArena::new(2, 16).map_err(|_| DemoError::InvalidToken)?;
        arena
            .init_root_struct(1, 0)
            .and_then(|mut root| root.set_u64(0, self.next_token, 0))
            .map_err(|_| DemoError::InvalidToken)?;
        OpaquePointer::from_root_segments(arena.into_segments(), ReaderLimits::default())
            .map_err(|_| DemoError::InvalidToken)
    }
}

impl Level3Network for DemoNetwork {
    type Connection = u8;
    type VatId = &'static str;
    type Rendezvous = u64;
    type Error = DemoError;

    fn authenticated_peer(
        &self,
        connection: &Self::Connection,
    ) -> Result<AuthenticatedVatId<Self::VatId>, Self::Error> {
        match connection {
            1 => Ok(AuthenticatedVatId::new_authenticated("provider")),
            2 => Ok(AuthenticatedVatId::new_authenticated("recipient")),
            _ => Err(DemoError::InvalidToken),
        }
    }

    fn can_introduce(&self, provider: &Self::Connection, recipient: &Self::Connection) -> bool {
        *provider == 1 && *recipient == 2
    }

    fn introduce(
        &mut self,
        _provider: &Self::Connection,
        _recipient: &Self::Connection,
    ) -> Result<Introduction, Self::Error> {
        Ok(Introduction {
            contact: ThirdPartyToContact::from_opaque(self.pointer()?),
            recipient: ThirdPartyToAwait::from_opaque(self.pointer()?),
        })
    }

    fn connect_to_introduced(
        &mut self,
        _source: &Self::Connection,
        _contact: &ThirdPartyToContact,
    ) -> Result<IntroducedConnection<Self::Connection>, Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn can_forward(
        &self,
        _source: &Self::Connection,
        _contact: &ThirdPartyToContact,
        _destination: &Self::Connection,
    ) -> bool {
        false
    }

    fn forward(
        &mut self,
        _source: &Self::Connection,
        _contact: &ThirdPartyToContact,
        _destination: &Self::Connection,
    ) -> Result<ThirdPartyToContact, Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn await_rendezvous(
        &self,
        _source: &Self::Connection,
        _token: &ThirdPartyToAwait,
    ) -> Result<Self::Rendezvous, Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn completion_rendezvous(
        &self,
        _source: &Self::Connection,
        _token: &ThirdPartyCompletion,
    ) -> Result<Self::Rendezvous, Self::Error> {
        Err(DemoError::Unsupported)
    }

    fn generate_embargo_id(
        &mut self,
        _provision: &Self::Rendezvous,
    ) -> Result<Vec<u8>, Self::Error> {
        Err(DemoError::Unsupported)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DurableCapability {
    object: u64,
    connection: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DemoRealmMarker;

#[derive(Debug)]
struct DemoRealm {
    next_grant: u64,
}

impl PersistenceRealm for DemoRealm {
    type Realm = DemoRealmMarker;
    type GrantId = u64;
    type StableObjectId = u64;
    type Owner = u64;
    type Error = DemoError;

    fn issue(
        &mut self,
        stable_object_id: &Self::StableObjectId,
        options: &SaveOptions<Self::Owner>,
    ) -> Result<RealmIssuedGrant<Self>, Self::Error> {
        let grant_id = self.next_grant;
        self.next_grant = self.next_grant.saturating_add(1);
        let mut bytes = grant_id.to_le_bytes().to_vec();
        bytes.extend_from_slice(&stable_object_id.to_le_bytes());
        Ok(IssuedGrant {
            sturdy_ref: SturdyRef::from_bytes(bytes),
            record: GrantRecord {
                grant_id,
                stable_object_id: *stable_object_id,
                sealed_for: options.seal_for,
                expires_at: options.expires_at,
                revoked: false,
            },
        })
    }

    fn validate(
        &self,
        sturdy_ref: &SturdyRef<Self::Realm>,
    ) -> Result<RealmValidatedSturdyRef<Self>, Self::Error> {
        let bytes = sturdy_ref.as_bytes();
        let grant = bytes
            .get(0..8)
            .and_then(|value| value.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or(DemoError::InvalidToken)?;
        let object = bytes
            .get(8..16)
            .and_then(|value| value.try_into().ok())
            .map(u64::from_le_bytes)
            .ok_or(DemoError::InvalidToken)?;
        Ok(ValidatedSturdyRef {
            grant_id: grant,
            stable_object_id: object,
            sealed_for: Some(9),
            expires_at: Some(500),
        })
    }
}

#[derive(Debug, Default)]
struct DemoStore {
    grants: BTreeMap<u64, GrantRecord<u64, u64, u64>>,
}

impl PersistentStore for DemoStore {
    type GrantId = u64;
    type StableObjectId = u64;
    type Owner = u64;
    type Error = DemoError;

    fn insert(&mut self, record: StoredGrant<Self>) -> Result<(), Self::Error> {
        self.grants.insert(record.grant_id, record);
        Ok(())
    }

    fn get(&self, grant_id: &Self::GrantId) -> Result<Option<StoredGrant<Self>>, Self::Error> {
        Ok(self.grants.get(grant_id).cloned())
    }

    fn revoke_grant(&mut self, grant_id: &Self::GrantId) -> Result<bool, Self::Error> {
        let Some(record) = self.grants.get_mut(grant_id) else {
            return Ok(false);
        };
        let changed = !record.revoked;
        record.revoked = true;
        Ok(changed)
    }

    fn revoke_object(
        &mut self,
        stable_object_id: &Self::StableObjectId,
    ) -> Result<usize, Self::Error> {
        let mut count = 0;
        for record in self.grants.values_mut() {
            if record.stable_object_id == *stable_object_id && !record.revoked {
                record.revoked = true;
                count += 1;
            }
        }
        Ok(count)
    }
}

#[derive(Debug)]
struct DemoResolver {
    next_connection: u64,
}

impl PersistentResolver for DemoResolver {
    type StableObjectId = u64;
    type Capability = DurableCapability;
    type Error = DemoError;

    fn identify(&self, capability: &Self::Capability) -> Result<Self::StableObjectId, Self::Error> {
        Ok(capability.object)
    }

    fn restore(
        &mut self,
        stable_object_id: &Self::StableObjectId,
    ) -> Result<Self::Capability, Self::Error> {
        let connection = self.next_connection;
        self.next_connection = self.next_connection.saturating_add(1);
        Ok(DurableCapability {
            object: *stable_object_id,
            connection,
        })
    }
}

fn persistence_manager(
    next_connection: u64,
) -> PersistenceManager<DemoRealm, DemoStore, DemoResolver> {
    PersistenceManager::new(
        DemoRealm { next_grant: 1 },
        DemoStore::default(),
        DemoResolver { next_connection },
        PersistenceLimits::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_facilities_compose_without_widening_authority() -> ExampleResult<()> {
        let result = run()?;
        assert_eq!(result.streamed_bytes, b"ordered stream discarded");
        assert_eq!(result.clean_ends, 1);
        assert_eq!(result.cancellations, 1);
        assert!(result.direct_handoff);
        assert!(result.distributed_equality);
        assert_eq!(result.restored_object, 7);
        assert_eq!(result.original_connection, 44);
        assert_eq!(result.restored_connection, 900);
        Ok(())
    }
}
