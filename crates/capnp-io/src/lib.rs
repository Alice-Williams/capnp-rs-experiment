#![no_std]
#![doc = "Synchronous Cap'n Proto framing, packing, and I/O adapters."]
//!
//! Standard unpacked framing follows `capnp/serialize.h` and
//! `capnp/serialize.c++` from pinned C++ oracle commit
//! `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`: a little-endian segment count
//! minus one, one word count per segment, optional 32-bit padding, then segment
//! bodies in order.
//!
//! M04 provides bounded parsing and encoding of standard frames. M15 adds
//! bounded incremental packed encoding and decoding across arbitrary byte
//! chunks. I/O adapters, pointer traversal, and schema checks are separate.

mod framing;
#[cfg(feature = "alloc")]
mod packed;
#[cfg(feature = "std")]
mod std_io;

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(any(feature = "std", test))]
extern crate std;

pub use framing::{
    BorrowedFrame, BorrowedFrameRead, FrameError, FrameLimits, MAX_SEGMENTS, Segment,
    parse_frame_into,
};
#[cfg(feature = "alloc")]
pub use framing::{
    Frame, FrameRead, PreparedSegments, encode_frame, encode_prepared_frame, parse_frame,
};
#[cfg(feature = "alloc")]
pub use packed::{PackedDecoder, PackedEncoder, PackedError, pack, unpack};
#[cfg(feature = "std")]
pub use std_io::{
    BoundedWriter, IoFrameError, MappedFrame, read_frame, write_frame, write_prepared_frame,
};
