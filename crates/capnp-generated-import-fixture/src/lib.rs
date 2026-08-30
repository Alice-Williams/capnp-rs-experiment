#![doc = "Cross-crate generated import compilation and runtime fixture."]

include!(concat!(env!("OUT_DIR"), "/import_fixture.rs"));

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
    use capnp_schema::{CompiledSchema, LoadLimits};

    const REQUEST: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-import-fixture.bin"
    ));

    #[test]
    fn external_generated_crates_compose_for_read_and_build() {
        let schema = Arc::new(
            CompiledSchema::from_code_generator_request(REQUEST, LoadLimits::default())
                .expect("import request"),
        );
        let mut arena = ExclusiveArena::new(64, 2048).expect("arena");
        {
            let mut root = super::import_fixture::Builder::init_root(&schema, &mut arena)
                .expect("import root");
            root.init_wire()
                .expect("external wire builder")
                .set_uint32_value(123)
                .expect("wire value");
            root.init_language()
                .expect("external language builder")
                .set_state(capnp_generated_fixture::language::State::Ready)
                .expect("language enum");
        }
        let message = OwnedMessage::new(arena.into_segments(), ReaderLimits::default())
            .expect("message validates");
        let root =
            super::import_fixture::Reader::from_root(schema, message).expect("import reader");
        assert_eq!(
            root.wire()
                .expect("wire")
                .expect("wire non-null")
                .uint32_value()
                .expect("wire value"),
            123
        );
        assert_eq!(
            root.language()
                .expect("language")
                .expect("language non-null")
                .state()
                .expect("state"),
            capnp_generated_fixture::language::State::Ready
        );
    }
}
