//! Executor-neutral Level-3 handoff coordination.
//!
//! The pinned `rpc.capnp` schema is authoritative for the wire protocol. The
//! network owns authentication and interpretation of its opaque third-party
//! tokens; this module only coordinates authenticated connections, keeps a
//! proxy vine until a provision is finished, and preserves E-order while an
//! `Accept` is embargoed. Capability values are cloned, never synthesized or
//! combined, so forwarding cannot widen authority.

use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::hash::Hash;

use capnp_rpc_core::{
    AcceptMessage, CallTarget, DisembargoContext, DisembargoMessage, PromisedAnswer,
    ThirdPartyAnswerMessage, ThirdPartyCompletion, ThirdPartyToAwait, ThirdPartyToContact,
};

/// A peer identity authenticated by a [`Level3Network`] implementation.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AuthenticatedVatId<T>(T);

impl<T> AuthenticatedVatId<T> {
    /// Constructs an identity after the transport has authenticated its peer.
    ///
    /// Implementations should not use unverified address or message data here.
    pub fn new_authenticated(value: T) -> Self {
        Self(value)
    }

    pub fn get(&self) -> &T {
        &self.0
    }

    pub fn into_inner(self) -> T {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Introduction {
    pub contact: ThirdPartyToContact,
    pub recipient: ThirdPartyToAwait,
}

/// Result of connecting to an introduced party. `connection == None` means
/// that the introduced party is the current vat.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntroducedConnection<C> {
    pub connection: Option<C>,
    pub completion: ThirdPartyCompletion,
}

/// Network-specific authentication and opaque-token translation boundary.
///
/// Every token method receives the connection on which the token arrived (or
/// will be sent). Implementations must reject tokens outside that authenticated
/// context rather than treating token bytes as globally transferable bearer
/// authority.
pub trait Level3Network {
    type Connection: Clone + fmt::Debug + Eq + Hash;
    type VatId: Clone + fmt::Debug + Eq + Hash;
    type Rendezvous: Clone + fmt::Debug + Eq + Hash;
    type Error: std::error::Error + Send + Sync + 'static;

    fn authenticated_peer(
        &self,
        connection: &Self::Connection,
    ) -> Result<AuthenticatedVatId<Self::VatId>, Self::Error>;

    fn can_introduce(&self, provider: &Self::Connection, recipient: &Self::Connection) -> bool;

    fn introduce(
        &mut self,
        provider: &Self::Connection,
        recipient: &Self::Connection,
    ) -> Result<Introduction, Self::Error>;

    fn connect_to_introduced(
        &mut self,
        source: &Self::Connection,
        contact: &ThirdPartyToContact,
    ) -> Result<IntroducedConnection<Self::Connection>, Self::Error>;

    fn can_forward(
        &self,
        source: &Self::Connection,
        contact: &ThirdPartyToContact,
        destination: &Self::Connection,
    ) -> bool;

    fn forward(
        &mut self,
        source: &Self::Connection,
        contact: &ThirdPartyToContact,
        destination: &Self::Connection,
    ) -> Result<ThirdPartyToContact, Self::Error>;

    fn await_rendezvous(
        &self,
        source: &Self::Connection,
        token: &ThirdPartyToAwait,
    ) -> Result<Self::Rendezvous, Self::Error>;

    fn completion_rendezvous(
        &self,
        source: &Self::Connection,
        token: &ThirdPartyCompletion,
    ) -> Result<Self::Rendezvous, Self::Error>;

