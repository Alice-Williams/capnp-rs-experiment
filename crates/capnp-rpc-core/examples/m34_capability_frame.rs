use std::io::{Read, Write};

use capnp_io::{FrameLimits, FrameRead, encode_frame, parse_frame};
use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
use capnp_rpc_core::{
    CallTarget, CapDescriptor, ProtocolLimits, ProtocolMessage, encode_call_with_capabilities,
    read_protocol_message,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    if !input.is_empty() {
        let FrameRead::Message { frame, remaining } = parse_frame(&input, FrameLimits::default())?
        else {
            return Err("missing frame".into());
        };
        if !remaining.is_empty() {
            return Err("trailing frame bytes".into());
        }
        let message = OwnedMessage::new(
            frame.segments().iter().map(|segment| segment.bytes()),
            ReaderLimits::default(),
        )?;
        let ProtocolMessage::Call(call) = read_protocol_message(message)? else {
            return Err("expected Call".into());
        };
        if call.target != CallTarget::ImportedCap(12)
            || call.interface_id != 0xfeed
            || call.method_id != 5
            || call.params.cap_table
                != [
                    CapDescriptor::SenderHosted(4),
                    CapDescriptor::SenderHosted(4),
                    CapDescriptor::ReceiverHosted(9),
                    CapDescriptor::None,
                ]
        {
            return Err("C++ capability payload mismatch".into());
        }
    }

    let mut arena = ExclusiveArena::new(1, 16)?;
    arena.init_root_struct(0, 0)?;
    let content = OwnedMessage::new(arena.into_segments(), ReaderLimits::default())?;
    let message = encode_call_with_capabilities(
        10,
        CallTarget::ImportedCap(12),
        0xfeed,
        5,
        &content,
        &[
            CapDescriptor::SenderHosted(4),
            CapDescriptor::SenderHosted(4),
            CapDescriptor::ReceiverHosted(9),
            CapDescriptor::None,
        ],
        ProtocolLimits::default(),
    )?;
    let segments = (0..message.segment_count())
        .map(|index| {
            message
                .segment(u32::try_from(index).expect("segment index"))
                .expect("segment exists")
        })
        .collect::<Vec<_>>();
    std::io::stdout().write_all(&encode_frame(&segments, FrameLimits::default())?)?;
    Ok(())
}
