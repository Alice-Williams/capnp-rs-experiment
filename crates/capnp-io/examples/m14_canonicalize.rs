use std::io::{self, Read, Write};

use capnp_io::{FrameLimits, FrameRead, parse_frame};
use capnp_message::{LocalTraversalBudget, MessageSegments, NestingLimit, canonicalize};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    let FrameRead::Message { frame, remaining } = parse_frame(&input, FrameLimits::default())?
    else {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "expected one message").into());
    };
    if !remaining.is_empty() {
        return Err(
            io::Error::new(io::ErrorKind::InvalidData, "expected exactly one message").into(),
        );
    }
    let segments = frame
        .segments()
        .iter()
        .map(|segment| segment.bytes())
        .collect::<Vec<_>>();
    let message = MessageSegments::new(&segments)?;
    let canonical = canonicalize(
        &message,
        &LocalTraversalBudget::new(FrameLimits::default().max_total_words),
        NestingLimit::new(64),
        u32::try_from(FrameLimits::default().max_total_words)?,
    )?;
    io::stdout().write_all(&canonical)?;
    Ok(())
}
