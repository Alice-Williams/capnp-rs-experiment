use std::error::Error;
use std::io::{self, Write};

use capnp_generated_fixture::wire::{Color, wire_fixture};
use capnp_io::{FrameLimits, encode_frame};
use capnp_message::ExclusiveArena;
use capnp_schema::{CompiledSchema, LoadLimits};

const REQUEST: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../conformance/fixtures/cpp/",
    "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
    "compiler-request-wire-fixture.bin"
));

fn main() -> Result<(), Box<dyn Error>> {
    let schema = CompiledSchema::from_code_generator_request(REQUEST, LoadLimits::default())?;
    let mut arena = ExclusiveArena::new(32, 1024)?;
    {
        let mut root = wire_fixture::Builder::init_root(&schema, &mut arena)?;
        root.set_uint32_value(77)?;
        root.set_color(Color::Unrecognized(99))?;
        root.set_text("native generated")?;
        {
            let mut values = root.init_uint16s(3)?;
            values.set(0, capnp_schema::DynamicInput::UInt16(2))?;
            values.set(1, capnp_schema::DynamicInput::UInt16(3))?;
            values.set(2, capnp_schema::DynamicInput::UInt16(5))?;
        }
        root.choice()?.set_number(444)?;
        root.init_node()?.set_value(88)?;
    }
    let segments = arena.into_segments();
    let borrowed = segments.iter().map(AsRef::as_ref).collect::<Vec<_>>();
    let frame = encode_frame(&borrowed, FrameLimits::default())?;
    io::stdout().write_all(&frame)?;
    Ok(())
}
