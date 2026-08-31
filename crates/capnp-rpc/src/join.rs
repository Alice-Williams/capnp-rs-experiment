//! Network-parameterized Level-4 distributed capability equality.
//!
//! The pinned `rpc.capnp` schema defines the `Join` message and deliberately
//! leaves join-key construction, result authentication, and direct connection
//! establishment to the vat network. This coordinator never interprets those
//! opaque values. It authenticates every transport through [`JoinNetwork`],
//! keeps all join answers live until a direct connection succeeds, and fails
//! closed when paths disagree about their common hosted object.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;
use std::hash::Hash;

use capnp_rpc_core::{CallTarget, JoinKeyPart, JoinMessage, JoinResult};

use crate::{AuthenticatedVatId, IntroducedConnection};

/// Network-owned cryptographic and connection operations used by Join.
///
/// Opaque key parts and results are scoped to their authenticated source and
/// destination connections. Implementations must reject a value replayed on a
/// different connection, a forged value, or results naming different roots.
pub trait JoinNetwork {
    type Connection: Clone + fmt::Debug + Eq + Hash;
    type VatId: Clone + fmt::Debug + Eq + Hash;
    type Object: Clone + fmt::Debug + Eq + Hash;
    type JoinSession: fmt::Debug;
    type Error: std::error::Error + Send + Sync + 'static;

    fn authenticated_peer(
        &self,
        connection: &Self::Connection,
    ) -> Result<AuthenticatedVatId<Self::VatId>, Self::Error>;

    /// Creates exactly `count` independently scoped key parts.
    fn begin_join(&mut self, count: usize) -> Result<NewJoin<Self::JoinSession>, Self::Error>;

    /// Consumes a key part received from `source` for a locally hosted object.
    fn accept_join_part(
        &mut self,
        source: &Self::Connection,
        object: &Self::Object,
        part: &JoinKeyPart,
    ) -> Result<JoinResult, Self::Error>;

    /// Translates a key part across a fully transparent proxy hop.
    fn forward_join_part(
        &mut self,
        source: &Self::Connection,
        destination: &Self::Connection,
        part: &JoinKeyPart,
    ) -> Result<JoinKeyPart, Self::Error>;

    /// Translates a result back across a fully transparent proxy hop.
    fn relay_join_result(
        &mut self,
        source: &Self::Connection,
        destination: &Self::Connection,
        result: &JoinResult,
    ) -> Result<JoinResult, Self::Error>;

    /// Adds one authenticated result to a locally initiated session.
    fn add_join_result(
        &mut self,
        session: &Self::JoinSession,
        path_index: usize,
        source: &Self::Connection,
        result: &JoinResult,
    ) -> Result<(), Self::Error>;

    /// Connects only after all results prove the same hosted object and vat.
    fn connect_join(
        &mut self,
        session: &Self::JoinSession,
    ) -> Result<IntroducedConnection<Self::Connection>, Self::Error>;

