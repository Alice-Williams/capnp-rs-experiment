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
    const EVOLUTION_V2_MESSAGE: &[u8] = fixture!("evolution-v2-unpacked.bin");
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
    fn older_dynamic_schema_reads_newer_message_and_preserves_unknown_enum() {
        use std::sync::Arc;

        use capnp_io::{FrameLimits, FrameRead, parse_frame};
        use capnp_message::{OwnedMessage, ReaderLimits};

        const RECORD: NodeId = 0x8178_7eed_de27_c411;
        let schema = Arc::new(load_fixture(EVOLUTION_V1));
        let parsed = parse_frame(EVOLUTION_V2_MESSAGE, FrameLimits::default())
            .expect("newer fixture frame parses");
        assert!(matches!(parsed, FrameRead::Message { .. }));
        let FrameRead::Message { frame, remaining } = parsed else {
            return;
        };
        assert!(remaining.is_empty());
        let message = OwnedMessage::new(
            frame.segments().iter().map(|segment| segment.bytes()),
            ReaderLimits::default(),
        )
        .expect("newer fixture message opens");
        let dynamic = DynamicStruct::root(Arc::clone(&schema), message, RECORD)
            .expect("older schema opens newer message");
        let id_field = dynamic.scalar_field("id").expect("id plan");
        let name_field = dynamic.text_field("name").expect("name plan");
        let state_field = dynamic.scalar_field("state").expect("state plan");
        let values_field = dynamic.list_field("values").expect("values plan");

        let observed = dynamic
            .with_view(|view| {
                let DynamicScalarValue::UInt32(id) = view.get_scalar(&id_field)? else {
                    return Err(DynamicError::TypeMismatch {
                        expected: "UInt32 id",
                    });
                };
                let DynamicScalarValue::Enum { ordinal, .. } = view.get_scalar(&state_field)?
                else {
                    return Err(DynamicError::TypeMismatch {
                        expected: "state enum",
                    });
                };
                let name = view.with_text(&name_field, str::to_owned)?;
                let second = view.with_list(&values_field, |values| values.get_u32(1))?;
                Ok((id, name, ordinal, second))
            })
            .expect("old dynamic schema reads compatible newer storage");
        assert_eq!(
            observed,
            (17, "written with evolution-v2".to_owned(), 2, 42)
        );
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
        assert_send_sync::<OwnedDynamicField>();
        assert_send_sync::<OwnedDynamicTextField>();
        assert_send_sync::<OwnedDynamicDataField>();
        assert_send_sync::<OwnedDynamicScalarField>();
        assert_send_sync::<OwnedDynamicListField>();
        assert_send_sync::<OwnedDynamicStructField>();

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
        let text_field = dynamic
            .text_field("text")
            .expect("typed Text descriptor resolves");
        let data_field = dynamic
            .data_field("data")
            .expect("typed Data descriptor resolves");
        let borrowed = dynamic
            .with_view(|view| {
                view.with_text(&text_field, |text| {
                    view.with_data(&data_field, |data| (text.to_owned(), data.to_vec()))
                })?
            })
            .expect("batched blob view reads");
        assert_eq!(borrowed, ("dynamic text".to_owned(), vec![0, 1, 2, 0xff]));
        assert_eq!(
            dynamic
                .with_text(&text_field, str::len)
                .expect("single Text plan reads"),
            "dynamic text".len()
        );
        assert!(matches!(
            dynamic.text_field("uint32Value"),
            Err(DynamicError::TypeMismatch { .. })
        ));
        let foreign_text = other_dynamic
            .text_field("text")
            .expect("foreign Text plan resolves");
        assert!(matches!(
            dynamic.with_text(&foreign_text, str::len),
            Err(DynamicError::TypeMismatch { .. })
        ));
        let foreign_scalar = other_dynamic
            .scalar_field("defaulted")
            .expect("foreign scalar plan resolves");
        assert!(matches!(
            dynamic.get_scalar(&foreign_scalar),
            Err(DynamicError::TypeMismatch { .. })
        ));
        let default_text = dynamic
            .text_field("defaultText")
            .expect("defaulted Text plan resolves");
        assert_eq!(
            dynamic
                .with_text(&default_text, str::to_owned)
                .expect("non-null schema default is borrowed"),
            "default text"
        );
        assert!(matches!(
            dynamic.get("defaulted"),
            Ok(DynamicValue::UInt32(123456))
        ));
        let defaulted_scalar = dynamic
            .scalar_field("defaulted")
            .expect("typed defaulted scalar resolves");
        let color_scalar = dynamic
            .scalar_field("color")
            .expect("typed enum scalar resolves");
        assert_eq!(
            dynamic
                .get_scalar(&defaulted_scalar)
                .expect("prepared default reads"),
            DynamicScalarValue::UInt32(123456)
        );
        assert_eq!(
            dynamic
                .get_scalar(&color_scalar)
                .expect("prepared enum reads"),
            DynamicScalarValue::Enum {
                type_id: 0xd5e4_ed5f_9f36_445f,
                ordinal: 77,
            }
        );
        assert!(matches!(
            dynamic.scalar_field("text"),
            Err(DynamicError::TypeMismatch { .. })
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

        let uint16s_field = dynamic
            .list_field("uint16s")
            .expect("typed primitive-list descriptor resolves");
        let node_field = dynamic
            .struct_field("node")
            .expect("typed nested-struct descriptor resolves");
        let structs_field = dynamic
            .list_field("structs")
            .expect("typed struct-list descriptor resolves");
        let nested_lists_field = dynamic
            .list_field("nestedLists")
            .expect("typed nested-list descriptor resolves");
        let node_value_field = second.field("value").expect("nested scalar resolves");
        let borrowed_nested = dynamic
            .with_view(|view| {
                let primitive = view.with_list(&uint16s_field, |list| {
                    assert_eq!(list.len(), 3);
                    list.get_u16(2)
                })?;
                let nested = view.with_struct(&node_field, |child| {
                    let DynamicValue::UInt32(value) = child.get_scalar_field(node_value_field)?
                    else {
                        return Err(DynamicError::TypeMismatch {
                            expected: "UInt32 nested value",
                        });
                    };
                    Ok(value)
                })?;
                let struct_list = view.with_list(&structs_field, |list| {
                    list.with_struct(1, |child| {
                        let DynamicValue::UInt32(value) =
                            child.get_scalar_field(node_value_field)?
                        else {
                            return Err(DynamicError::TypeMismatch {
                                expected: "UInt32 struct-list value",
                            });
                        };
                        Ok(value)
                    })
                })?;
                let nested_list = view.with_list(&nested_lists_field, |outer| {
                    outer.with_list(0, |inner| inner.get_u16(2))
                })?;
                Ok((primitive, nested, struct_list, nested_list))
            })
            .expect("nested borrowed views read");
        assert_eq!(borrowed_nested, (5, 0, 22, 13));

        let owned_general = dynamic
            .owned_field("uint32Value")
            .expect("owned general descriptor resolves");
        assert_eq!(
            owned_general
                .schema_field()
                .expect("owned descriptor retains schema")
                .name,
            "uint32Value"
        );
        let owned_defaulted = dynamic
            .owned_scalar_field("defaulted")
            .expect("owned scalar descriptor resolves");
        let owned_text = dynamic
            .owned_text_field("text")
            .expect("owned Text descriptor resolves");
        let owned_default_text = dynamic
            .owned_text_field("defaultText")
            .expect("owned default Text descriptor resolves");
        let owned_data = dynamic
            .owned_data_field("data")
            .expect("owned Data descriptor resolves");
        let owned_list = dynamic
            .owned_list_field("uint16s")
            .expect("owned List descriptor resolves");
        let owned_node = dynamic
            .owned_struct_field("node")
            .expect("owned struct descriptor resolves");
        let owned_node_value = second
            .owned_scalar_field("value")
            .expect("owned child scalar descriptor resolves");
        assert_eq!(
            dynamic
                .get_owned_scalar(&owned_defaulted)
                .expect("owned scalar reads directly"),
            DynamicScalarValue::UInt32(123456)
        );
        assert_eq!(
            dynamic
                .with_owned_text(&owned_text, str::len)
                .expect("owned Text reads directly"),
            "dynamic text".len()
        );
        assert_eq!(
            dynamic
                .with_owned_data(&owned_data, <[u8]>::len)
                .expect("owned Data reads directly"),
            4
        );
        assert_eq!(
            dynamic
                .with_owned_list(&owned_list, |list| list.get_u16(2))
                .expect("owned List reads directly"),
            5
        );
        assert_eq!(
            dynamic
                .with_owned_struct(&owned_node, |node| {
                    node.get_owned_scalar(&owned_node_value)
                })
                .expect("owned struct reads directly"),
            DynamicScalarValue::UInt32(0)
        );
        let first_owned = dynamic
            .owned_field_by_index(0)
            .expect("indexed owned descriptor resolves");
        assert_eq!(first_owned.type_id(), WIRE_FIXTURE);
        assert!(Arc::ptr_eq(first_owned.schema(), &schema));
        assert_eq!(
            first_owned
                .schema_field()
                .expect("indexed descriptor retains its field"),
            dynamic
                .field_by_index(0)
                .expect("borrowed indexed descriptor resolves")
                .schema_field()
        );
        let worker_value = dynamic.clone();
        let detached = std::thread::spawn(move || -> Result<_, DynamicError> {
            let DynamicValue::UInt32(general) = worker_value.get_owned_field(&owned_general)?
            else {
                return Err(DynamicError::TypeMismatch {
                    expected: "owned UInt32 field",
                });
            };
            worker_value.with_view(|view| {
                let defaulted = view.get_owned_scalar(&owned_defaulted)?;
                let text = view.with_owned_text(&owned_text, str::to_owned)?;
                let default_text = view.with_owned_text(&owned_default_text, str::to_owned)?;
                let data = view.with_owned_data(&owned_data, <[u8]>::to_vec)?;
                let last = view.with_owned_list(&owned_list, |list| list.get_u16(2))?;
                let nested = view.with_owned_struct(&owned_node, |child| {
                    child.get_owned_scalar(&owned_node_value)
                })?;
                Ok((general, defaulted, text, default_text, data, last, nested))
            })
        })
        .join()
        .expect("detached owned-descriptor worker does not panic")
        .expect("detached owned-descriptor worker reads");
        assert_eq!(
            detached,
            (
                0xdead_beef,
                DynamicScalarValue::UInt32(123456),
                "dynamic text".to_owned(),
                "default text".to_owned(),
                vec![0, 1, 2, 0xff],
                5,
                DynamicScalarValue::UInt32(0),
            )
        );

        let foreign_owned_scalar = other_dynamic
            .owned_scalar_field("defaulted")
            .expect("foreign owned scalar descriptor resolves");
        assert!(matches!(
            dynamic.get_owned_scalar(&foreign_owned_scalar),
            Err(DynamicError::TypeMismatch { .. })
        ));
        let foreign_owned_field = other_dynamic
            .owned_field("uint32Value")
            .expect("foreign owned builder descriptor resolves");
        let mut descriptor_arena =
            ExclusiveArena::new(8, 64).expect("descriptor identity arena opens");
        let mut descriptor_builder =
            DynamicStructBuilder::root(&schema, &mut descriptor_arena, WIRE_FIXTURE)
                .expect("descriptor identity root opens");
        assert!(matches!(
            descriptor_builder.set_owned_field(&foreign_owned_field, DynamicInput::UInt32(1)),
            Err(DynamicError::TypeMismatch { .. })
        ));
        assert!(matches!(
            dynamic.list_field("uint32Value"),
            Err(DynamicError::TypeMismatch { .. })
        ));
        assert!(matches!(
            dynamic.with_view(|view| { view.with_list(&uint16s_field, |list| list.get_u16(3)) }),
            Err(DynamicError::IndexOutOfBounds { index: 3, len: 3 })
        ));

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
        let choice_field = dynamic
            .struct_field("choice")
            .expect("typed group descriptor resolves");
        let number_field = choice
            .scalar_field("number")
            .expect("typed union scalar resolves");
        let none_field = choice
            .scalar_field("none")
            .expect("typed inactive union scalar resolves");
        let borrowed_choice = dynamic
            .with_view(|view| {
                view.with_struct(&choice_field, |choice| {
                    assert_eq!(
                        choice
                            .active_union_field()?
                            .map(|field| field.name.as_str()),
                        Some("number")
                    );
                    assert!(matches!(
                        choice.get_scalar(&none_field),
                        Err(DynamicError::InactiveUnion { .. })
                    ));
                    choice.get_scalar(&number_field)
                })
            })
            .expect("callback-scoped group reads");
        assert_eq!(borrowed_choice, DynamicScalarValue::UInt64(1234));

        let write_uint32 = dynamic
            .owned_field("uint32Value")
            .expect("owned builder scalar descriptor resolves");
        let write_text = dynamic
            .owned_field("text")
            .expect("owned builder Text descriptor resolves");
        let write_list = dynamic
            .owned_field("uint16s")
            .expect("owned builder List descriptor resolves");
        let write_node = dynamic
            .owned_field("node")
            .expect("owned builder struct descriptor resolves");
        let write_node_value = second
            .owned_field("value")
            .expect("owned builder child descriptor resolves");
        let write_choice = dynamic
            .owned_field("choice")
            .expect("owned builder group descriptor resolves");
        let write_number = choice
            .owned_field("number")
            .expect("owned builder union descriptor resolves");
        let write_schema = Arc::clone(&schema);
        let built_in_worker = std::thread::spawn(move || -> Result<_, DynamicError> {
            let mut worker_arena =
                ExclusiveArena::new(64, 256).expect("detached builder arena opens");
            {
                let mut root =
                    DynamicStructBuilder::root(&write_schema, &mut worker_arena, WIRE_FIXTURE)?;
                assert_eq!(root.owned_field_type(&write_uint32)?, Type::UInt32);
                root.set_owned_field(&write_uint32, DynamicInput::UInt32(77))?;
                root.set_owned_field(&write_text, DynamicInput::Text("worker"))?;
                root.init_list_owned_field(&write_list, 2)?
                    .set(1, DynamicInput::UInt16(9))?;
                root.init_struct_owned_field(&write_node)?
                    .set_owned_field(&write_node_value, DynamicInput::UInt32(88))?;
                let mut choice = root.group_owned_field(&write_choice)?;
                choice.activate_owned_field(&write_number)?;
                choice.set_owned_field(&write_number, DynamicInput::UInt64(99))?;
            }
            let worker_message =
                OwnedMessage::new(worker_arena.into_segments(), ReaderLimits::default())
                    .expect("detached builder output validates");
            let worker_value =
                DynamicStruct::root(Arc::clone(&write_schema), worker_message, WIRE_FIXTURE)?;
            Ok((
                worker_value.get("uint32Value")?,
                worker_value.get("text")?,
                worker_value.get("uint16s")?,
                worker_value.get("node")?,
                worker_value.get("choice")?,
            ))
        })
        .join()
        .expect("detached descriptor-driven builder does not panic")
        .expect("detached descriptor-driven builder succeeds");
        assert!(matches!(built_in_worker.0, DynamicValue::UInt32(77)));
        assert!(matches!(built_in_worker.1, DynamicValue::Text(ref value) if value == "worker"));
        let worker_list = match built_in_worker.2 {
            DynamicValue::List(Some(value)) => value,
            other => {
                assert!(
                    matches!(other, DynamicValue::List(Some(_))),
                    "worker list was not retained"
                );
                return;
            }
        };
        assert!(matches!(worker_list.get(1), Ok(DynamicValue::UInt16(9))));
        let worker_node = match built_in_worker.3 {
            DynamicValue::Struct(Some(value)) => value,
            other => {
                assert!(
                    matches!(other, DynamicValue::Struct(Some(_))),
                    "worker node was not retained"
                );
                return;
            }
        };
        assert!(matches!(
            worker_node.get("value"),
            Ok(DynamicValue::UInt32(88))
        ));
        let worker_choice = match built_in_worker.4 {
            DynamicValue::Struct(Some(value)) => value,
            other => {
                assert!(
                    matches!(other, DynamicValue::Struct(Some(_))),
                    "worker choice was not retained"
                );
                return;
            }
        };
        assert!(matches!(
            worker_choice.get("number"),
            Ok(DynamicValue::UInt64(99))
        ));

        let root_schema = match &schema.node(WIRE_FIXTURE).expect("wire schema exists").kind {
            NodeKind::Struct(value) => value,
            _ => return,
        };
        let choice_type_id = match root_schema
            .field("choice")
            .expect("choice field exists")
            .kind
        {
            FieldKind::Group { type_id } => type_id,
            FieldKind::Slot { .. } => return,
        };
        let choice_schema = match &schema
            .node(choice_type_id)
            .expect("choice schema exists")
            .kind
        {
            NodeKind::Struct(value) => value,
            _ => return,
        };
        let mut unknown_arena = ExclusiveArena::new(8, 256).expect("unknown union arena");
        unknown_arena
            .init_root_struct(root_schema.data_word_count, root_schema.pointer_count)
            .expect("unknown union root initializes")
            .set_u16(choice_schema.discriminant_offset, 55, 0)
            .expect("unknown union discriminant writes");
        let unknown_message =
            OwnedMessage::new(unknown_arena.into_segments(), ReaderLimits::default())
                .expect("unknown union message opens");
        let unknown = DynamicStruct::root(Arc::clone(&schema), unknown_message, WIRE_FIXTURE)
            .expect("unknown union root opens");
        let unknown_discriminant = unknown
            .with_view(|view| {
                view.with_struct(&choice_field, |choice| {
                    assert!(choice.active_union_field()?.is_none());
                    choice.union_discriminant()
                })
            })
            .expect("unknown union remains observable");
        assert_eq!(unknown_discriminant, Some(55));

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
