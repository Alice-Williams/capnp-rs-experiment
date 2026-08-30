#![doc = "Owned compiled-schema reflection for native Cap'n Proto tooling."]
//!
//! Compatibility is defined by `schema.capnp` and the generated accessors from
//! pinned C++ commit `e7c9cd96f1505b5ae486db7821006c2f5dce5b5b`.
//! Loading is bounded by standard framing, traversal, nesting, and metadata
//! item limits. Unknown union tags and inconsistent lookup metadata are errors;
//! they are never silently interpreted as a known declaration.
//!
//! This crate deliberately owns metadata rather than retaining self-referential
//! wire views. Dynamic value interpretation and schema compilation are later
//! milestones; pointer-backed values are retained here by their validated wire
//! kind so every schema value remains describable without pre-empting M18.

mod loader;
mod model;

pub use loader::{LoadError, LoadLimits};
pub use model::*;

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../conformance/fixtures/cpp/",
                "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
                $name
            ))
        };
    }
    const EVOLUTION_V1: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-evolution-v1.bin"
    ));
    const EVOLUTION_V2: &[u8] = fixture!("compiler-request-evolution-v2.bin");
    const EVOLUTION_V3: &[u8] = fixture!("compiler-request-evolution-v3.bin");
    const IMPORT: &[u8] = fixture!("compiler-request-import-fixture.bin");
    const LANGUAGE: &[u8] = fixture!("compiler-request-language-fixture.bin");
    const STREAMING: &[u8] = fixture!("compiler-request-streaming-fixture.bin");
    const WIRE: &[u8] = fixture!("compiler-request-wire-fixture.bin");

    fn load_fixture(bytes: &[u8]) -> CompiledSchema {
        CompiledSchema::from_code_generator_request(bytes, LoadLimits::default())
            .expect("pinned compiler request loads")
    }

    #[test]
    fn every_pinned_compiler_request_loads() {
        for (name, bytes) in [
            ("evolution-v1", EVOLUTION_V1),
            ("evolution-v2", EVOLUTION_V2),
            ("evolution-v3", EVOLUTION_V3),
            ("imports", IMPORT),
            ("language", LANGUAGE),
            ("streaming", STREAMING),
            ("wire", WIRE),
        ] {
            let schema = load_fixture(bytes);
            assert!(!schema.nodes().is_empty(), "{name}");
            assert_eq!(schema.requested_files().len(), 1, "{name}");
            let file = &schema.requested_files()[0];
            assert!(schema.requested_file(file.id).is_some(), "{name}");
            assert!(schema.node(file.id).is_some(), "{name}");
        }
    }

    #[test]
    fn language_fixture_exposes_generics_brands_values_and_annotations() {
        let schema = load_fixture(LANGUAGE);
        let generic_box = schema
            .node(0x82b1_5e53_797a_8580)
            .expect("generic Box node is indexed");
        assert_eq!(generic_box.short_name(), Some("Box"));
        assert_eq!(generic_box.parameters[0].name, "T");
        assert!(generic_box.is_generic);
        assert_eq!(
            schema
                .nested(generic_box.id, "Pair")
                .expect("nested generic type resolves")
                .parameters[0]
                .name,
            "U"
        );

        let fixture = schema
            .node(0xb4c4_35b2_1aa9_b116)
            .expect("LanguageFixture node is indexed");
        let structure = match &fixture.kind {
            NodeKind::Struct(value) => value,
            _ => {
                assert!(matches!(fixture.kind, NodeKind::Struct(_)));
                return;
            }
        };
        let state = structure.field("state").expect("field lookup works");
        assert!(matches!(
            state.kind,
            FieldKind::Slot {
                ty: Type::Enum { .. },
                ..
            }
        ));
        assert!(!generic_box.annotations.is_empty());
        assert!(matches!(
            generic_box.annotations[0].value,
            Value::Text(ref value) if value == "generic-box"
        ));

        let answer = schema
            .nested(fixture.id, "answer")
            .expect("nested constant resolves");
        assert!(matches!(
            answer.kind,
            NodeKind::Const(ConstSchema {
                ty: Type::UInt64,
                value: Value::UInt64(42),
            })
        ));
        let greeting = schema
            .nested(fixture.id, "greeting")
            .expect("Text constant resolves");
        assert!(matches!(
            greeting.kind,
            NodeKind::Const(ConstSchema {
                ty: Type::Text,
                value: Value::Text(ref value),
            }) if value == "hello"
        ));
        let state_node = schema
            .nested(fixture.id, "State")
            .expect("nested enum resolves");
        let state_enum = match &state_node.kind {
            NodeKind::Enum(value) => value,
            _ => {
                assert!(matches!(state_node.kind, NodeKind::Enum(_)));
                return;
            }
        };
        assert_eq!(
            state_enum
                .enumerant("ready")
                .expect("enumerant lookup")
                .code_order,
            1
        );

        let service = schema
            .node(0xa452_e51f_e34f_10ac)
            .expect("generic service is indexed");
        let interface = match &service.kind {
            NodeKind::Interface(value) => value,
            _ => {
                assert!(matches!(service.kind, NodeKind::Interface(_)));
                return;
            }
        };
        let transform = interface.method("transform").expect("method lookup works");
        assert_eq!(transform.implicit_parameters[0].name, "U");

        assert!(schema.source_info(fixture.id).is_some());
        assert!(!schema.requested_files()[0].identifiers.is_empty());
    }

    #[test]
    fn wire_fixture_describes_all_wire_type_families() {
        let schema = load_fixture(WIRE);
        let wire = schema
            .node(0x99c9_abad_7396_3922)
            .expect("WireFixture node is indexed");
        let structure = match &wire.kind {
            NodeKind::Struct(value) => value,
            _ => {
                assert!(matches!(wire.kind, NodeKind::Struct(_)));
                return;
            }
        };
        assert!(matches!(
            structure
                .field("nestedLists")
                .expect("nested list field")
                .kind,
            FieldKind::Slot {
                ty: Type::List(_),
                ..
            }
        ));
        assert!(matches!(
            structure.field("anyStruct").expect("AnyStruct field").kind,
            FieldKind::Slot {
                ty: Type::AnyPointer(AnyPointerType::Unconstrained(AnyPointerKind::Struct)),
                ..
            }
        ));
        let type_id = match structure.field("choice").map(|field| &field.kind) {
            Some(FieldKind::Group { type_id }) => *type_id,
            _ => {
                assert!(matches!(
                    structure.field("choice").map(|field| &field.kind),
                    Some(FieldKind::Group { .. })
                ));
                return;
            }
        };
        let group = schema.node(type_id).expect("group type resolves");
        let group = match &group.kind {
            NodeKind::Struct(value) => value,
            _ => {
                assert!(matches!(group.kind, NodeKind::Struct(_)));
                return;
            }
        };
        assert_eq!(group.discriminant_count, 3);
    }

    fn file_node(id: NodeId, display_name: &str) -> Node {
        Node {
            id,
            display_name: display_name.to_owned(),
            display_name_prefix_length: 0,
            scope_id: 0,
            parameters: Vec::new(),
            is_generic: false,
            nested_nodes: Vec::new(),
            annotations: Vec::new(),
            kind: NodeKind::File,
            start_byte: 0,
            end_byte: 0,
        }
    }

    #[test]
    fn malformed_lookup_metadata_is_rejected() {
        let duplicate = CompiledSchema::indexed(
            CapnpVersion {
                major: 1,
                minor: 0,
                micro: 0,
            },
            vec![file_node(7, "a"), file_node(7, "b")],
            Vec::new(),
            Vec::new(),
        );
        assert_eq!(duplicate, Err(LoadError::DuplicateNodeId(7)));

        let mut invalid_prefix = file_node(9, "short");
        invalid_prefix.display_name_prefix_length = 99;
        assert!(matches!(
            CompiledSchema::indexed(
                CapnpVersion {
                    major: 1,
                    minor: 0,
                    micro: 0
                },
                vec![invalid_prefix],
                Vec::new(),
                Vec::new(),
            ),
            Err(LoadError::DisplayNamePrefix { id: 9, .. })
        ));
    }

    #[test]
    fn framing_and_item_limits_fail_closed() {
        assert_eq!(
            CompiledSchema::from_code_generator_request(&[], LoadLimits::default()),
            Err(LoadError::EmptyRequest)
        );
        let limits = LoadLimits {
            max_metadata_items: 1,
            ..LoadLimits::default()
        };
        assert_eq!(
            CompiledSchema::from_code_generator_request(LANGUAGE, limits),
            Err(LoadError::MetadataLimit { limit: 1 })
        );
    }
}