    fn generate_embargo_id(&mut self, provision: &Self::Rendezvous)
    -> Result<Vec<u8>, Self::Error>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Level3Limits {
    pub max_provisions: usize,
    pub max_pending_accepts: usize,
    pub max_embargoes_per_provision: usize,
    pub max_return_routes: usize,
}

impl Default for Level3Limits {
    fn default() -> Self {
        Self {
            max_provisions: 4096,
            max_pending_accepts: 4096,
            max_embargoes_per_provision: 1024,
            max_return_routes: 4096,
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ProvisionId<C> {
    pub connection: C,
    pub question_id: u32,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct AcceptId<C> {
    pub connection: C,
    pub question_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HandoffPlan<C> {
    /// The network minted contact/await tokens for a direct handoff.
    Introduce {
        provider: C,
        recipient: C,
        introduction: Introduction,
    },
    /// An already-received contact token was translated for another recipient.
    Forward {
        destination: C,
        contact: ThirdPartyToContact,
    },
    /// The recipient must continue using an ordinary sender-hosted vine.
    Proxy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcceptState {
    Ready,
    Pending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AcceptFailure {
    ProvisionFinished,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AcceptCompletion<C, A> {
    pub accept: AcceptId<C>,
    pub result: Result<A, AcceptFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisembargoAction {
    Released,
    /// The message targets an ordinary vine import and must continue along the
    /// existing proxy path before it reaches the provision's promised answer.
    ForwardVine {
        vine_id: u32,
        embargo: Vec<u8>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReturnRoute<C> {
    pub original_connection: C,
    pub original_question_id: u32,
    pub direct_connection: C,
    pub adopted_answer_id: u32,
}

#[derive(Debug)]
pub enum Level3Error<E> {
    Network(E),
    Limit {
        resource: &'static str,
        limit: usize,
    },
    DuplicateProvision,
    DuplicateAccept,
    DuplicateReturnAwait,
    DuplicateThirdPartyAnswer,
    UnknownProvision,
    UnknownAccept,
    UnknownReturnRoute,
    InvalidDisembargoTarget,
    InvalidThirdPartyAnswerId(u32),
}

impl<E: fmt::Display> fmt::Display for Level3Error<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Network(error) => error.fmt(formatter),
            Self::Limit { resource, limit } => {
                write!(formatter, "Level-3 {resource} limit {limit} exceeded")
            }
            Self::DuplicateProvision => formatter.write_str("duplicate Level-3 provision"),
            Self::DuplicateAccept => formatter.write_str("duplicate Level-3 accept question"),
            Self::DuplicateReturnAwait => {
                formatter.write_str("duplicate awaitFromThirdParty rendezvous")
            }
            Self::DuplicateThirdPartyAnswer => {
                formatter.write_str("duplicate ThirdPartyAnswer route or answer ID")
            }
            Self::UnknownProvision => formatter.write_str("unknown Level-3 provision"),
            Self::UnknownAccept => formatter.write_str("unknown Level-3 accept"),
            Self::UnknownReturnRoute => formatter.write_str("unknown third-party return route"),
            Self::InvalidDisembargoTarget => {
                formatter.write_str("third-party disembargo has an invalid target")
            }
            Self::InvalidThirdPartyAnswerId(id) => write!(
                formatter,
                "third-party answer ID {id} is outside [2^30, 2^31)"
            ),
        }
    }
}

impl<E: std::error::Error + 'static> std::error::Error for Level3Error<E> {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Network(error) => Some(error),
            _ => None,
        }
    }
}

impl<E> From<E> for Level3Error<E> {
    fn from(value: E) -> Self {
        Self::Network(value)
    }
}

#[derive(Clone, Debug)]
struct PendingAccept<R> {
    rendezvous: R,
    embargo: Vec<u8>,
}

#[derive(Clone, Debug)]
enum Embargo<C> {
    Waiting(Vec<AcceptId<C>>),
    Released,
}

#[derive(Clone, Debug)]
struct Provision<C, A> {
    capability: A,
    embargoes: HashMap<Vec<u8>, Embargo<C>>,
}

#[derive(Clone, Debug)]
struct AwaitedReturn<C> {
    connection: C,
    question_id: u32,
}

/// Deterministic Level-3 state machine shared by all connections in one vat.
pub struct Level3Router<N: Level3Network, A: Clone> {
    network: N,
    limits: Level3Limits,
    provisions: HashMap<N::Rendezvous, Provision<N::Connection, A>>,
    provision_ids: HashMap<ProvisionId<N::Connection>, N::Rendezvous>,
    pending_accepts: HashMap<AcceptId<N::Connection>, PendingAccept<N::Rendezvous>>,
    pending_by_rendezvous: HashMap<N::Rendezvous, Vec<AcceptId<N::Connection>>>,
    accept_completions: VecDeque<AcceptCompletion<N::Connection, A>>,
    awaited_returns: HashMap<N::Rendezvous, AwaitedReturn<N::Connection>>,
    third_party_answers: HashMap<N::Rendezvous, (N::Connection, u32)>,
    return_routes: HashMap<(N::Connection, u32), ReturnRoute<N::Connection>>,
    new_return_routes: VecDeque<ReturnRoute<N::Connection>>,
}

impl<N: Level3Network, A: Clone> fmt::Debug for Level3Router<N, A> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Level3Router")
            .field("limits", &self.limits)
            .field("provisions", &self.provisions.len())
            .field("pending_accepts", &self.pending_accepts.len())
            .field("return_routes", &self.return_routes.len())
            .finish_non_exhaustive()
    }
}

impl<N: Level3Network, A: Clone> Level3Router<N, A> {
    pub fn new(network: N, limits: Level3Limits) -> Self {
        Self {
            network,
            limits,
            provisions: HashMap::new(),
            provision_ids: HashMap::new(),
            pending_accepts: HashMap::new(),
            pending_by_rendezvous: HashMap::new(),
            accept_completions: VecDeque::new(),
            awaited_returns: HashMap::new(),
            third_party_answers: HashMap::new(),
            return_routes: HashMap::new(),
            new_return_routes: VecDeque::new(),
        }
    }

    pub fn network(&self) -> &N {
        &self.network
    }

    pub fn network_mut(&mut self) -> &mut N {
        &mut self.network
    }

    pub fn authenticated_peer(
        &self,
        connection: &N::Connection,
    ) -> Result<AuthenticatedVatId<N::VatId>, Level3Error<N::Error>> {
        self.network
            .authenticated_peer(connection)
            .map_err(Level3Error::Network)
    }

    /// Plans a direct handoff, falling back to the existing proxy vine when
    /// the network cannot introduce these authenticated connections.
    pub fn plan_introduction(
        &mut self,
        provider: &N::Connection,
        recipient: &N::Connection,
    ) -> Result<HandoffPlan<N::Connection>, Level3Error<N::Error>> {
        self.authenticated_peer(provider)?;
        self.authenticated_peer(recipient)?;
        if !self.network.can_introduce(provider, recipient) {
            return Ok(HandoffPlan::Proxy);
        }
        let Ok(introduction) = self.network.introduce(provider, recipient) else {
            return Ok(HandoffPlan::Proxy);
        };
        Ok(HandoffPlan::Introduce {
            provider: provider.clone(),
            recipient: recipient.clone(),
            introduction,
        })
    }

    /// Translates an existing contact token for a fourth party when the
    /// authenticated network permits forwarding. Otherwise the caller must
    /// accept first and begin another introduction or retain the vine.
    pub fn plan_forward(
        &mut self,
        source: &N::Connection,
        contact: &ThirdPartyToContact,
        destination: &N::Connection,
    ) -> Result<HandoffPlan<N::Connection>, Level3Error<N::Error>> {
        self.authenticated_peer(source)?;
        self.authenticated_peer(destination)?;
        if !self.network.can_forward(source, contact, destination) {
            return Ok(HandoffPlan::Proxy);
        }
        let Ok(contact) = self.network.forward(source, contact, destination) else {
            return Ok(HandoffPlan::Proxy);
        };
        Ok(HandoffPlan::Forward {
            destination: destination.clone(),
            contact,
        })
    }

    pub fn connect_to_introduced(
        &mut self,
        source: &N::Connection,
        contact: &ThirdPartyToContact,
    ) -> Result<IntroducedConnection<N::Connection>, Level3Error<N::Error>> {
        self.authenticated_peer(source)?;
        self.network
            .connect_to_introduced(source, contact)
            .map_err(Level3Error::Network)
    }

    /// Registers an incoming `Provide`. The caller resolves its target through
    /// the connection actor and supplies exactly that capability value.
    pub fn provide(
        &mut self,
        connection: &N::Connection,
        question_id: u32,
        recipient: &ThirdPartyToAwait,
        capability: A,
    ) -> Result<ProvisionId<N::Connection>, Level3Error<N::Error>> {
        if self.provisions.len() >= self.limits.max_provisions {
            return Err(Level3Error::Limit {
                resource: "provision",
                limit: self.limits.max_provisions,
            });
        }
        let id = ProvisionId {
            connection: connection.clone(),
            question_id,
        };
        if self.provision_ids.contains_key(&id) {
            return Err(Level3Error::DuplicateProvision);
        }
        let rendezvous = self
            .network
            .await_rendezvous(connection, recipient)
            .map_err(Level3Error::Network)?;
        if self.provisions.contains_key(&rendezvous) {
            return Err(Level3Error::DuplicateProvision);
        }
        let pending_embargoes = self
            .pending_by_rendezvous
            .get(&rendezvous)
            .into_iter()
            .flatten()
            .filter_map(|accept| self.pending_accepts.get(accept))
            .filter(|accept| !accept.embargo.is_empty())
            .map(|accept| &accept.embargo)
            .collect::<std::collections::HashSet<_>>();
        if pending_embargoes.len() > self.limits.max_embargoes_per_provision {
            return Err(Level3Error::Limit {
                resource: "embargo",
                limit: self.limits.max_embargoes_per_provision,
            });
        }
        self.provisions.insert(
            rendezvous.clone(),
            Provision {
                capability,
                embargoes: HashMap::new(),
            },
        );
        self.provision_ids.insert(id.clone(), rendezvous.clone());
        let pending = self
            .pending_by_rendezvous
            .remove(&rendezvous)
            .unwrap_or_default();
        for accept in pending {
            self.try_complete_accept(&accept)?;
        }
        Ok(id)
    }

    /// Registers an incoming `Accept`. Completion may occur immediately, after
    /// a matching `Provide`, or after the matching third-party disembargo.
    pub fn accept(
        &mut self,
        connection: &N::Connection,
        message: &AcceptMessage,
    ) -> Result<AcceptState, Level3Error<N::Error>> {
        if self.pending_accepts.len() >= self.limits.max_pending_accepts {
            return Err(Level3Error::Limit {
                resource: "pending accept",
                limit: self.limits.max_pending_accepts,
            });
        }
        let id = AcceptId {
            connection: connection.clone(),
            question_id: message.question_id,
        };
        if self.pending_accepts.contains_key(&id) {
            return Err(Level3Error::DuplicateAccept);
        }
        let rendezvous = self
            .network
            .completion_rendezvous(connection, &message.provision)
            .map_err(Level3Error::Network)?;
        self.pending_accepts.insert(
            id.clone(),
            PendingAccept {
                rendezvous: rendezvous.clone(),
                embargo: message.embargo.clone(),
            },
        );
        match self.try_complete_accept(&id) {
            Ok(true) => Ok(AcceptState::Ready),
            Ok(false) => {
                self.pending_by_rendezvous
                    .entry(rendezvous)
                    .or_default()
                    .push(id);
                Ok(AcceptState::Pending)
            }
            Err(error) => {
                self.pending_accepts.remove(&id);
                Err(error)
            }
        }
    }

    pub fn next_accept_completion(&mut self) -> Option<AcceptCompletion<N::Connection, A>> {
        self.accept_completions.pop_front()
    }

    /// Finishes a provision when its vine is released. Pending accepts fail
    /// closed instead of gaining a capability after the provider withdrew it.
    pub fn finish_provision(
        &mut self,
        id: &ProvisionId<N::Connection>,
    ) -> Result<(), Level3Error<N::Error>> {
        let rendezvous = self
            .provision_ids
            .remove(id)
            .ok_or(Level3Error::UnknownProvision)?;
        self.provisions
            .remove(&rendezvous)
            .ok_or(Level3Error::UnknownProvision)?;
        let accepts = self
            .pending_accepts
            .iter()
            .filter(|(_accept, pending)| pending.rendezvous == rendezvous)
            .map(|(accept, _pending)| accept.clone())
            .collect::<Vec<_>>();
        for accept in accepts {
            self.pending_accepts.remove(&accept);
            self.accept_completions.push_back(AcceptCompletion {
                accept,
                result: Err(AcceptFailure::ProvisionFinished),
            });
        }
        self.pending_by_rendezvous.remove(&rendezvous);
        Ok(())
    }

    pub fn generate_embargo_id(
        &mut self,
        id: &ProvisionId<N::Connection>,
    ) -> Result<Vec<u8>, Level3Error<N::Error>> {
        let rendezvous = self
            .provision_ids
            .get(id)
            .ok_or(Level3Error::UnknownProvision)?;
        self.network
            .generate_embargo_id(rendezvous)
            .map_err(Level3Error::Network)
    }

    /// Applies a third-party `Disembargo`. A promised-answer target releases
    /// the matching local provision; an imported-cap target must continue
    /// through the vine and is returned as an explicit forwarding action.
    pub fn disembargo(
        &mut self,
        connection: &N::Connection,
        message: &DisembargoMessage,
    ) -> Result<DisembargoAction, Level3Error<N::Error>> {
        let DisembargoContext::Accept(embargo) = &message.context else {
            return Err(Level3Error::InvalidDisembargoTarget);
        };
        match &message.target {
            CallTarget::PromisedAnswer(PromisedAnswer {
                question_id,
                transform,
            }) if transform.is_empty() => {
                let id = ProvisionId {
                    connection: connection.clone(),
                    question_id: *question_id,
                };
                self.release_embargo(&id, embargo)?;
                Ok(DisembargoAction::Released)
            }
            CallTarget::BootstrapAnswer(question_id) => {
                let id = ProvisionId {
                    connection: connection.clone(),
                    question_id: *question_id,
                };
                self.release_embargo(&id, embargo)?;
                Ok(DisembargoAction::Released)
            }
            CallTarget::ImportedCap(vine_id) => Ok(DisembargoAction::ForwardVine {
                vine_id: *vine_id,
                embargo: embargo.clone(),
            }),
            _ => Err(Level3Error::InvalidDisembargoTarget),
        }
    }

    pub fn release_embargo(
        &mut self,
        id: &ProvisionId<N::Connection>,
        embargo_id: &[u8],
    ) -> Result<(), Level3Error<N::Error>> {
        let rendezvous = self
            .provision_ids
            .get(id)
            .cloned()
            .ok_or(Level3Error::UnknownProvision)?;
        let waiting = {
            let provision = self
                .provisions
                .get_mut(&rendezvous)
                .ok_or(Level3Error::UnknownProvision)?;
            match provision.embargoes.remove(embargo_id) {
                Some(Embargo::Waiting(waiting)) => {
                    provision
                        .embargoes
                        .insert(embargo_id.to_vec(), Embargo::Released);
                    waiting
                }
                Some(Embargo::Released) => {
                    provision
                        .embargoes
                        .insert(embargo_id.to_vec(), Embargo::Released);
                    Vec::new()
                }
                None => {
                    if provision.embargoes.len() >= self.limits.max_embargoes_per_provision {
                        return Err(Level3Error::Limit {
                            resource: "embargo",
                            limit: self.limits.max_embargoes_per_provision,
                        });
                    }
                    provision
                        .embargoes
                        .insert(embargo_id.to_vec(), Embargo::Released);
                    Vec::new()
                }
            }
        };
        for accept in waiting {
            self.try_complete_accept(&accept)?;
        }
        Ok(())
    }

    /// Registers the `awaitFromThirdParty` half of direct return routing. The
    /// matching `ThirdPartyAnswer` may arrive before or after this call.
    pub fn await_return(
        &mut self,
        connection: &N::Connection,
        question_id: u32,
        token: &ThirdPartyToAwait,
    ) -> Result<(), Level3Error<N::Error>> {
        let rendezvous = self
            .network
            .await_rendezvous(connection, token)
            .map_err(Level3Error::Network)?;
        if self.awaited_returns.contains_key(&rendezvous) {
            return Err(Level3Error::DuplicateReturnAwait);
        }
        if !self.third_party_answers.contains_key(&rendezvous)
            && self.active_return_rendezvous() >= self.limits.max_return_routes
        {
            return Err(Level3Error::Limit {
                resource: "return route",
                limit: self.limits.max_return_routes,
            });
        }
        self.awaited_returns.insert(
            rendezvous.clone(),
            AwaitedReturn {
                connection: connection.clone(),
                question_id,
            },
        );
        self.try_adopt_return(&rendezvous)
    }

    pub fn third_party_answer(
        &mut self,
        connection: &N::Connection,
        message: &ThirdPartyAnswerMessage,
    ) -> Result<(), Level3Error<N::Error>> {
        if !(1_u32 << 30..1_u32 << 31).contains(&message.answer_id) {
            return Err(Level3Error::InvalidThirdPartyAnswerId(message.answer_id));
        }
        if self
            .return_routes
            .contains_key(&(connection.clone(), message.answer_id))
            || self
                .third_party_answers
                .values()
                .any(|(existing_connection, existing_id)| {
                    existing_connection == connection && *existing_id == message.answer_id
                })
        {
            return Err(Level3Error::DuplicateThirdPartyAnswer);
        }
        let rendezvous = self
            .network
            .completion_rendezvous(connection, &message.completion)
            .map_err(Level3Error::Network)?;
        if self.third_party_answers.contains_key(&rendezvous) {
            return Err(Level3Error::DuplicateThirdPartyAnswer);
        }
        if !self.awaited_returns.contains_key(&rendezvous)
            && self.active_return_rendezvous() >= self.limits.max_return_routes
        {
            return Err(Level3Error::Limit {
                resource: "return route",
                limit: self.limits.max_return_routes,
            });
        }
        self.third_party_answers
            .insert(rendezvous.clone(), (connection.clone(), message.answer_id));
        self.try_adopt_return(&rendezvous)
    }

    pub fn next_return_route(&mut self) -> Option<ReturnRoute<N::Connection>> {
        self.new_return_routes.pop_front()
    }

    pub fn return_route(
        &self,
        direct_connection: &N::Connection,
        answer_id: u32,
    ) -> Option<&ReturnRoute<N::Connection>> {
        self.return_routes
            .get(&(direct_connection.clone(), answer_id))
    }

    pub fn finish_return_route(
        &mut self,
        direct_connection: &N::Connection,
        answer_id: u32,
    ) -> Result<ReturnRoute<N::Connection>, Level3Error<N::Error>> {
        self.return_routes
            .remove(&(direct_connection.clone(), answer_id))
            .ok_or(Level3Error::UnknownReturnRoute)
    }

    pub fn provision_count(&self) -> usize {
        self.provisions.len()
    }

    pub fn pending_accept_count(&self) -> usize {
        self.pending_accepts.len()
    }

    pub fn return_route_count(&self) -> usize {
        self.return_routes.len()
    }

    fn active_return_rendezvous(&self) -> usize {
        self.awaited_returns.len() + self.third_party_answers.len() + self.return_routes.len()
    }

    fn try_complete_accept(
        &mut self,
        id: &AcceptId<N::Connection>,
    ) -> Result<bool, Level3Error<N::Error>> {
        let pending = self
            .pending_accepts
            .get(id)
            .cloned()
            .ok_or(Level3Error::UnknownAccept)?;
        let Some(provision) = self.provisions.get_mut(&pending.rendezvous) else {
            return Ok(false);
        };
        if !pending.embargo.is_empty() {
            match provision.embargoes.get_mut(&pending.embargo) {
                Some(Embargo::Released) => {}
                Some(Embargo::Waiting(waiting)) => {
                    if !waiting.contains(id) {
                        waiting.push(id.clone());
                    }
                    return Ok(false);
                }
                None => {
                    if provision.embargoes.len() >= self.limits.max_embargoes_per_provision {
                        return Err(Level3Error::Limit {
                            resource: "embargo",
                            limit: self.limits.max_embargoes_per_provision,
                        });
                    }
                    provision
                        .embargoes
                        .insert(pending.embargo, Embargo::Waiting(vec![id.clone()]));
                    return Ok(false);
                }
            }
        }
        let capability = provision.capability.clone();
        self.pending_accepts.remove(id);
        self.accept_completions.push_back(AcceptCompletion {
            accept: id.clone(),
            result: Ok(capability),
        });
        Ok(true)
    }

    fn try_adopt_return(
        &mut self,
        rendezvous: &N::Rendezvous,
    ) -> Result<(), Level3Error<N::Error>> {
        let (Some(awaited), Some((direct_connection, answer_id))) = (
            self.awaited_returns.get(rendezvous),
            self.third_party_answers.get(rendezvous),
        ) else {
            return Ok(());
        };
        let key = (direct_connection.clone(), *answer_id);
        if self.return_routes.contains_key(&key) {
            return Err(Level3Error::DuplicateThirdPartyAnswer);
        }
        let route = ReturnRoute {
            original_connection: awaited.connection.clone(),
            original_question_id: awaited.question_id,
            direct_connection: direct_connection.clone(),
            adopted_answer_id: *answer_id,
        };
        self.awaited_returns.remove(rendezvous);
        self.third_party_answers.remove(rendezvous);
        self.return_routes.insert(key, route.clone());
        self.new_return_routes.push_back(route);
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use std::error::Error;

    use capnp_message::{ExclusiveArena, ReaderLimits};
    use capnp_schema::OpaquePointer;

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TestError {
        UnknownConnection,
        ForgedToken,
        IntroductionDenied,
        ForwardDenied,
    }

    impl fmt::Display for TestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "{self:?}")
        }
    }

    impl Error for TestError {}

    #[derive(Clone, Debug)]
    struct Contact {
        token: ThirdPartyToContact,
        source: u8,
        target: u8,
        rendezvous: u64,
    }

    #[derive(Clone, Debug)]
    struct Await {
        token: ThirdPartyToAwait,
        source: u8,
        rendezvous: u64,
    }

    #[derive(Clone, Debug)]
    struct Completion {
        token: ThirdPartyCompletion,
        source: u8,
        rendezvous: u64,
    }

    #[derive(Debug)]
    struct TestNetwork {
        peers: HashMap<u8, &'static str>,
        introductions: bool,
        forwarding: bool,
        fail_introduction: bool,
        fail_forward: bool,
        next_token: u64,
        contacts: Vec<Contact>,
        awaits: Vec<Await>,
        completions: Vec<Completion>,
        introduce_count: usize,
        connect_count: usize,
        forward_count: usize,
        embargo_count: u64,
    }

    impl TestNetwork {
        fn new() -> Self {
            Self {
                peers: HashMap::from([
                    (1, "bob"),
                    (2, "carol"),
                    (3, "alice"),
                    (4, "dave"),
                    (5, "eve"),
                ]),
                introductions: true,
                forwarding: true,
                fail_introduction: false,
                fail_forward: false,
                next_token: 1,
                contacts: Vec::new(),
                awaits: Vec::new(),
                completions: Vec::new(),
                introduce_count: 0,
                connect_count: 0,
                forward_count: 0,
                embargo_count: 0,
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

        fn contact(&self, source: u8, token: &ThirdPartyToContact) -> Option<&Contact> {
            self.contacts
                .iter()
                .find(|entry| entry.source == source && entry.token == *token)
        }
    }

    impl Level3Network for TestNetwork {
        type Connection = u8;
        type VatId = &'static str;
        type Rendezvous = u64;
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

        fn can_introduce(
            &self,
            _provider: &Self::Connection,
            _recipient: &Self::Connection,
        ) -> bool {
            self.introductions
        }

        fn introduce(
            &mut self,
            provider: &Self::Connection,
            recipient: &Self::Connection,
        ) -> Result<Introduction, Self::Error> {
            if !self.introductions {
                return Err(TestError::IntroductionDenied);
            }
            if self.fail_introduction {
                return Err(TestError::IntroductionDenied);
            }
            self.authenticated_peer(provider)?;
            self.authenticated_peer(recipient)?;
            let rendezvous = self.next_token;
            let contact = ThirdPartyToContact::from_opaque(self.pointer());
            let await_token = ThirdPartyToAwait::from_opaque(self.pointer());
            self.contacts.push(Contact {
                token: contact.clone(),
                source: *recipient,
                target: *provider,
                rendezvous,
            });
            self.awaits.push(Await {
                token: await_token.clone(),
                source: *provider,
                rendezvous,
            });
            self.introduce_count += 1;
            Ok(Introduction {
                contact,
                recipient: await_token,
            })
        }

        fn connect_to_introduced(
            &mut self,
            source: &Self::Connection,
            contact: &ThirdPartyToContact,
        ) -> Result<IntroducedConnection<Self::Connection>, Self::Error> {
            let entry = self
                .contact(*source, contact)
                .cloned()
                .ok_or(TestError::ForgedToken)?;
            let completion = ThirdPartyCompletion::from_opaque(self.pointer());
            self.completions.push(Completion {
                token: completion.clone(),
                source: entry.target,
                rendezvous: entry.rendezvous,
            });
            self.connect_count += 1;
            let connection = (self.peers[&entry.target] != "alice").then_some(entry.target);
            Ok(IntroducedConnection {
                connection,
                completion,
            })
        }

        fn can_forward(
            &self,
            source: &Self::Connection,
            contact: &ThirdPartyToContact,
            _destination: &Self::Connection,
        ) -> bool {
            self.forwarding && self.contact(*source, contact).is_some()
        }

        fn forward(
            &mut self,
            source: &Self::Connection,
            contact: &ThirdPartyToContact,
            destination: &Self::Connection,
        ) -> Result<ThirdPartyToContact, Self::Error> {
            if !self.forwarding {
                return Err(TestError::ForwardDenied);
            }
            if self.fail_forward {
                return Err(TestError::ForwardDenied);
            }
            let entry = self
                .contact(*source, contact)
                .cloned()
                .ok_or(TestError::ForgedToken)?;
            let token = ThirdPartyToContact::from_opaque(self.pointer());
            self.contacts.push(Contact {
                token: token.clone(),
                source: *destination,
                target: entry.target,
                rendezvous: entry.rendezvous,
            });
            self.forward_count += 1;
            Ok(token)
        }

        fn await_rendezvous(
            &self,
            source: &Self::Connection,
            token: &ThirdPartyToAwait,
        ) -> Result<Self::Rendezvous, Self::Error> {
            self.awaits
                .iter()
                .find(|entry| entry.source == *source && entry.token == *token)
                .map(|entry| entry.rendezvous)
                .ok_or(TestError::ForgedToken)
        }

        fn completion_rendezvous(
            &self,
            source: &Self::Connection,
            token: &ThirdPartyCompletion,
        ) -> Result<Self::Rendezvous, Self::Error> {
            self.completions
                .iter()
                .find(|entry| entry.source == *source && entry.token == *token)
                .map(|entry| entry.rendezvous)
                .ok_or(TestError::ForgedToken)
        }

        fn generate_embargo_id(
            &mut self,
            provision: &Self::Rendezvous,
        ) -> Result<Vec<u8>, Self::Error> {
            self.embargo_count += 1;
            let mut id = provision.to_le_bytes().to_vec();
            id.extend_from_slice(&self.embargo_count.to_le_bytes());
            Ok(id)
        }
    }

    fn introduction(plan: HandoffPlan<u8>) -> Introduction {
        let HandoffPlan::Introduce { introduction, .. } = plan else {
            panic!("introduction")
        };
        introduction
    }

    #[test]
    fn basic_three_party_handoff_accepts_lazily_and_releases_the_vine() {
        let mut router = Level3Router::<_, u64>::new(TestNetwork::new(), Level3Limits::default());
        let introduction = introduction(router.plan_introduction(&1, &2).expect("introduction"));
        let provision = router
            .provide(&1, 10, &introduction.recipient, 77)
            .expect("provide");
        assert_eq!(router.provision_count(), 1);
        assert_eq!(router.network().connect_count, 0);

        let connected = router
            .connect_to_introduced(&2, &introduction.contact)
            .expect("lazy connection");
        assert_eq!(connected.connection, Some(1));
        assert_eq!(router.network().connect_count, 1);
        assert_eq!(
            router
                .accept(
                    &1,
                    &AcceptMessage {
                        question_id: 20,
                        provision: connected.completion,
                        embargo: Vec::new(),
                    },
                )
                .expect("accept"),
            AcceptState::Ready
        );
        assert_eq!(
            router.next_accept_completion(),
            Some(AcceptCompletion {
                accept: AcceptId {
                    connection: 1,
                    question_id: 20,
                },
                result: Ok(77),
            })
        );
        router.finish_provision(&provision).expect("finish vine");
        assert_eq!(router.provision_count(), 0);
    }

    #[test]
    fn accept_before_provide_rendezvouses_without_granting_early_authority() {
        let mut router = Level3Router::<_, u64>::new(TestNetwork::new(), Level3Limits::default());
        let introduction = introduction(router.plan_introduction(&1, &2).expect("introduction"));
        let connected = router
            .connect_to_introduced(&2, &introduction.contact)
            .expect("connect");
        assert_eq!(
            router
                .accept(
                    &1,
                    &AcceptMessage {
                        question_id: 21,
                        provision: connected.completion,
                        embargo: Vec::new(),
                    },
                )
                .expect("pending accept"),
            AcceptState::Pending
        );
        assert!(router.next_accept_completion().is_none());
        router
            .provide(&1, 11, &introduction.recipient, 88)
            .expect("late provide");
        assert_eq!(
            router.next_accept_completion().expect("completed").result,
            Ok(88)
        );
    }

    #[test]
    fn introduce_to_self_and_embargo_hold_calls_until_the_vine_path_clears() {
        let mut router = Level3Router::<_, u64>::new(TestNetwork::new(), Level3Limits::default());
        let introduction = introduction(router.plan_introduction(&3, &2).expect("introduction"));
        let provision = router
            .provide(&3, 30, &introduction.recipient, 99)
            .expect("provide");
        let embargo = router.generate_embargo_id(&provision).expect("embargo ID");
        let connected = router
            .connect_to_introduced(&2, &introduction.contact)
            .expect("connect to self");
        assert_eq!(connected.connection, None);
        assert_eq!(
            router
                .accept(
                    &3,
                    &AcceptMessage {
                        question_id: 31,
                        provision: connected.completion,
                        embargo: embargo.clone(),
                    },
                )
                .expect("accept"),
            AcceptState::Pending
        );
        assert!(router.next_accept_completion().is_none());
        assert_eq!(
            router
                .disembargo(
                    &3,
                    &DisembargoMessage {
                        target: CallTarget::BootstrapAnswer(30),
                        context: DisembargoContext::Accept(embargo),
                    },
                )
                .expect("disembargo"),
            DisembargoAction::Released
        );
        assert_eq!(
            router.next_accept_completion().expect("released").result,
            Ok(99)
        );
    }

    #[test]
    fn forwarding_translates_tokens_but_forgery_and_disabled_routes_keep_the_proxy() {
        let mut router = Level3Router::<_, u64>::new(TestNetwork::new(), Level3Limits::default());
        let introduction = introduction(router.plan_introduction(&1, &2).expect("introduction"));
        let forwarded = router
            .plan_forward(&2, &introduction.contact, &4)
            .expect("forward");
        let HandoffPlan::Forward {
            destination,
            contact,
        } = forwarded
        else {
            panic!("forward plan")
        };
        assert_eq!(destination, 4);
        assert_eq!(router.network().forward_count, 1);
        assert!(router.connect_to_introduced(&4, &contact).is_ok());
        assert!(matches!(
            router.connect_to_introduced(&4, &introduction.contact),
            Err(Level3Error::Network(TestError::ForgedToken))
        ));

        router.network_mut().forwarding = false;
        assert_eq!(
            router
                .plan_forward(&2, &introduction.contact, &5)
                .expect("proxy fallback"),
            HandoffPlan::Proxy
        );
        assert_eq!(router.network().forward_count, 1);
        router.network_mut().forwarding = true;
        router.network_mut().fail_forward = true;
        assert_eq!(
            router
                .plan_forward(&2, &introduction.contact, &5)
                .expect("failed forward fallback"),
            HandoffPlan::Proxy
        );
        router.network_mut().fail_forward = false;
        router.network_mut().introductions = false;
        assert_eq!(
            router
                .plan_introduction(&1, &5)
                .expect("introduction fallback"),
            HandoffPlan::Proxy
        );
        router.network_mut().introductions = true;
        router.network_mut().fail_introduction = true;
        assert_eq!(
            router
                .plan_introduction(&1, &5)
                .expect("failed introduction fallback"),
            HandoffPlan::Proxy
        );
    }

    #[test]
    fn reflected_forwarding_keeps_one_rendezvous_across_every_translation() {
        let mut router = Level3Router::<_, u64>::new(TestNetwork::new(), Level3Limits::default());
        let introduction = introduction(router.plan_introduction(&1, &2).expect("introduction"));
        let HandoffPlan::Forward { contact, .. } = router
            .plan_forward(&2, &introduction.contact, &4)
            .expect("first forward")
        else {
            panic!("first forward")
        };
        let HandoffPlan::Forward { contact, .. } =
            router.plan_forward(&4, &contact, &5).expect("reflection")
        else {
            panic!("reflection")
        };
        let HandoffPlan::Forward { contact, .. } = router
            .plan_forward(&5, &contact, &4)
            .expect("reflected return")
        else {
            panic!("reflected return")
        };
        let connected = router
            .connect_to_introduced(&4, &contact)
            .expect("connect after reflection");
        router
            .provide(&1, 40, &introduction.recipient, 123)
            .expect("provide");
        assert_eq!(
            router
                .accept(
                    &1,
                    &AcceptMessage {
                        question_id: 41,
                        provision: connected.completion,
                        embargo: Vec::new(),
                    },
                )
                .expect("accept"),
            AcceptState::Ready
        );
        assert_eq!(
            router.next_accept_completion().expect("completion").result,
            Ok(123)
        );
        assert_eq!(router.network().forward_count, 3);
    }

    #[test]
    fn third_party_return_routing_matches_both_arrival_orders_and_releases_exactly() {
        let mut router = Level3Router::<_, u64>::new(TestNetwork::new(), Level3Limits::default());
        for (index, answer_first) in [false, true].into_iter().enumerate() {
            let introduction =
                introduction(router.plan_introduction(&1, &2).expect("introduction"));
            let connected = router
                .connect_to_introduced(&2, &introduction.contact)
                .expect("connect");
            let answer = ThirdPartyAnswerMessage {
                completion: connected.completion,
                answer_id: (1_u32 << 30) + u32::try_from(index).expect("small index"),
            };
            if answer_first {
                router
                    .third_party_answer(&1, &answer)
                    .expect("answer first");
                router
                    .await_return(
                        &1,
                        50 + u32::try_from(index).expect("small index"),
                        &introduction.recipient,
                    )
                    .expect("await second");
            } else {
                router
                    .await_return(
                        &1,
                        50 + u32::try_from(index).expect("small index"),
                        &introduction.recipient,
                    )
                    .expect("await first");
                router
                    .third_party_answer(&1, &answer)
                    .expect("answer second");
            }
            let route = router.next_return_route().expect("adopted route");
            assert_eq!(route.original_question_id, 50 + index as u32);
            assert_eq!(route.adopted_answer_id, answer.answer_id);
            assert_eq!(router.return_route(&1, answer.answer_id), Some(&route));
            assert_eq!(
                router
                    .finish_return_route(&1, answer.answer_id)
                    .expect("finish route"),
                route
            );
        }
        assert_eq!(router.return_route_count(), 0);
    }

    #[test]
    fn finishing_a_vine_fails_waiting_accepts_and_limits_are_transactional() {
        let limits = Level3Limits {
            max_provisions: 1,
            max_pending_accepts: 1,
            max_embargoes_per_provision: 1,
            max_return_routes: 1,
        };
        let mut router = Level3Router::<_, u64>::new(TestNetwork::new(), limits);
        let first = introduction(router.plan_introduction(&1, &2).expect("first intro"));
        let connected = router
            .connect_to_introduced(&2, &first.contact)
            .expect("connect");
        assert_eq!(
            router
                .accept(
                    &1,
                    &AcceptMessage {
                        question_id: 60,
                        provision: connected.completion,
                        embargo: Vec::new(),
                    },
                )
                .expect("waiting accept"),
            AcceptState::Pending
        );
        let provision = router
            .provide(&1, 61, &first.recipient, 456)
            .expect("provide");
        assert_eq!(
            router.next_accept_completion().expect("ready").result,
            Ok(456)
        );

        let second = introduction(router.plan_introduction(&1, &4).expect("second intro"));
        assert!(matches!(
            router.provide(&1, 62, &second.recipient, 789),
            Err(Level3Error::Limit {
                resource: "provision",
                limit: 1,
            })
        ));
        assert_eq!(router.provision_count(), 1);
        router.finish_provision(&provision).expect("finish");
        assert_eq!(router.provision_count(), 0);

        let connected = router
            .connect_to_introduced(&4, &second.contact)
            .expect("second connect");
        let provision = router
            .provide(&1, 64, &second.recipient, 789)
            .expect("second provide");
        assert_eq!(
            router
                .accept(
                    &1,
                    &AcceptMessage {
                        question_id: 63,
                        provision: connected.completion,
                        embargo: vec![1],
                    },
                )
                .expect("waiting accept"),
            AcceptState::Pending
        );
        router.finish_provision(&provision).expect("finish second");
        assert_eq!(
            router.next_accept_completion(),
            Some(AcceptCompletion {
                accept: AcceptId {
                    connection: 1,
                    question_id: 63,
                },
                result: Err(AcceptFailure::ProvisionFinished),
            })
        );
    }
}
