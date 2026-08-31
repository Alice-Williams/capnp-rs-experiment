use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use capnp_io::{FrameLimits, FrameRead, encode_frame, parse_frame};
use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
use capnp_rpc_core::{
    ActorEffect, ActorLimits, CompletionToken, ConnectionActor, HandlerResult, HostedCapability,
    IncomingRequest, OutgoingCapability, ProtocolLimits,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let (handle, mut actor) =
        ConnectionActor::new(ActorLimits::default(), ProtocolLimits::default());
    let mut completions = BTreeMap::new();
    let mut output = Vec::new();
    let mut remaining = input.as_slice();
    while !remaining.is_empty() {
        let FrameRead::Message {
            frame,
            remaining: next,
        } = parse_frame(remaining, FrameLimits::default())?
        else {
            return Err("unexpected end of calculator trace".into());
        };
        let message = OwnedMessage::new(
            frame.segments().iter().map(|segment| segment.bytes()),
            ReaderLimits::default(),
        )?;
        handle.receive(message)?;
        drain(&mut actor, &mut completions, &mut output)?;
        remaining = next;
    }

    let bootstrap = completions.remove(&u16::MAX).ok_or("missing bootstrap")?;
    bootstrap.complete_with_capabilities(
        capability_result(0)?,
        vec![OutgoingCapability::Hosted(HostedCapability::new()?)],
    )?;
    drain(&mut actor, &mut completions, &mut output)?;

    let get_operator = completions.remove(&0).ok_or("missing getOperator")?;
    get_operator.complete_with_capabilities(
        capability_result(0)?,
        vec![OutgoingCapability::Hosted(HostedCapability::new()?)],
    )?;
    drain(&mut actor, &mut completions, &mut output)?;

    let evaluate = completions.remove(&1).ok_or("missing evaluate")?;
    evaluate.complete(HandlerResult::Results(data_result(42)?))?;
    drain(&mut actor, &mut completions, &mut output)?;
    std::io::stdout().write_all(&output)?;
    Ok(())
}

fn drain(
    actor: &mut ConnectionActor,
    completions: &mut BTreeMap<u16, CompletionToken>,
    output: &mut Vec<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    loop {
        let mut context = Context::from_waker(Waker::noop());
        match actor.poll_next_effect(&mut context) {
            Poll::Ready(Some(ActorEffect::Send(message))) => {
                let segments = (0..message.segment_count())
                    .map(|index| {
                        message
                            .segment(u32::try_from(index).expect("segment index"))
                            .expect("segment exists")
                    })
                    .collect::<Vec<_>>();
                output.extend_from_slice(&encode_frame(&segments, FrameLimits::default())?);
            }
            Poll::Ready(Some(ActorEffect::Dispatch {
                request,
                completion,
            })) => {
                let method = match request {
                    IncomingRequest::Bootstrap => u16::MAX,
                    IncomingRequest::Call { method_id, .. } => method_id,
                };
                if completions.insert(method, completion).is_some() {
                    return Err("duplicate calculator method dispatch".into());
                }
            }
            Poll::Ready(Some(ActorEffect::CloseTransport)) => {
                return Err("calculator trace closed transport".into());
            }
            Poll::Ready(None) => return Err("calculator actor terminated".into()),
            Poll::Pending => return Ok(()),
        }
    }
}

fn capability_result(index: u32) -> Result<Arc<OwnedMessage>, Box<dyn std::error::Error>> {
    let mut arena = ExclusiveArena::new(2, 16)?;
    arena.init_root_struct(0, 1)?.set_capability(0, index)?;
    Ok(OwnedMessage::new(
        arena.into_segments(),
        ReaderLimits::default(),
    )?)
}

fn data_result(value: u64) -> Result<Arc<OwnedMessage>, Box<dyn std::error::Error>> {
    let mut arena = ExclusiveArena::new(2, 16)?;
    arena.init_root_struct(1, 0)?.set_u64(0, value, 0)?;
    Ok(OwnedMessage::new(
        arena.into_segments(),
        ReaderLimits::default(),
    )?)
}
