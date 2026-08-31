//! Typed, realm-specific persistent capability coordination.
//!
//! The pinned `persistent.capnp` interface intentionally leaves SturdyRef and
//! Owner formats to the application realm. This module preserves that boundary:
//! realms authenticate opaque tokens, durable stores retain grants, and
//! resolvers map live capabilities to stable object IDs and back. Live
//! connection IDs never enter a [`GrantRecord`].

use std::fmt;
use std::marker::PhantomData;

/// An opaque, serializable SturdyRef branded by its application realm.
#[derive(Eq, Hash, PartialEq)]
pub struct SturdyRef<R> {
    bytes: Vec<u8>,
    realm: PhantomData<fn() -> R>,
}

impl<R> SturdyRef<R> {
    /// Loads an untrusted serialized token. Validation occurs during restore.
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            realm: PhantomData,
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl<R> Clone for SturdyRef<R> {
    fn clone(&self) -> Self {
        Self::from_bytes(self.bytes.clone())
    }
}

impl<R> fmt::Debug for SturdyRef<R> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SturdyRef")
            .field("byte_len", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

/// An owner identity produced only after realm-defined authentication.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AuthenticatedOwner<O>(O);

impl<O> AuthenticatedOwner<O> {
    /// Wraps an identity after an application or transport authenticated it.
    pub fn new_authenticated(owner: O) -> Self {
        Self(owner)
    }

    pub fn get(&self) -> &O {
        &self.0
    }

    pub fn into_inner(self) -> O {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersistenceLimits {
    pub max_token_bytes: usize,
}

impl Default for PersistenceLimits {
    fn default() -> Self {
        Self {
            max_token_bytes: 64 * 1024,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SaveOptions<O> {
    pub seal_for: Option<O>,
    /// Absolute application-defined time. The runtime treats `now >= expiry`
    /// as expired and never reads a wall clock itself.
    pub expires_at: Option<u64>,
}

impl<O> Default for SaveOptions<O> {
    fn default() -> Self {
        Self {
            seal_for: None,
            expires_at: None,
        }
    }
}

/// Canonical durable grant data. It deliberately has no connection field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GrantRecord<G, S, O> {
    pub grant_id: G,
    pub stable_object_id: S,
    pub sealed_for: Option<O>,
    pub expires_at: Option<u64>,
    pub revoked: bool,
}

/// Claims authenticated from an opaque SturdyRef by its realm.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedSturdyRef<G, S, O> {
    pub grant_id: G,
    pub stable_object_id: S,
    pub sealed_for: Option<O>,
    pub expires_at: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IssuedGrant<R, G, S, O> {
    pub sturdy_ref: SturdyRef<R>,
    pub record: GrantRecord<G, S, O>,
}

pub type RealmIssuedGrant<R> = IssuedGrant<
    <R as PersistenceRealm>::Realm,
    <R as PersistenceRealm>::GrantId,
    <R as PersistenceRealm>::StableObjectId,
    <R as PersistenceRealm>::Owner,
>;

pub type RealmValidatedSturdyRef<R> = ValidatedSturdyRef<
    <R as PersistenceRealm>::GrantId,
    <R as PersistenceRealm>::StableObjectId,
    <R as PersistenceRealm>::Owner,
>;

pub type StoredGrant<S> = GrantRecord<
    <S as PersistentStore>::GrantId,
    <S as PersistentStore>::StableObjectId,
    <S as PersistentStore>::Owner,
>;

pub type PersistenceResult<R, T> = Result<T, PersistenceError<<R as PersistenceRealm>::Error>>;

/// Realm-specific token authentication and issuance.
pub trait PersistenceRealm {
    type Realm;
    type GrantId: Clone + fmt::Debug + Eq;
    type StableObjectId: Clone + fmt::Debug + Eq;
    type Owner: Clone + fmt::Debug + Eq;
    type Error: std::error::Error + Send + Sync + 'static;

    fn issue(
        &mut self,
        stable_object_id: &Self::StableObjectId,
        options: &SaveOptions<Self::Owner>,
    ) -> Result<RealmIssuedGrant<Self>, Self::Error>;

    fn validate(
        &self,
        sturdy_ref: &SturdyRef<Self::Realm>,
    ) -> Result<RealmValidatedSturdyRef<Self>, Self::Error>;
}

/// Durable grant storage. Implementations must commit each mutation before
/// returning success so revocation survives process restart.
pub trait PersistentStore {
    type GrantId: Clone + fmt::Debug + Eq;
    type StableObjectId: Clone + fmt::Debug + Eq;
    type Owner: Clone + fmt::Debug + Eq;
    type Error: std::error::Error + Send + Sync + 'static;

    fn insert(
        &mut self,
        record: GrantRecord<Self::GrantId, Self::StableObjectId, Self::Owner>,
    ) -> Result<(), Self::Error>;

    fn get(&self, grant_id: &Self::GrantId) -> Result<Option<StoredGrant<Self>>, Self::Error>;

    fn revoke_grant(&mut self, grant_id: &Self::GrantId) -> Result<bool, Self::Error>;

    fn revoke_object(
        &mut self,
        stable_object_id: &Self::StableObjectId,
    ) -> Result<usize, Self::Error>;
}

/// Converts live capabilities to durable object IDs and resolves those IDs on
/// demand. `restore()` must establish fresh connection state when needed.
pub trait PersistentResolver {
    type StableObjectId: Clone + fmt::Debug + Eq;
    type Capability;
    type Error: std::error::Error + Send + Sync + 'static;

    fn identify(&self, capability: &Self::Capability) -> Result<Self::StableObjectId, Self::Error>;

    fn restore(
        &mut self,
        stable_object_id: &Self::StableObjectId,
    ) -> Result<Self::Capability, Self::Error>;
}

#[derive(Debug)]
pub enum PersistenceError<E> {
    Backend(E),
    TokenTooLarge { requested: usize, limit: usize },
    InvalidExpiry,
    UnknownReference,
    ClaimsMismatch,
    Expired,
    Revoked,
    OwnerRequired,
    WrongOwner,
}

impl<E: fmt::Display> fmt::Display for PersistenceError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(error) => error.fmt(formatter),
            Self::TokenTooLarge { requested, limit } => {
                write!(
                    formatter,
                    "SturdyRef size {requested} exceeds limit {limit}"
                )
            }
            Self::InvalidExpiry => formatter.write_str("SturdyRef expiry is not in the future"),
            Self::UnknownReference => formatter.write_str("unknown SturdyRef"),
            Self::ClaimsMismatch => {
                formatter.write_str("SturdyRef claims do not match durable state")
            }
            Self::Expired => formatter.write_str("SturdyRef has expired"),
            Self::Revoked => formatter.write_str("SturdyRef has been revoked"),
            Self::OwnerRequired => formatter.write_str("SturdyRef requires an authenticated owner"),
            Self::WrongOwner => formatter.write_str("SturdyRef is sealed to a different owner"),
        }
    }
}

impl<E> std::error::Error for PersistenceError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(error) => Some(error),
            _ => None,
        }
    }
}