    fn cancel_join(&mut self, session: &Self::JoinSession) -> Result<(), Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JoinLimits {
    pub max_paths: usize,
    pub max_joins: usize,
    pub max_forward_hops: usize,
}

impl Default for JoinLimits {
    fn default() -> Self {
        Self {
            max_paths: 64,
            max_joins: 1024,
            max_forward_hops: 64,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct JoinId(u64);

impl JoinId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Debug)]
pub struct NewJoin<S> {
    pub session: S,
    pub key_parts: Vec<JoinKeyPart>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JoinCandidate<C, O> {
    Local(O),
    Remote {
        connection: C,
        import_id: u32,
        question_id: u32,
    },
    Broken,
    Revoked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JoinedCapability<C, O> {
    Local(O),
    Remote { connection: C, import_id: u32 },
    Introduced(IntroducedConnection<C>),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinRequest<C> {
    pub join_id: JoinId,
    pub path_index: usize,
    pub connection: C,
    pub message: JoinMessage,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JoinStart<C, O> {
    Direct(JoinedCapability<C, O>),
    Pending {
        join_id: JoinId,
        requests: Vec<JoinRequest<C>>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinFinish<C> {
    pub connection: C,
    pub question_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JoinCompletion<C> {
    pub introduced: IntroducedConnection<C>,
    /// Join answers may be finished only after `introduced` is established.
    pub finishes: Vec<JoinFinish<C>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JoinProgress<C> {
    Pending,
    Complete(JoinCompletion<C>),
}

/// Result type returned when a network-parameterized Join is started.
pub type JoinBeginResult<N> = Result<
    JoinStart<<N as JoinNetwork>::Connection, <N as JoinNetwork>::Object>,
    JoinError<<N as JoinNetwork>::Error>,
>;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JoinResolution<C, O> {
    Root(O),
    TransparentProxy {
        destination: C,
        target: CallTarget,
        forward_question_id: u32,
        hop_count: usize,
    },
    OpaqueProxy,
    Broken,
    Revoked,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JoinAction<C> {
    Return {
        question_id: u32,
        result: JoinResult,
    },
    Forward {
        incoming_question_id: u32,
        destination: C,
        message: JoinMessage,
    },
}

#[derive(Debug)]
pub enum JoinError<E> {
    Network(E),
    InvalidCount(usize),
    Limit {
        resource: &'static str,
        limit: usize,
    },
    Broken,
    Revoked,
    MixedLocalRemote,
    DifferentLocalObjects,
    DuplicateQuestion,
    NetworkContract(&'static str),
    UnknownJoin(JoinId),
    WrongPath,
    DuplicateResult,
    OpaqueProxy,
    ForwardHopLimit(usize),
}

impl<E: fmt::Display> fmt::Display for JoinError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(error) => error.fmt(formatter),
            Self::InvalidCount(count) => {
                write!(formatter, "Join requires at least two paths, got {count}")
            }
            Self::Limit { resource, limit } => {
                write!(formatter, "Join {resource} limit {limit} exceeded")
            }
            Self::Broken => formatter.write_str("cannot Join a broken capability"),
            Self::Revoked => formatter.write_str("cannot Join a revoked capability"),
            Self::MixedLocalRemote => {
                formatter.write_str("local and remote Join candidates cannot be securely compared")
            }
            Self::DifferentLocalObjects => {
                formatter.write_str("local Join candidates name different objects")
            }
            Self::DuplicateQuestion => {
                formatter.write_str("duplicate Join question on one connection")
            }
            Self::NetworkContract(detail) => {
                write!(formatter, "Join network contract violated: {detail}")
            }
            Self::UnknownJoin(id) => write!(formatter, "unknown Join session {}", id.get()),
            Self::WrongPath => formatter.write_str("Join result arrived on the wrong path"),
            Self::DuplicateResult => formatter.write_str("duplicate Join result"),
            Self::OpaqueProxy => formatter.write_str("Join cannot bypass a non-transparent proxy"),
            Self::ForwardHopLimit(limit) => {
                write!(formatter, "Join forwarding hop limit {limit} exceeded")
            }
        }
    }
}

impl<E> std::error::Error for JoinError<E>
where
    E: std::error::Error + 'static,
{
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Network(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug)]
struct JoinPath<C> {
    connection: C,
    question_id: u32,
    result: Option<JoinResult>,
}

#[derive(Debug)]
struct ActiveJoin<N: JoinNetwork> {
    session: N::JoinSession,
    paths: Vec<JoinPath<N::Connection>>,
}

/// Owns Level-4 session state without depending on an async executor.
#[derive(Debug)]
pub struct DistributedJoin<N: JoinNetwork> {
    network: N,
    limits: JoinLimits,
    next_id: u64,
    active: HashMap<JoinId, ActiveJoin<N>>,
    failed_finishes: VecDeque<JoinFinish<N::Connection>>,
}

impl<N: JoinNetwork> DistributedJoin<N> {
    pub fn new(network: N, limits: JoinLimits) -> Self {
        Self {
            network,
            limits,
            next_id: 0,
            active: HashMap::new(),
            failed_finishes: VecDeque::new(),
        }
    }

    pub fn network(&self) -> &N {
        &self.network
    }

    pub fn network_mut(&mut self) -> &mut N {
        &mut self.network
    }

    pub fn active_joins(&self) -> usize {
        self.active.len()
    }

    pub fn next_failed_finish(&mut self) -> Option<JoinFinish<N::Connection>> {
        self.failed_finishes.pop_front()
    }

    pub fn begin(
        &mut self,
        candidates: Vec<JoinCandidate<N::Connection, N::Object>>,
    ) -> JoinBeginResult<N> {
        let count = candidates.len();
        if count < 2 {
            return Err(JoinError::InvalidCount(count));
        }
        if count > self.limits.max_paths {
            return Err(JoinError::Limit {
                resource: "path",
                limit: self.limits.max_paths,
            });
        }
        if candidates
            .iter()
            .any(|candidate| matches!(candidate, JoinCandidate::Broken))
        {
            return Err(JoinError::Broken);
        }
        if candidates
            .iter()
            .any(|candidate| matches!(candidate, JoinCandidate::Revoked))
        {
            return Err(JoinError::Revoked);
        }

        let local_count = candidates
            .iter()
            .filter(|candidate| matches!(candidate, JoinCandidate::Local(_)))
            .count();
        if local_count != 0 {
            if local_count != count {
                return Err(JoinError::MixedLocalRemote);
            }
            let JoinCandidate::Local(first) = &candidates[0] else {
                unreachable!()
            };
            if candidates.iter().all(
                |candidate| matches!(candidate, JoinCandidate::Local(object) if object == first),
            ) {
                return Ok(JoinStart::Direct(JoinedCapability::Local(first.clone())));
            }
            return Err(JoinError::DifferentLocalObjects);
        }

        let remote = candidates
            .into_iter()
            .map(|candidate| match candidate {
                JoinCandidate::Remote {
                    connection,
                    import_id,
                    question_id,
                } => Ok((connection, import_id, question_id)),
                _ => Err(JoinError::MixedLocalRemote),
            })
            .collect::<Result<Vec<_>, _>>()?;

        for (connection, _, _) in &remote {
            self.network
                .authenticated_peer(connection)
                .map_err(JoinError::Network)?;
        }
        let (first_connection, first_import, _) = &remote[0];
        if remote.iter().all(|(connection, import_id, _)| {
            connection == first_connection && import_id == first_import
        }) {
            return Ok(JoinStart::Direct(JoinedCapability::Remote {
                connection: first_connection.clone(),
                import_id: *first_import,
            }));
        }

        if self.active.len() >= self.limits.max_joins {
            return Err(JoinError::Limit {
                resource: "active session",
                limit: self.limits.max_joins,
            });
        }
        let mut questions = HashSet::with_capacity(remote.len());
        for (connection, _, question_id) in &remote {
            if !questions.insert((connection.clone(), *question_id)) {
                return Err(JoinError::DuplicateQuestion);
            }
        }

        let NewJoin { session, key_parts } = self
            .network
            .begin_join(remote.len())
            .map_err(JoinError::Network)?;
        if key_parts.len() != remote.len() {
            let _ = self.network.cancel_join(&session);
            return Err(JoinError::NetworkContract(
                "begin_join returned the wrong number of key parts",
            ));
        }

        let join_id = JoinId(self.next_id);
        self.next_id = self.next_id.wrapping_add(1);
        let mut requests = Vec::with_capacity(remote.len());
        let mut paths = Vec::with_capacity(remote.len());
        for (path_index, ((connection, import_id, question_id), key_part)) in
            remote.into_iter().zip(key_parts).enumerate()
        {
            requests.push(JoinRequest {
                join_id,
                path_index,
                connection: connection.clone(),
                message: JoinMessage {
                    question_id,
                    target: CallTarget::ImportedCap(import_id),
                    key_part,
                },
            });
            paths.push(JoinPath {
                connection,
                question_id,
                result: None,
            });
        }
        self.active.insert(join_id, ActiveJoin { session, paths });
        Ok(JoinStart::Pending { join_id, requests })
    }

    pub fn add_result(
        &mut self,
        join_id: JoinId,
        path_index: usize,
        connection: &N::Connection,
        question_id: u32,
        result: JoinResult,
    ) -> Result<JoinProgress<N::Connection>, JoinError<N::Error>> {
        let Some(active) = self.active.get(&join_id) else {
            return Err(JoinError::UnknownJoin(join_id));
        };
        let Some(path) = active.paths.get(path_index) else {
            return Err(JoinError::WrongPath);
        };
        if &path.connection != connection || path.question_id != question_id {
            return Err(JoinError::WrongPath);
        }
        if path.result.is_some() {
            return Err(JoinError::DuplicateResult);
        }

        let add_result = {
            let active = self.active.get(&join_id).expect("checked above");
            self.network
                .add_join_result(&active.session, path_index, connection, &result)
        };
        if let Err(error) = add_result {
            self.abort_after_failure(join_id);
            return Err(JoinError::Network(error));
        }
        let active = self.active.get_mut(&join_id).expect("active Join");
        active.paths[path_index].result = Some(result);
        if active.paths.iter().any(|path| path.result.is_none()) {
            return Ok(JoinProgress::Pending);
        }

        let connect = {
            let active = self.active.get(&join_id).expect("active Join");
            self.network.connect_join(&active.session)
        };
        let introduced = match connect {
            Ok(introduced) => introduced,
            Err(error) => {
                self.abort_after_failure(join_id);
                return Err(JoinError::Network(error));
            }
        };
        let active = self.active.remove(&join_id).expect("active Join");
        let finishes = active
            .paths
            .into_iter()
            .map(|path| JoinFinish {
                connection: path.connection,
                question_id: path.question_id,
            })
            .collect();
        Ok(JoinProgress::Complete(JoinCompletion {
            introduced,
            finishes,
        }))
    }

    pub fn cancel(
        &mut self,
        join_id: JoinId,
    ) -> Result<Vec<JoinFinish<N::Connection>>, JoinError<N::Error>> {
        let Some(active) = self.active.remove(&join_id) else {
            return Err(JoinError::UnknownJoin(join_id));
        };
        let finishes = finishes(&active);
        if let Err(error) = self.network.cancel_join(&active.session) {
            self.failed_finishes.extend(finishes);
            return Err(JoinError::Network(error));
        }
        Ok(finishes)
    }

    pub fn route_join(
        &mut self,
        source: &N::Connection,
        message: JoinMessage,
        resolution: JoinResolution<N::Connection, N::Object>,
    ) -> Result<JoinAction<N::Connection>, JoinError<N::Error>> {
        self.network
            .authenticated_peer(source)
            .map_err(JoinError::Network)?;
        match resolution {
            JoinResolution::Root(object) => {
                let result = self
                    .network
                    .accept_join_part(source, &object, &message.key_part)
                    .map_err(JoinError::Network)?;
                Ok(JoinAction::Return {
                    question_id: message.question_id,
                    result,
                })
            }
            JoinResolution::TransparentProxy {
                destination,
                target,
                forward_question_id,
                hop_count,
            } => {
                if hop_count >= self.limits.max_forward_hops {
                    return Err(JoinError::ForwardHopLimit(self.limits.max_forward_hops));
                }
                self.network
                    .authenticated_peer(&destination)
                    .map_err(JoinError::Network)?;
                let key_part = self
                    .network
                    .forward_join_part(source, &destination, &message.key_part)
                    .map_err(JoinError::Network)?;
                Ok(JoinAction::Forward {
                    incoming_question_id: message.question_id,
                    destination,
                    message: JoinMessage {
                        question_id: forward_question_id,
                        target,
                        key_part,
                    },
                })
            }
            JoinResolution::OpaqueProxy => Err(JoinError::OpaqueProxy),
            JoinResolution::Broken => Err(JoinError::Broken),
            JoinResolution::Revoked => Err(JoinError::Revoked),
        }
    }

    pub fn relay_result(
        &mut self,
        source: &N::Connection,
        destination: &N::Connection,
        result: &JoinResult,
    ) -> Result<JoinResult, JoinError<N::Error>> {
        self.network
            .authenticated_peer(source)
            .map_err(JoinError::Network)?;
        self.network
            .authenticated_peer(destination)
            .map_err(JoinError::Network)?;
        self.network
            .relay_join_result(source, destination, result)
            .map_err(JoinError::Network)
    }

    fn abort_after_failure(&mut self, join_id: JoinId) {
        if let Some(active) = self.active.remove(&join_id) {
            let queued = finishes(&active);
            let _ = self.network.cancel_join(&active.session);
            self.failed_finishes.extend(queued);
        }
    }
}

fn finishes<N: JoinNetwork>(active: &ActiveJoin<N>) -> Vec<JoinFinish<N::Connection>> {
    active
        .paths
        .iter()
        .map(|path| JoinFinish {
            connection: path.connection.clone(),
            question_id: path.question_id,
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::error::Error;

    use capnp_message::{ExclusiveArena, ReaderLimits};
    use capnp_rpc_core::ThirdPartyCompletion;
    use capnp_schema::OpaquePointer;

    #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
    struct Root {
        host: u8,
        object: u64,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    struct TestSession(u64);

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestError {
        UnknownConnection,
        ForgedToken,
        WrongSession,
        WrongPath,
        MismatchedRoot,
        Incomplete,
    }

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "{self:?}")
        }
    }

    impl Error for TestError {}

    #[derive(Clone, Debug)]
    struct PartRecord {
        token: JoinKeyPart,
        session: TestSession,
        path: usize,
    }

    #[derive(Clone, Debug)]
    struct ResultRecord {
        token: JoinResult,
        session: TestSession,
        path: usize,
        source: u8,
        root: Root,
    }

    #[derive(Debug)]
    struct SessionRecord {
        id: TestSession,
        roots: Vec<Option<Root>>,
        canceled: bool,
    }

    #[derive(Debug)]
    struct TestNetwork {
        peers: HashMap<u8, &'static str>,
        next_token: u64,
        next_session: u64,
        parts: Vec<PartRecord>,
        results: Vec<ResultRecord>,
        sessions: Vec<SessionRecord>,
        begin_count: usize,
        connect_count: usize,
        cancel_count: usize,
        forward_count: usize,
        relay_count: usize,
    }

    impl TestNetwork {
        fn new() -> Self {
            Self {
                peers: HashMap::from([
                    (1, "one"),
                    (2, "two"),
                    (3, "three"),
                    (4, "four"),
                    (5, "five"),
                ]),
                next_token: 1,
                next_session: 1,
                parts: Vec::new(),
                results: Vec::new(),
                sessions: Vec::new(),
                begin_count: 0,
                connect_count: 0,
                cancel_count: 0,
                forward_count: 0,
                relay_count: 0,
            }
        }

        fn pointer(&mut self) -> OpaquePointer {
            let value = self.next_token;
            self.next_token += 1;
            let mut arena = ExclusiveArena::new(2, 16).expect("token arena");
            arena
                .init_root_struct(1, 0)
                .expect("token root")
                .set_u64(0, value, 0)
                .expect("token value");
            OpaquePointer::from_root_segments(arena.into_segments(), ReaderLimits::default())
                .expect("opaque token")
        }

        fn forged_result(&mut self) -> JoinResult {
            JoinResult::from_opaque(self.pointer())
        }

        fn part(&self, token: &JoinKeyPart) -> Result<PartRecord, TestError> {
            self.parts
                .iter()
                .find(|entry| entry.token == *token)
                .cloned()
                .ok_or(TestError::ForgedToken)
        }

        fn result(&self, token: &JoinResult, source: u8) -> Result<ResultRecord, TestError> {
            self.results
                .iter()
                .find(|entry| entry.token == *token && entry.source == source)
                .cloned()
                .ok_or(TestError::ForgedToken)
        }
    }

    impl JoinNetwork for TestNetwork {
        type Connection = u8;
        type VatId = &'static str;
        type Object = Root;
        type JoinSession = TestSession;
        type Error = TestError;

        fn authenticated_peer(
            &self,
            connection: &Self::Connection,
        ) -> Result<AuthenticatedVatId<Self::VatId>, Self::Error> {
            self.peers
                .get(connection)
                .copied()
                .map(AuthenticatedVatId::new_authenticated)
                .ok_or(TestError::UnknownConnection)
        }

        fn begin_join(&mut self, count: usize) -> Result<NewJoin<Self::JoinSession>, Self::Error> {
            let session = TestSession(self.next_session);
            self.next_session += 1;
            let mut key_parts = Vec::with_capacity(count);
            for path in 0..count {
                let token = JoinKeyPart::from_opaque(self.pointer());
                self.parts.push(PartRecord {
                    token: token.clone(),
                    session,
                    path,
                });
                key_parts.push(token);
            }
            self.sessions.push(SessionRecord {
                id: session,
                roots: vec![None; count],
                canceled: false,
            });
            self.begin_count += 1;
            Ok(NewJoin { session, key_parts })
        }

        fn accept_join_part(
            &mut self,
            source: &Self::Connection,
            object: &Self::Object,
            part: &JoinKeyPart,
        ) -> Result<JoinResult, Self::Error> {
            self.authenticated_peer(source)?;
            let part = self.part(part)?;
            let token = JoinResult::from_opaque(self.pointer());
            self.results.push(ResultRecord {
                token: token.clone(),
                session: part.session,
                path: part.path,
                source: *source,
                root: *object,
            });
            Ok(token)
        }

        fn forward_join_part(
            &mut self,
            source: &Self::Connection,
            destination: &Self::Connection,
            part: &JoinKeyPart,
        ) -> Result<JoinKeyPart, Self::Error> {
            self.authenticated_peer(source)?;
            self.authenticated_peer(destination)?;
            let part = self.part(part)?;
            let token = JoinKeyPart::from_opaque(self.pointer());
            self.parts.push(PartRecord {
                token: token.clone(),
                session: part.session,
                path: part.path,
            });
            self.forward_count += 1;
            Ok(token)
        }

        fn relay_join_result(
            &mut self,
            source: &Self::Connection,
            destination: &Self::Connection,
            result: &JoinResult,
        ) -> Result<JoinResult, Self::Error> {
            self.authenticated_peer(source)?;
            self.authenticated_peer(destination)?;
            let result = self.result(result, *source)?;
            let token = JoinResult::from_opaque(self.pointer());
            self.results.push(ResultRecord {
                token: token.clone(),
                source: *destination,
                ..result
            });
            self.relay_count += 1;
            Ok(token)
        }

        fn add_join_result(
            &mut self,
            session: &Self::JoinSession,
            path_index: usize,
            source: &Self::Connection,
            result: &JoinResult,
        ) -> Result<(), Self::Error> {
            let result = self.result(result, *source)?;
            if result.session != *session {
                return Err(TestError::WrongSession);
            }
            if result.path != path_index {
                return Err(TestError::WrongPath);
            }
            let state = self
                .sessions
                .iter_mut()
                .find(|entry| entry.id == *session && !entry.canceled)
                .ok_or(TestError::WrongSession)?;
            let Some(slot) = state.roots.get_mut(path_index) else {
                return Err(TestError::WrongPath);
            };
            if slot.is_some() {
                return Err(TestError::WrongPath);
            }
            *slot = Some(result.root);
            Ok(())
        }

        fn connect_join(
            &mut self,
            session: &Self::JoinSession,
        ) -> Result<IntroducedConnection<Self::Connection>, Self::Error> {
            let state = self
                .sessions
                .iter()
                .find(|entry| entry.id == *session && !entry.canceled)
                .ok_or(TestError::WrongSession)?;
            let roots = state
                .roots
                .iter()
                .copied()
                .collect::<Option<Vec<_>>>()
                .ok_or(TestError::Incomplete)?;
            if !roots.iter().all(|root| *root == roots[0]) {
                return Err(TestError::MismatchedRoot);
            }
            let host = roots[0].host;
            let completion = ThirdPartyCompletion::from_opaque(self.pointer());
            self.connect_count += 1;
            Ok(IntroducedConnection {
                connection: Some(host),
                completion,
            })
        }

        fn cancel_join(&mut self, session: &Self::JoinSession) -> Result<(), Self::Error> {
            let state = self
                .sessions
                .iter_mut()
                .find(|entry| entry.id == *session)
                .ok_or(TestError::WrongSession)?;
            state.canceled = true;
            self.cancel_count += 1;
            Ok(())
        }
    }

    fn remote(connection: u8, import_id: u32, question_id: u32) -> JoinCandidate<u8, Root> {
        JoinCandidate::Remote {
            connection,
            import_id,
            question_id,
        }
    }

    fn pending(start: JoinStart<u8, Root>) -> (JoinId, Vec<JoinRequest<u8>>) {
        let JoinStart::Pending { join_id, requests } = start else {
            panic!("pending Join")
        };
        (join_id, requests)
    }

    fn returned(action: JoinAction<u8>) -> JoinResult {
        let JoinAction::Return { result, .. } = action else {
            panic!("Join result")
        };
        result
    }

    #[test]
    fn identical_local_or_remote_capabilities_shortcut_without_network_join() {
        let mut join = DistributedJoin::new(TestNetwork::new(), JoinLimits::default());
        let root = Root { host: 5, object: 9 };
        assert_eq!(
            join.begin(vec![JoinCandidate::Local(root), JoinCandidate::Local(root)])
                .expect("local equality"),
            JoinStart::Direct(JoinedCapability::Local(root))
        );
        assert_eq!(
            join.begin(vec![remote(1, 7, 10), remote(1, 7, 11)])
                .expect("two-party equality"),
            JoinStart::Direct(JoinedCapability::Remote {
                connection: 1,
                import_id: 7,
            })
        );
        assert_eq!(join.network().begin_count, 0);
        assert_eq!(join.active_joins(), 0);
    }

    #[test]
    fn different_paths_to_one_root_connect_before_releasing_answers() {
        let mut join = DistributedJoin::new(TestNetwork::new(), JoinLimits::default());
        let (join_id, requests) = pending(
            join.begin(vec![remote(1, 10, 101), remote(2, 20, 202)])
                .expect("begin"),
        );
        let root = Root {
            host: 5,
            object: 77,
        };
        let first = returned(
            join.route_join(&1, requests[0].message.clone(), JoinResolution::Root(root))
                .expect("first root"),
        );
        assert_eq!(
            join.add_result(join_id, 0, &1, 101, first)
                .expect("first result"),
            JoinProgress::Pending
        );
        assert!(join.next_failed_finish().is_none());
        let second = returned(
            join.route_join(&2, requests[1].message.clone(), JoinResolution::Root(root))
                .expect("second root"),
        );
        let JoinProgress::Complete(completion) = join
            .add_result(join_id, 1, &2, 202, second)
            .expect("complete")
        else {
            panic!("complete Join")
        };
        assert_eq!(completion.introduced.connection, Some(5));
        assert_eq!(
            completion.finishes,
            vec![
                JoinFinish {
                    connection: 1,
                    question_id: 101
                },
                JoinFinish {
                    connection: 2,
                    question_id: 202
                },
            ]
        );
        assert_eq!(join.network().connect_count, 1);
    }

    #[test]
    fn transparent_and_reflected_forwarding_translate_both_opaque_directions() {
        let mut join = DistributedJoin::new(TestNetwork::new(), JoinLimits::default());
        let (join_id, requests) = pending(
            join.begin(vec![remote(1, 10, 101), remote(2, 20, 202)])
                .expect("begin"),
        );
        let JoinAction::Forward {
            destination: 3,
            message: at_three,
            ..
        } = join
            .route_join(
                &1,
                requests[0].message.clone(),
                JoinResolution::TransparentProxy {
                    destination: 3,
                    target: CallTarget::ImportedCap(30),
                    forward_question_id: 303,
                    hop_count: 0,
                },
            )
            .expect("first forward")
        else {
            panic!("forward")
        };
        let JoinAction::Forward {
            destination: 4,
            message: at_four,
            ..
        } = join
            .route_join(
                &3,
                at_three,
                JoinResolution::TransparentProxy {
                    destination: 4,
                    target: CallTarget::ImportedCap(40),
                    forward_question_id: 404,
                    hop_count: 1,
                },
            )
            .expect("reflected forward")
        else {
            panic!("forward")
        };
        let root = Root {
            host: 5,
            object: 88,
        };
        let from_four = returned(
            join.route_join(&4, at_four, JoinResolution::Root(root))
                .expect("root"),
        );
        let at_three = join.relay_result(&4, &3, &from_four).expect("first relay");
        let at_one = join.relay_result(&3, &1, &at_three).expect("second relay");
        assert_eq!(
            join.add_result(join_id, 0, &1, 101, at_one)
                .expect("first result"),
            JoinProgress::Pending
        );
        let second = returned(
            join.route_join(&2, requests[1].message.clone(), JoinResolution::Root(root))
                .expect("second root"),
        );
        assert!(matches!(
            join.add_result(join_id, 1, &2, 202, second)
                .expect("complete"),
            JoinProgress::Complete(_)
        ));
        assert_eq!(join.network().forward_count, 2);
        assert_eq!(join.network().relay_count, 2);
    }

    #[test]
    fn repeated_endpoint_paths_are_distinct_and_duplicate_results_are_rejected() {
        let mut join = DistributedJoin::new(TestNetwork::new(), JoinLimits::default());
        let (join_id, requests) = pending(
            join.begin(vec![
                remote(1, 10, 101),
                remote(1, 11, 102),
                remote(2, 20, 201),
            ])
            .expect("begin"),
        );
        assert_eq!(requests.len(), 3);
        assert_ne!(requests[0].message.key_part, requests[1].message.key_part);
        let root = Root {
            host: 5,
            object: 99,
        };
        let first = returned(
            join.route_join(&1, requests[0].message.clone(), JoinResolution::Root(root))
                .expect("root"),
        );
        join.add_result(join_id, 0, &1, 101, first.clone())
            .expect("first result");
        assert!(matches!(
            join.add_result(join_id, 0, &1, 101, first),
            Err(JoinError::DuplicateResult)
        ));
        assert_eq!(join.active_joins(), 1);
    }

    #[test]
    fn unrelated_roots_and_forged_results_fail_closed_and_finish_every_path() {
        let mut join = DistributedJoin::new(TestNetwork::new(), JoinLimits::default());
        let (join_id, _requests) = pending(
            join.begin(vec![remote(1, 10, 101), remote(2, 20, 202)])
                .expect("begin"),
        );
        let forged = join.network_mut().forged_result();
        assert!(matches!(
            join.add_result(join_id, 0, &1, 101, forged),
            Err(JoinError::Network(TestError::ForgedToken))
        ));
        assert_eq!(join.active_joins(), 0);
        assert_eq!(join.network().cancel_count, 1);
        assert_eq!(
            join.next_failed_finish()
                .expect("first cleanup")
                .question_id,
            101
        );
        assert_eq!(
            join.next_failed_finish()
                .expect("second cleanup")
                .question_id,
            202
        );

        let (join_id, requests) = pending(
            join.begin(vec![remote(1, 10, 301), remote(2, 20, 302)])
                .expect("second begin"),
        );
        let a = returned(
            join.route_join(
                &1,
                requests[0].message.clone(),
                JoinResolution::Root(Root { host: 5, object: 1 }),
            )
            .expect("root a"),
        );
        let b = returned(
            join.route_join(
                &2,
                requests[1].message.clone(),
                JoinResolution::Root(Root { host: 5, object: 2 }),
            )
            .expect("root b"),
        );
        assert_eq!(
            join.add_result(join_id, 0, &1, 301, a).expect("first"),
            JoinProgress::Pending
        );
        assert!(matches!(
            join.add_result(join_id, 1, &2, 302, b),
            Err(JoinError::Network(TestError::MismatchedRoot))
        ));
        assert_eq!(join.network().connect_count, 0);
        assert_eq!(join.network().cancel_count, 2);
    }

    #[test]
    fn broken_revoked_mixed_and_opaque_proxy_paths_are_rejected() {
        let mut join = DistributedJoin::new(TestNetwork::new(), JoinLimits::default());
        let root = Root { host: 5, object: 1 };
        assert!(matches!(
            join.begin(vec![JoinCandidate::Broken, remote(1, 1, 1)]),
            Err(JoinError::Broken)
        ));
        assert!(matches!(
            join.begin(vec![JoinCandidate::Revoked, remote(1, 1, 1)]),
            Err(JoinError::Revoked)
        ));
        assert!(matches!(
            join.begin(vec![JoinCandidate::Local(root), remote(1, 1, 1)]),
            Err(JoinError::MixedLocalRemote)
        ));
        let (_, requests) = pending(
            join.begin(vec![remote(1, 1, 10), remote(2, 2, 20)])
                .expect("begin"),
        );
        assert!(matches!(
            join.route_join(&1, requests[0].message.clone(), JoinResolution::OpaqueProxy),
            Err(JoinError::OpaqueProxy)
        ));
        assert!(matches!(
            join.route_join(
                &1,
                requests[0].message.clone(),
                JoinResolution::TransparentProxy {
                    destination: 2,
                    target: CallTarget::ImportedCap(2),
                    forward_question_id: 20,
                    hop_count: JoinLimits::default().max_forward_hops,
                },
            ),
            Err(JoinError::ForwardHopLimit(_))
        ));
    }

    #[test]
    fn cancellation_and_limits_are_transactional() {
        let limits = JoinLimits {
            max_paths: 3,
            max_joins: 1,
            max_forward_hops: 2,
        };
        let mut join = DistributedJoin::new(TestNetwork::new(), limits);
        assert!(matches!(
            join.begin(vec![
                remote(1, 1, 1),
                remote(2, 2, 2),
                remote(3, 3, 3),
                remote(4, 4, 4)
            ]),
            Err(JoinError::Limit {
                resource: "path",
                ..
            })
        ));
        assert_eq!(join.network().begin_count, 0);
        let (join_id, _) = pending(
            join.begin(vec![remote(1, 1, 10), remote(2, 2, 20)])
                .expect("begin"),
        );
        assert!(matches!(
            join.begin(vec![remote(3, 3, 30), remote(4, 4, 40)]),
            Err(JoinError::Limit {
                resource: "active session",
                ..
            })
        ));
        assert_eq!(join.network().begin_count, 1);
        assert_eq!(
            join.cancel(join_id).expect("cancel"),
            vec![
                JoinFinish {
                    connection: 1,
                    question_id: 10
                },
                JoinFinish {
                    connection: 2,
                    question_id: 20
                },
            ]
        );
        assert_eq!(join.active_joins(), 0);
        assert_eq!(join.network().cancel_count, 1);
    }
}
