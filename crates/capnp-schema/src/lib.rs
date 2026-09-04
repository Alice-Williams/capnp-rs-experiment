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

mod dynamic;
mod loader;
mod model;

pub use dynamic::*;
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
    const SCHEMA: &[u8] = fixture!("compiler-request-schema.bin");
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
            ("schema", SCHEMA),
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

    #[test]
    fn dynamic_build_read_union_list_enum_downcast_and_stringify_agree() {
        use std::sync::Arc;

        use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};

        const WIRE_FIXTURE: NodeId = 0x99c9_abad_7396_3922;
        struct TypedWire(DynamicStruct);
        impl FromDynamicStruct for TypedWire {
            const TYPE_ID: NodeId = WIRE_FIXTURE;

            fn from_dynamic(value: DynamicStruct) -> Result<Self, DynamicError> {
                Ok(Self(value))
            }
        }
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<DynamicStruct>();
        assert_send_sync::<DynamicList>();

        let schema = Arc::new(load_fixture(WIRE));
        let mut arena = ExclusiveArena::new(64, 4096).expect("bounded arena");
        {
            let mut root = DynamicStructBuilder::root(&schema, &mut arena, WIRE_FIXTURE)
                .expect("dynamic root builder");
            root.set("uint32Value", DynamicInput::UInt32(0xdead_beef))
                .expect("scalar writes");
            root.set("text", DynamicInput::Text("dynamic text"))
                .expect("Text writes");
            root.set("data", DynamicInput::Data(&[0, 1, 2, 0xff]))
                .expect("Data writes");
            root.set("color", DynamicInput::Enum(77))
                .expect("unknown enum ordinal writes");
            root.set("callback", DynamicInput::Capability(9))
                .expect("capability writes");
            root.set("anyPointer", DynamicInput::Capability(12))
                .expect("unconstrained AnyPointer writes");
            {
                let mut values = root.init_list("uint16s", 3).expect("list initializes");
                values
                    .set(0, DynamicInput::UInt16(2))
                    .expect("element zero");
                values.set(1, DynamicInput::UInt16(3)).expect("element one");
                values.set(2, DynamicInput::UInt16(5)).expect("element two");
            }
            {
                let mut values = root
                    .init_list("structs", 2)
                    .expect("struct list initializes");
                values
                    .struct_element(0)
                    .expect("first struct")
                    .set("value", DynamicInput::UInt32(11))
                    .expect("first value");
                values
                    .struct_element(1)
                    .expect("second struct")
                    .set("value", DynamicInput::UInt32(22))
                    .expect("second value");
            }
            {
                let mut outer = root
                    .init_list("nestedLists", 1)
                    .expect("nested list initializes");
                let mut inner = outer.init_list(0, 3).expect("inner list initializes");
                inner.set(0, DynamicInput::UInt16(7)).expect("inner zero");
                inner.set(1, DynamicInput::UInt16(11)).expect("inner one");
                inner.set(2, DynamicInput::UInt16(13)).expect("inner two");
            }
            {
                let mut texts = root.init_list("texts", 2).expect("Text list initializes");
                texts
                    .set(0, DynamicInput::Text("alpha"))
                    .expect("first Text");
                texts
                    .set(1, DynamicInput::Text("beta"))
                    .expect("second Text");
            }
            {
                let mut blobs = root
                    .init_list("dataBlobs", 1)
                    .expect("Data list initializes");
                blobs
                    .set(0, DynamicInput::Data(&[0xca, 0xfe]))
                    .expect("Data element");
            }
            root.group("choice")
                .expect("union group")
                .set("number", DynamicInput::UInt64(1234))
                .expect("union member writes");
            let mut metadata = root.group("metadata").expect("metadata group");
            metadata
                .set("created", DynamicInput::UInt64(99))
                .expect("group scalar writes");
            metadata
                .set("valid", DynamicInput::Bool(true))
                .expect("group bool writes");
        }

        let message = OwnedMessage::new(arena.into_segments(), ReaderLimits::default())
            .expect("built segments form an owned message");
        let dynamic = DynamicStruct::root(Arc::clone(&schema), Arc::clone(&message), WIRE_FIXTURE)
            .expect("dynamic root opens");
        assert!(matches!(
            dynamic.get("uint32Value"),
            Ok(DynamicValue::UInt32(0xdead_beef))
        ));
        let cached_field = dynamic
            .field("uint32Value")
            .expect("dynamic field descriptor resolves");
        assert!(matches!(
            dynamic.get_field(cached_field),
            Ok(DynamicValue::UInt32(0xdead_beef))
        ));
        let other_schema = Arc::new(load_fixture(WIRE));
        let other_dynamic = DynamicStruct::root(
            Arc::clone(&other_schema),
            Arc::clone(&message),
            WIRE_FIXTURE,
        )
        .expect("second dynamic root opens");
        let foreign_field = other_dynamic
            .field("uint32Value")
            .expect("foreign descriptor resolves");
        assert!(matches!(
            dynamic.get_field(foreign_field),
            Err(DynamicError::TypeMismatch { .. })
        ));
        assert!(matches!(
            dynamic.get("defaulted"),
            Ok(DynamicValue::UInt32(123456))
        ));
        assert!(matches!(
            dynamic.get("defaultText"),
            Ok(DynamicValue::Text(ref value)) if value == "default text"
        ));
        assert!(matches!(
            dynamic.get("color"),
            Ok(DynamicValue::Enum(ref value)) if value.ordinal == 77 && value.name().is_none()
        ));
        assert!(matches!(
            dynamic.get("callback"),
            Ok(DynamicValue::Capability(Some(9)))
        ));
        assert!(matches!(
            dynamic.get("anyPointer"),
            Ok(DynamicValue::AnyPointer(DynamicAnyPointer::Capability(12)))
        ));
        let DynamicValue::List(Some(values)) = dynamic.get("uint16s").expect("list reads") else {
            assert!(matches!(
                dynamic.get("uint16s"),
                Ok(DynamicValue::List(Some(_)))
            ));
            return;
        };
        assert!(matches!(values.get(2), Ok(DynamicValue::UInt16(5))));
        assert_eq!(values.stringify().expect("list stringifies"), "[2, 3, 5]");
        let DynamicValue::List(Some(outer)) =
            dynamic.get("nestedLists").expect("nested list reads")
        else {
            return;
        };
        let DynamicValue::List(Some(inner)) = outer.get(0).expect("inner list reads") else {
            return;
        };
        assert_eq!(
            inner.stringify().expect("inner list stringifies"),
            "[7, 11, 13]"
        );
        let DynamicValue::List(Some(texts)) = dynamic.get("texts").expect("Text list reads") else {
            return;
        };
        assert!(matches!(texts.get(1), Ok(DynamicValue::Text(ref value)) if value == "beta"));
        let DynamicValue::List(Some(blobs)) = dynamic.get("dataBlobs").expect("Data list reads")
        else {
            return;
        };
        assert!(
            matches!(blobs.get(0), Ok(DynamicValue::Data(ref value)) if value == &[0xca, 0xfe])
        );

        let DynamicValue::List(Some(structs)) = dynamic.get("structs").expect("struct list reads")
        else {
            assert!(matches!(
                dynamic.get("structs"),
                Ok(DynamicValue::List(Some(_)))
            ));
            return;
        };
        let DynamicValue::Struct(Some(second)) = structs.get(1).expect("struct element reads")
        else {
            assert!(matches!(structs.get(1), Ok(DynamicValue::Struct(Some(_)))));
            return;
        };
        assert!(matches!(second.get("value"), Ok(DynamicValue::UInt32(22))));

        let DynamicValue::Struct(Some(choice)) = dynamic.get("choice").expect("group reads") else {
            assert!(matches!(
                dynamic.get("choice"),
                Ok(DynamicValue::Struct(Some(_)))
            ));
            return;
        };
        assert_eq!(
            choice
                .active_union_field()
                .expect("union reads")
                .map(|field| field.name.as_str()),
            Some("number")
        );
        assert!(matches!(
            choice.get("number"),
            Ok(DynamicValue::UInt64(1234))
        ));
        assert!(matches!(
            choice.get("words"),
            Err(DynamicError::InactiveUnion { .. })
        ));

        let typed = dynamic
            .clone()
            .downcast::<TypedWire>()
            .expect("matching downcast");
        assert!(
            matches!(typed.0.get("text"), Ok(DynamicValue::Text(ref value)) if value == "dynamic text")
        );
        assert!(
            dynamic
                .stringify()
                .expect("struct stringifies")
                .contains("number = 1234")
        );

        let low_level = message.root_struct().expect("typed root opens");
        let field = match &schema.node(WIRE_FIXTURE).expect("schema node").kind {
            NodeKind::Struct(value) => value.field("uint32Value").expect("schema field"),
            _ => return,
        };
        let offset = match field.kind {
            FieldKind::Slot { offset, .. } => offset,
            FieldKind::Group { .. } => return,
        };
        assert_eq!(
            low_level
                .root()
                .with_reader(|reader| {
                    reader
                        .data_section()?
                        .read_u32(offset, 0)
                        .map_err(capnp_message::StructReadError::from)
                })
                .expect("generated-style reader opens")
                .expect("generated-style scalar reads"),
            0xdead_beef
        );
    }

    #[test]
    fn aggregate_schema_constants_keep_their_message_without_leaked_lifetimes() {
        use std::sync::Arc;

        use capnp_message::{ExclusiveArena, OwnedMessage, ReaderLimits};

        let schema = Arc::new(load_fixture(LANGUAGE));
        let fixture = schema
            .node(0xb4c4_35b2_1aa9_b116)
            .expect("LanguageFixture exists");
        let sample = schema
            .nested(fixture.id, "sampleBox")
            .expect("aggregate constant exists");
        let NodeKind::Const(sample) = &sample.kind else {
            return;
        };
        let Type::Struct { type_id, brand } = &sample.ty else {
            return;
        };
        let dynamic = DynamicStruct::from_branded_value(
            Arc::clone(&schema),
            *type_id,
            brand.clone(),
            &sample.value,
            ReaderLimits::default(),
        )
        .expect("struct constant opens")
        .expect("struct constant is non-null");
        let aggregate_value = dynamic.get("value");
        assert!(
            matches!(
                aggregate_value,
                Ok(DynamicValue::Text(ref value)) if value == "constant generic struct"
            ),
            "{aggregate_value:?}"
        );

        let mut arena = ExclusiveArena::new(8, 64).expect("generic arena");
        DynamicStructBuilder::root_branded(&schema, &mut arena, *type_id, brand.clone())
            .expect("generic builder opens")
            .set("value", DynamicInput::Text("brand-resolved"))
            .expect("type parameter resolves while writing");
        let message = OwnedMessage::new(arena.into_segments(), ReaderLimits::default())
            .expect("generic message owns its segments");
        let dynamic =
            DynamicStruct::root_branded(Arc::clone(&schema), message, *type_id, brand.clone())
                .expect("generic reader opens");
        assert!(matches!(
            dynamic.get("value"),
            Ok(DynamicValue::Text(ref value)) if value == "brand-resolved"
        ));

        let primes = schema
            .nested(fixture.id, "primes")
            .expect("list constant exists");
        let NodeKind::Const(primes) = &primes.kind else {
            return;
        };
        let Type::List(element) = &primes.ty else {
            return;
        };
        let dynamic = DynamicList::from_value(
            Arc::clone(&schema),
            (**element).clone(),
            &primes.value,
            ReaderLimits::default(),
        )
        .expect("list constant opens")
        .expect("list constant is non-null");
        assert_eq!(
            dynamic.stringify().expect("constant list stringifies"),
            "[2, 3, 5, 7, 11]"
        );
    }
}
