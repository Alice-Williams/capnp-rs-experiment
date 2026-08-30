use std::io::{self, Write};

use capnp_message::ExclusiveArena;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arena = ExclusiveArena::new(8, 128)?;
    {
        let mut root = arena.init_root_struct(1, 7)?;
        root.set_u64(0, 0x0123_4567_89ab_cdef, 0)?;
        root.set_text(0, "native builder")?;
        root.set_data(1, &[0, 1, 2, 0xff])?;
        {
            let mut numbers = root.init_list::<u16>(2, 3)?;
            numbers.set(0, 10)?;
            numbers.set(1, 20)?;
            numbers.set(2, 30)?;
        }
        {
            let mut labels = root.init_pointer_list(3, 2)?;
            labels.set_text(0, "left")?;
            labels.set_text(1, "right")?;
        }
        {
            let mut child = root.init_struct(4, 1, 1)?;
            child.set_u32(0, 7, 0)?;
            child.set_text(0, "only")?;
        }
        {
            let mut children = root.init_struct_list(5, 2, 1, 1)?;
            {
                let mut first = children.get(0)?;
                first.set_u32(0, 11, 0)?;
                first.set_text(0, "first")?;
            }
            {
                let mut second = children.get(1)?;
                second.set_u32(0, 22, 0)?;
                second.set_text(0, "second")?;
            }
        }
        {
            let mut nested = root.init_pointer_list(6, 2)?;
            {
                let mut first = nested.init_list::<u16>(0, 2)?;
                first.set(0, 1)?;
                first.set(1, 2)?;
            }
            {
                let mut second = nested.init_list::<u16>(1, 1)?;
                second.set(0, 3)?;
            }
        }
    }
    io::stdout().write_all(arena.as_segment())?;
    Ok(())
}
