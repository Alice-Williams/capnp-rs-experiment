use std::io::{Read, Write};
use std::sync::Arc;

use capnp_io::{FrameLimits, FrameRead, encode_frame, parse_frame};
use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
use capnp_rpc_core::{
    CallTarget, CapDescriptor, DisembargoContext, DisembargoMessage, PromiseResolution,
    ProtocolLimits, ProtocolMessage, ResolveMessage, encode_call_with_capabilities,
    encode_disembargo, encode_resolve, read_protocol_message,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    let mut decoded = Vec::new();
    let mut remaining = input.as_slice();
    while !remaining.is_empty() {
        let FrameRead::Message {
            frame,
            remaining: next,
        } = parse_frame(remaining, FrameLimits::default())?
        else {
            return Err("truncated M36 fixture".into());
        };
        let message = OwnedMessage::new(
            frame.segments().iter().map(|segment| segment.bytes()),
            ReaderLimits::default(),
        )?;
        decoded.push(read_protocol_message(message)?);
        remaining = next;
    }
    validate(&decoded)?;

    let limits = ProtocolLimits::default();
    let messages = [
        encode_call_with_capabilities(
            10,
            CallTarget::ImportedCap(3),
            0xfeedbeef,
            6,
            &params()?,
            &[CapDescriptor::SenderPromise(4)],
            limits,
        )?,
        encode_resolve(
            4,
            &PromiseResolution::Cap(CapDescriptor::SenderHosted(5)),
            limits,
        )?,
        encode_disembargo(
            &CallTarget::ImportedCap(4),
            DisembargoContext::SenderLoopback(77),
            limits,
        )?,
        encode_disembargo(
            &CallTarget::ImportedCap(4),
            DisembargoContext::ReceiverLoopback(77),
            limits,
        )?,
    ];
    let mut output = Vec::new();
    for message in messages {
        let segments = (0..message.segment_count())
            .map(|index| {
                message
                    .segment(u32::try_from(index).expect("segment index"))
                    .expect("segment exists")
            })
            .collect::<Vec<_>>();
        output.extend_from_slice(&encode_frame(&segments, FrameLimits::default())?);
    }
    std::io::stdout().write_all(&output)?;
    Ok(())
}

fn validate(messages: &[ProtocolMessage]) -> Result<(), Box<dyn std::error::Error>> {
    if messages.len() != 4 {
        return Err(format!("expected four M36 messages, got {}", messages.len()).into());
    }
    let ProtocolMessage::Call(call) = &messages[0] else {
        return Err("fixture message 0 is not Call".into());
    };
    if call.question_id != 10
        || call.target != CallTarget::ImportedCap(3)
        || call.interface_id != 0xfeedbeef
        || call.method_id != 6
        || call.params.cap_table != [CapDescriptor::SenderPromise(4)]
    {
        return Err("fixture Call fields differ".into());
    }
    if !matches!(
        &messages[1],
        ProtocolMessage::Resolve(ResolveMessage {
            promise_id: 4,
            resolution: PromiseResolution::Cap(CapDescriptor::SenderHosted(5)),
        })
    ) {
        return Err("fixture Resolve fields differ".into());
    }
    for (message, expected) in messages[2..].iter().zip([
        DisembargoContext::SenderLoopback(77),
        DisembargoContext::ReceiverLoopback(77),
    ]) {
        let ProtocolMessage::Disembargo(DisembargoMessage { target, context }) = message else {
            return Err("fixture message is not Disembargo".into());
        };
        if *target != CallTarget::ImportedCap(4) || *context != expected {
            return Err("fixture Disembargo fields differ".into());
        }
    }
    Ok(())
}

fn params() -> Result<Arc<OwnedMessage>, Box<dyn std::error::Error>> {
    let mut arena = ExclusiveArena::new(2, 16)?;
    arena.init_root_struct(1, 0)?.set_u64(0, 123, 0)?;
    Ok(OwnedMessage::new(
        arena.into_segments(),
        ReaderLimits::default(),
    )?)
}