/// Coordinates typed save, restore, expiry, owner sealing, and revocation.
#[derive(Debug)]
pub struct PersistenceManager<R, S, V> {
    realm: R,
    store: S,
    resolver: V,
    limits: PersistenceLimits,
}

impl<R, S, V> PersistenceManager<R, S, V>
where
    R: PersistenceRealm,
    S: PersistentStore<
            GrantId = R::GrantId,
            StableObjectId = R::StableObjectId,
            Owner = R::Owner,
            Error = R::Error,
        >,
    V: PersistentResolver<StableObjectId = R::StableObjectId, Error = R::Error>,
{
    pub fn new(realm: R, store: S, resolver: V, limits: PersistenceLimits) -> Self {
        Self {
            realm,
            store,
            resolver,
            limits,
        }
    }

    pub fn realm(&self) -> &R {
        &self.realm
    }

    pub fn store(&self) -> &S {
        &self.store
    }

    pub fn resolver(&self) -> &V {
        &self.resolver
    }

    pub fn into_parts(self) -> (R, S, V) {
        (self.realm, self.store, self.resolver)
    }

    pub fn save(
        &mut self,
        capability: &V::Capability,
        options: SaveOptions<R::Owner>,
        now: u64,
    ) -> Result<SturdyRef<R::Realm>, PersistenceError<R::Error>> {
        if options.expires_at.is_some_and(|expiry| now >= expiry) {
            return Err(PersistenceError::InvalidExpiry);
        }
        let stable_object_id = self
            .resolver
            .identify(capability)
            .map_err(PersistenceError::Backend)?;
        let issued = self
            .realm
            .issue(&stable_object_id, &options)
            .map_err(PersistenceError::Backend)?;
        self.check_token_size(&issued.sturdy_ref)?;
        if issued.record.stable_object_id != stable_object_id
            || issued.record.sealed_for != options.seal_for
            || issued.record.expires_at != options.expires_at
            || issued.record.revoked
        {
            return Err(PersistenceError::ClaimsMismatch);
        }
        self.store
            .insert(issued.record)
            .map_err(PersistenceError::Backend)?;
        Ok(issued.sturdy_ref)
    }

    pub fn restore(
        &mut self,
        sturdy_ref: &SturdyRef<R::Realm>,
        owner: Option<&AuthenticatedOwner<R::Owner>>,
        now: u64,
    ) -> Result<V::Capability, PersistenceError<R::Error>> {
        let record = self.lookup(sturdy_ref)?;
        if record.revoked {
            return Err(PersistenceError::Revoked);
        }
        if record.expires_at.is_some_and(|expiry| now >= expiry) {
            return Err(PersistenceError::Expired);
        }
        if let Some(expected) = &record.sealed_for {
            let Some(actual) = owner else {
                return Err(PersistenceError::OwnerRequired);
            };
            if actual.get() != expected {
                return Err(PersistenceError::WrongOwner);
            }
        }
        self.resolver
            .restore(&record.stable_object_id)
            .map_err(PersistenceError::Backend)
    }

    pub fn revoke(
        &mut self,
        sturdy_ref: &SturdyRef<R::Realm>,
    ) -> Result<bool, PersistenceError<R::Error>> {
        let record = self.lookup(sturdy_ref)?;
        self.store
            .revoke_grant(&record.grant_id)
            .map_err(PersistenceError::Backend)
    }

    pub fn revoke_object(
        &mut self,
        stable_object_id: &R::StableObjectId,
    ) -> Result<usize, PersistenceError<R::Error>> {
        self.store
            .revoke_object(stable_object_id)
            .map_err(PersistenceError::Backend)
    }

    fn lookup(&self, sturdy_ref: &SturdyRef<R::Realm>) -> PersistenceResult<R, StoredGrant<S>> {
        self.check_token_size(sturdy_ref)?;
        let claims = self
            .realm
            .validate(sturdy_ref)
            .map_err(PersistenceError::Backend)?;
        let record = self
            .store
            .get(&claims.grant_id)
            .map_err(PersistenceError::Backend)?
            .ok_or(PersistenceError::UnknownReference)?;
        if claims.stable_object_id != record.stable_object_id
            || claims.sealed_for != record.sealed_for
            || claims.expires_at != record.expires_at
        {
            return Err(PersistenceError::ClaimsMismatch);
        }
        Ok(record)
    }

    fn check_token_size(
        &self,
        sturdy_ref: &SturdyRef<R::Realm>,
    ) -> Result<(), PersistenceError<R::Error>> {
        let requested = sturdy_ref.as_bytes().len();
        if requested > self.limits.max_token_bytes {
            return Err(PersistenceError::TokenTooLarge {
                requested,
                limit: self.limits.max_token_bytes,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct TestRealmMarker;
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct GrantId(u64);
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct ObjectId(u64);
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct OwnerId(u64);
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct ConnectionId(u64);

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct Capability {
        object: ObjectId,
        connection: ConnectionId,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestError {
        InvalidToken,
        DuplicateGrant,
        UnknownObject,
    }

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "{self:?}")
        }
    }

    impl std::error::Error for TestError {}

    #[derive(Clone, Debug)]
    struct TestRealm {
        key: u64,
        next_grant: u64,
    }

    impl TestRealm {
        fn new(key: u64) -> Self {
            Self { key, next_grant: 1 }
        }

        fn tag(&self, bytes: &[u8]) -> u64 {
            bytes
                .iter()
                .fold(self.key ^ 0xcbf2_9ce4_8422_2325, |hash, byte| {
                    (hash ^ u64::from(*byte)).wrapping_mul(0x100_0000_01b3)
                })
        }

        fn encode(
            &self,
            grant: GrantId,
            object: ObjectId,
            owner: Option<OwnerId>,
            expiry: Option<u64>,
        ) -> SturdyRef<TestRealmMarker> {
            let mut bytes = b"STR1".to_vec();
            bytes.extend_from_slice(&grant.0.to_le_bytes());
            bytes.extend_from_slice(&object.0.to_le_bytes());
            bytes.push(u8::from(owner.is_some()));
            bytes.extend_from_slice(&owner.unwrap_or(OwnerId(0)).0.to_le_bytes());
            bytes.push(u8::from(expiry.is_some()));
            bytes.extend_from_slice(&expiry.unwrap_or(0).to_le_bytes());
            bytes.extend_from_slice(&self.tag(&bytes).to_le_bytes());
            SturdyRef::from_bytes(bytes)
        }

        fn decode(
            &self,
            token: &SturdyRef<TestRealmMarker>,
        ) -> Result<RealmValidatedSturdyRef<Self>, TestError> {
            let bytes = token.as_bytes();
            if bytes.len() != 46 || &bytes[..4] != b"STR1" {
                return Err(TestError::InvalidToken);
            }
            let supplied = u64::from_le_bytes(bytes[38..46].try_into().expect("tag"));
            if supplied != self.tag(&bytes[..38]) {
                return Err(TestError::InvalidToken);
            }
            let number = |range: std::ops::Range<usize>| {
                u64::from_le_bytes(bytes[range].try_into().expect("number"))
            };
            let owner = match bytes[20] {
                0 => None,
                1 => Some(OwnerId(number(21..29))),
                _ => return Err(TestError::InvalidToken),
            };
            let expires_at = match bytes[29] {
                0 => None,
                1 => Some(number(30..38)),
                _ => return Err(TestError::InvalidToken),
            };
            Ok(ValidatedSturdyRef {
                grant_id: GrantId(number(4..12)),
                stable_object_id: ObjectId(number(12..20)),
                sealed_for: owner,
                expires_at,
            })
        }
    }

    impl PersistenceRealm for TestRealm {
        type Realm = TestRealmMarker;
        type GrantId = GrantId;
        type StableObjectId = ObjectId;
        type Owner = OwnerId;
        type Error = TestError;

        fn issue(
            &mut self,
            stable_object_id: &Self::StableObjectId,
            options: &SaveOptions<Self::Owner>,
        ) -> Result<RealmIssuedGrant<Self>, Self::Error> {
            let grant_id = GrantId(self.next_grant);
            self.next_grant += 1;
            Ok(IssuedGrant {
                sturdy_ref: self.encode(
                    grant_id,
                    *stable_object_id,
                    options.seal_for,
                    options.expires_at,
                ),
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
            self.decode(sturdy_ref)
        }
    }

    #[derive(Clone, Debug, Default)]
    struct TestStore {
        records: Vec<GrantRecord<GrantId, ObjectId, OwnerId>>,
    }

    impl PersistentStore for TestStore {
        type GrantId = GrantId;
        type StableObjectId = ObjectId;
        type Owner = OwnerId;
        type Error = TestError;

        fn insert(&mut self, record: StoredGrant<Self>) -> Result<(), Self::Error> {
            if self
                .records
                .iter()
                .any(|entry| entry.grant_id == record.grant_id)
            {
                return Err(TestError::DuplicateGrant);
            }
            self.records.push(record);
            Ok(())
        }

        fn get(&self, grant_id: &Self::GrantId) -> Result<Option<StoredGrant<Self>>, Self::Error> {
            Ok(self
                .records
                .iter()
                .find(|entry| entry.grant_id == *grant_id)
                .cloned())
        }

        fn revoke_grant(&mut self, grant_id: &Self::GrantId) -> Result<bool, Self::Error> {
            let Some(record) = self
                .records
                .iter_mut()
                .find(|entry| entry.grant_id == *grant_id)
            else {
                return Ok(false);
            };
            let changed = !record.revoked;
            record.revoked = true;
            Ok(changed)
        }

        fn revoke_object(&mut self, object: &Self::StableObjectId) -> Result<usize, Self::Error> {
            let mut changed = 0;
            for record in &mut self.records {
                if record.stable_object_id == *object && !record.revoked {
                    record.revoked = true;
                    changed += 1;
                }
            }
            Ok(changed)
        }
    }

    #[derive(Debug)]
    struct TestResolver {
        objects: Vec<ObjectId>,
        next_connection: u64,
    }

    impl TestResolver {
        fn new(objects: Vec<ObjectId>, next_connection: u64) -> Self {
            Self {
                objects,
                next_connection,
            }
        }
    }

    impl PersistentResolver for TestResolver {
        type StableObjectId = ObjectId;
        type Capability = Capability;
        type Error = TestError;

        fn identify(&self, capability: &Self::Capability) -> Result<ObjectId, TestError> {
            self.objects
                .contains(&capability.object)
                .then_some(capability.object)
                .ok_or(TestError::UnknownObject)
        }

        fn restore(&mut self, object: &ObjectId) -> Result<Capability, TestError> {
            if !self.objects.contains(object) {
                return Err(TestError::UnknownObject);
            }
            let connection = ConnectionId(self.next_connection);
            self.next_connection += 1;
            Ok(Capability {
                object: *object,
                connection,
            })
        }
    }

    type Manager = PersistenceManager<TestRealm, TestStore, TestResolver>;

    fn manager(next_connection: u64) -> Manager {
        PersistenceManager::new(
            TestRealm::new(0xa11c_e5ec_5ec0_u64),
            TestStore::default(),
            TestResolver::new(vec![ObjectId(7), ObjectId(8)], next_connection),
            PersistenceLimits::default(),
        )
    }

    #[test]
    fn sealed_reference_restores_after_restart_over_fresh_connection_state() {
        let original = Capability {
            object: ObjectId(7),
            connection: ConnectionId(44),
        };
        let mut first = manager(100);
        let token = first
            .save(
                &original,
                SaveOptions {
                    seal_for: Some(OwnerId(9)),
                    expires_at: Some(500),
                },
                100,
            )
            .expect("save");
        assert_eq!(first.store().records[0].stable_object_id, ObjectId(7));
        let (realm, store, _) = first.into_parts();
        let mut restarted = PersistenceManager::new(
            realm,
            store,
            TestResolver::new(vec![ObjectId(7)], 900),
            PersistenceLimits::default(),
        );
        let restored = restarted
            .restore(
                &token,
                Some(&AuthenticatedOwner::new_authenticated(OwnerId(9))),
                200,
            )
            .expect("restore after restart");
        assert_eq!(restored.object, original.object);
        assert_eq!(restored.connection, ConnectionId(900));
        assert_ne!(restored.connection, original.connection);
    }

    #[test]
    fn invalid_expired_unauthorized_and_oversized_tokens_fail_closed() {
        let capability = Capability {
            object: ObjectId(7),
            connection: ConnectionId(1),
        };
        let mut manager = manager(10);
        assert!(matches!(
            manager.save(
                &capability,
                SaveOptions {
                    seal_for: None,
                    expires_at: Some(5)
                },
                5
            ),
            Err(PersistenceError::InvalidExpiry)
        ));
        let token = manager
            .save(
                &capability,
                SaveOptions {
                    seal_for: Some(OwnerId(2)),
                    expires_at: Some(50),
                },
                1,
            )
            .expect("save");
        assert!(matches!(
            manager.restore(&token, None, 2),
            Err(PersistenceError::OwnerRequired)
        ));
        assert!(matches!(
            manager.restore(
                &token,
                Some(&AuthenticatedOwner::new_authenticated(OwnerId(3))),
                2
            ),
            Err(PersistenceError::WrongOwner)
        ));
        assert!(matches!(
            manager.restore(
                &token,
                Some(&AuthenticatedOwner::new_authenticated(OwnerId(2))),
                50
            ),
            Err(PersistenceError::Expired)
        ));
        let mut tampered = token.clone().into_bytes();
        tampered[12] ^= 1;
        assert!(matches!(
            manager.restore(&SturdyRef::from_bytes(tampered), None, 2),
            Err(PersistenceError::Backend(TestError::InvalidToken))
        ));
        let oversized =
            SturdyRef::from_bytes(vec![0; PersistenceLimits::default().max_token_bytes + 1]);
        assert!(matches!(
            manager.restore(&oversized, None, 2),
            Err(PersistenceError::TokenTooLarge { .. })
        ));
    }

    #[test]
    fn reference_and_object_revocation_are_durable_and_non_owning() {
        let capability = Capability {
            object: ObjectId(7),
            connection: ConnectionId(1),
        };
        let mut manager = manager(10);
        let first = manager
            .save(&capability, SaveOptions::default(), 1)
            .expect("first");
        let second = manager
            .save(&capability, SaveOptions::default(), 1)
            .expect("second");
        drop(first.clone());
        assert_eq!(
            manager
                .restore(&first, None, 2)
                .expect("dropping a copy is inert")
                .object,
            ObjectId(7)
        );
        assert!(manager.revoke(&first).expect("revoke"));
        assert!(matches!(
            manager.restore(&first, None, 2),
            Err(PersistenceError::Revoked)
        ));
        assert_eq!(
            manager
                .restore(&second, None, 2)
                .expect("independent grant")
                .object,
            ObjectId(7)
        );
        assert_eq!(
            manager.revoke_object(&ObjectId(7)).expect("object revoke"),
            1
        );
        let (realm, store, _) = manager.into_parts();
        let mut restarted = PersistenceManager::new(
            realm,
            store,
            TestResolver::new(vec![ObjectId(7)], 99),
            PersistenceLimits::default(),
        );
        assert!(matches!(
            restarted.restore(&second, None, 3),
            Err(PersistenceError::Revoked)
        ));
    }
}
