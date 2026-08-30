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
mod packed;

pub use framing::{
    Frame, FrameError, FrameLimits, FrameRead, MAX_SEGMENTS, Segment, encode_frame, parse_frame,
};
pub use packed::{PackedDecoder, PackedEncoder, PackedError, pack, unpack};
