use std::io::{self, Read, Write};

use capnp_io::pack;

const MAX_OUTPUT_BYTES: usize = 64 * 1024 * 1024;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    let output = pack(&input, MAX_OUTPUT_BYTES)?;
    io::stdout().write_all(&output)?;
    Ok(())
}
