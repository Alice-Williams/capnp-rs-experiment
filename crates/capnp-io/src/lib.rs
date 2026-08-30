#![doc = "Synchronous Cap'n Proto framing, packing, and I/O adapters."]
//!
//! Standard unpacked framing follows `capnp/serialize.h` and
//! `capnp/serialize.c++` from pinned C++ oracle commit
//! `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`: a little-endian segment count
//! minus one, one word count per segment, optional 32-bit padding, then segment
//! bodies in order.
//!
//! M04 provides bounded parsing and encoding of complete byte slices. It does
//! not perform packed encoding, stream I/O, pointer traversal, schema checks,
//! or root-object validation.

mod framing;

pub use framing::{
    Frame, FrameError, FrameLimits, FrameRead, MAX_SEGMENTS, Segment, encode_frame, parse_frame,
};
