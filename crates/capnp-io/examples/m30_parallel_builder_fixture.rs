use std::io::{self, Write};

use capnp_io::{FrameLimits, encode_frame};
use capnp_message::{ParallelBuildOptions, PartitionedPointerList};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut builder = PartitionedPointerList::new(6, 7, 128)?;
    let lanes = builder.lanes(ParallelBuildOptions {
        requested_workers: 3,
        min_parallel_items: 1,
        min_items_per_partition: 1,
    })?;
    let sealed = std::thread::scope(|scope| {
        lanes
            .into_iter()
            .map(|mut lane| {
                scope.spawn(move || {
                    for index in lane.range() {
                        lane.build_fragment(index, 16, |arena| {
                            let label = format!("worker-item-{index}");
                            let count = u32::try_from(label.len() + 1)
                                .map_err(|_| capnp_message::ArenaError::AllocationOverflow)?;
                            let mut text = arena.init_root_list::<u8>(count)?;
                            for (offset, byte) in label.bytes().enumerate() {
                                text.set(
                                    u32::try_from(offset).map_err(|_| {
                                        capnp_message::ArenaError::AllocationOverflow
                                    })?,
                                    byte,
                                )?;
                            }
                            Ok(())
                        })?;
                    }
                    Ok::<_, capnp_message::ParallelBuildError>(lane.seal())
                })
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| io::Error::other("parallel builder worker panicked"))?
                    .map_err(io::Error::other)
            })
            .collect::<Result<Vec<_>, io::Error>>()
    })?;
    let segments = builder.finish(sealed.into_iter().rev())?;
    let borrowed = segments.iter().map(AsRef::as_ref).collect::<Vec<_>>();
    let frame = encode_frame(&borrowed, FrameLimits::default())?;
    io::stdout().write_all(&frame)?;
    Ok(())
}
