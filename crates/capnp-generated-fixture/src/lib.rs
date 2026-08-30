#![doc = "Compilation and interoperability coverage for generated M19 data APIs."]

pub mod wire {
    include!(concat!(env!("OUT_DIR"), "/wire_fixture.rs"));
}

pub mod evolution_v2 {
    include!(concat!(env!("OUT_DIR"), "/evolution_v2.rs"));
}

pub mod imports {
    include!(concat!(env!("OUT_DIR"), "/import_fixture.rs"));
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use capnp_io::{FrameLimits, FrameRead, parse_frame};
    use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};
    use capnp_schema::{CompiledSchema, FieldKind, LoadLimits, NodeKind};

    use super::wire::{Color, wire_fixture};

    const ORACLE: &str = "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b";
    const REQUEST: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-wire-fixture.bin"
    ));
    const CPP_FRAME: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "wire-unpacked.bin"
    ));

    fn schema() -> Arc<CompiledSchema> {
        Arc::new(
            CompiledSchema::from_code_generator_request(REQUEST, LoadLimits::default())
                .expect("pinned request loads"),
        )
    }

    fn cpp_message() -> Arc<OwnedMessage> {
        let FrameRead::Message { frame, remaining } =
            parse_frame(CPP_FRAME, FrameLimits::default()).expect("C++ frame parses")
        else {
            unreachable!("fixture is not empty")
        };
        assert!(remaining.is_empty());
        OwnedMessage::new(
            frame.segments().iter().map(|segment| segment.bytes()),
            ReaderLimits::default(),
        )
        .expect("C++ segments validate")
    }

    #[test]
    fn generated_reader_decodes_cpp_fixture_with_typed_lists_groups_and_defaults() {
        let reader = wire_fixture::Reader::from_root(schema(), cpp_message())
            .expect("generated reader opens C++ fixture");
        assert!(reader.bool_value().expect("bool"));
        assert_eq!(reader.uint32_value().expect("u32"), 4_000_000_000);
        assert_eq!(reader.color().expect("enum"), Color::Blue);
        assert!(reader.text().expect("text").contains("UTF-8: λ"));
        assert_eq!(reader.defaulted().expect("explicit default XOR"), 0);
        assert_eq!(
            reader.default_text().expect("pointer default"),
            "overridden"
        );

        let values = reader
            .uint16s()
            .expect("list field")
            .expect("list is non-null");
        assert_eq!(values.len().expect("list length"), 3);
        assert_eq!(values.get(2).expect("typed element"), u16::MAX);

        let nodes = reader
            .structs()
            .expect("struct list")
            .expect("struct list non-null");
        assert_eq!(
            nodes
                .get(1)
                .expect("typed struct element")
                .value()
                .expect("value"),
            2
        );
        let choice = reader.choice().expect("union group");
        assert_eq!(
            choice.which().expect("known union"),
            super::wire::choice::Which::Number
        );
        assert_eq!(
            choice.number().expect("active union value"),
            12_345_678_901_234_567_890
        );
        let metadata = reader.metadata().expect("regular group");
        assert_eq!(metadata.created().expect("created"), 9_876_543_210);
        assert!(metadata.valid().expect("valid"));
        assert_eq!(ORACLE.len(), 40);
    }

    #[test]
    fn generated_builder_round_trips_unknown_enum_union_list_and_struct() {
        let schema = schema();
        let mut arena = ExclusiveArena::new(32, 1024).expect("arena");
        {
            let mut root = wire_fixture::Builder::init_root(&schema, &mut arena)
                .expect("generated root builder");
            root.set_uint32_value(77).expect("scalar setter");
            root.set_color(Color::Unrecognized(99))
                .expect("unknown enum setter");
            root.set_text("native generated").expect("text setter");
            root.set_defaulted(123_456).expect("default XOR setter");
            {
                let mut list = root.init_uint16s(3).expect("typed field list init");
                list.set(0, capnp_schema::DynamicInput::UInt16(2))
                    .expect("list 0");
                list.set(1, capnp_schema::DynamicInput::UInt16(3))
                    .expect("list 1");
                list.set(2, capnp_schema::DynamicInput::UInt16(5))
                    .expect("list 2");
            }
            root.choice()
                .expect("union group builder")
                .set_number(444)
                .expect("union setter activates tag");
            root.init_node()
                .expect("nested struct")
                .set_value(88)
                .expect("nested scalar");
        }
        let message = OwnedMessage::new(arena.into_segments(), ReaderLimits::default())
            .expect("native generated message validates");
        let reader = wire_fixture::Reader::from_root(schema, message).expect("generated read back");
        assert_eq!(reader.uint32_value().expect("scalar"), 77);
        assert_eq!(reader.color().expect("enum"), Color::Unrecognized(99));
        assert_eq!(reader.text().expect("text"), "native generated");
        assert_eq!(reader.defaulted().expect("default"), 123_456);
        assert_eq!(
            reader.default_text().expect("pointer default"),
            "default text"
        );
        assert_eq!(
            reader
                .uint16s()
                .expect("list")
                .expect("non-null")
                .get(2)
                .expect("item"),
            5
        );
        assert_eq!(
            reader.choice().expect("choice").number().expect("number"),
            444
        );
        assert_eq!(
            reader
                .node()
                .expect("node")
                .expect("non-null")
                .value()
                .expect("value"),
            88
        );
    }

    #[test]
    fn generated_union_preserves_an_unrecognized_discriminant() {
        let schema = schema();
        let root_node = schema
            .node(wire_fixture::TYPE_ID)
            .expect("wire fixture schema");
        let NodeKind::Struct(root_schema) = &root_node.kind else {
            unreachable!("wire fixture is a struct")
        };
        let choice_id = match root_schema.field("choice").expect("choice group").kind {
            FieldKind::Group { type_id } => type_id,
            FieldKind::Slot { .. } => unreachable!("choice is a group"),
        };
        let NodeKind::Struct(choice_schema) = &schema.node(choice_id).expect("choice schema").kind
        else {
            unreachable!("choice is a struct group")
        };

        let mut arena = ExclusiveArena::new(32, 1024).expect("arena");
        arena
            .init_root_struct(root_schema.data_word_count, root_schema.pointer_count)
            .expect("raw root")
            .set_u16(choice_schema.discriminant_offset, 55, 0)
            .expect("unknown discriminant writes");
        let message = OwnedMessage::new(arena.into_segments(), ReaderLimits::default())
            .expect("message validates");
        let reader = wire_fixture::Reader::from_root(schema, message).expect("generated reader");
        assert_eq!(
            reader.choice().expect("group").which().expect("raw tag"),
            super::wire::choice::Which::Unrecognized(55)
        );
    }

    #[test]
    fn imported_and_evolved_generated_modules_compile_with_expected_shapes() {
        assert_eq!(super::imports::IMPORTS.len(), 2);
        assert_eq!(
            super::imports::import_fixture::TYPE_ID,
            0xe8a7_d522_0752_0de1
        );
        assert_eq!(super::evolution_v2::State::Paused.ordinal(), 2);
        assert_eq!(super::evolution_v2::record::TYPE_ID, 0x8178_7eed_de27_c411);
    }
}
