use std::env;
use std::error::Error;
use std::fs;
use std::io::{self, Write};

use capnp_codegen::generate_requested_files;
use capnp_schema::{CompiledSchema, LoadLimits};

fn main() -> Result<(), Box<dyn Error>> {
    let request = env::args_os().nth(1).ok_or("usage: m20_generate REQUEST")?;
    let schema =
        CompiledSchema::from_code_generator_request(&fs::read(request)?, LoadLimits::default())?;
    let generated = generate_requested_files(&schema)?;
    let file = generated.first().ok_or("request has no requested file")?;
    io::stdout().write_all(file.source.as_bytes())?;
    Ok(())
}
