use std::io::{self, Write};

use capnp_io::{FrameLimits, encode_frame};
use capnp_message::ExclusiveArena;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arena = ExclusiveArena::new_segmented(2, 2, 32, 128)?;
    {
        let mut root = arena.init_root_struct(0, 4)?;
        let root_offset = root.offset();
        {
            let mut child = root.init_struct(0, 1, 1)?;
            child.set_u32(0, 4242, 0)?;
            child.set_text(0, "moved without copying")?;
        }
        {
            let mut values = root.init_list::<u16>(2, 3)?;
            values.set(0, 13)?;
            values.set(1, 21)?;
            values.set(2, 34)?;
        }
        root.disown_struct(0)?.adopt_into_struct(root_offset, 1)?;
        root.disown_list(2)?.adopt_into_struct(root_offset, 3)?;
    }
    let segments = arena.segments().collect::<Vec<_>>();
    let frame = encode_frame(&segments, FrameLimits::default())?;
    io::stdout().write_all(&frame)?;
    Ok(())
}
