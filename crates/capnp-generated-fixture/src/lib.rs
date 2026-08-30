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

pub mod language {
    include!(concat!(env!("OUT_DIR"), "/language_fixture.rs"));
}

pub mod streaming {
    include!(concat!(env!("OUT_DIR"), "/streaming_fixture.rs"));
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Poll, Waker};

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
    const IMPORT_REQUEST: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-import-fixture.bin"
    ));
    const LANGUAGE_REQUEST: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-language-fixture.bin"
    ));
    const LANGUAGE_FRAME: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "language-unpacked.bin"
    ));
    const STREAMING_REQUEST: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../conformance/fixtures/cpp/",
        "e7c9cd96f1505b5ae486db7821006c2f5dce5b5b/",
        "compiler-request-streaming-fixture.bin"
    ));

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn schema() -> Arc<CompiledSchema> {
        Arc::new(
            CompiledSchema::from_code_generator_request(REQUEST, LoadLimits::default())
                .expect("pinned request loads"),
        )
    }

    fn language_schema() -> Arc<CompiledSchema> {
        Arc::new(
            CompiledSchema::from_code_generator_request(LANGUAGE_REQUEST, LoadLimits::default())
                .expect("language request loads"),
        )
    }

    fn streaming_schema() -> Arc<CompiledSchema> {
        Arc::new(
            CompiledSchema::from_code_generator_request(STREAMING_REQUEST, LoadLimits::default())
                .expect("streaming request loads"),
        )
    }

    fn owned_arena(arena: ExclusiveArena) -> Arc<OwnedMessage> {
        OwnedMessage::new(arena.into_segments(), ReaderLimits::default())
            .expect("generated message validates")
    }

    struct LanguageService {
        schema: Arc<CompiledSchema>,
    }

    impl super::language::base_service::Server for LanguageService {
        fn ping(&self, _params: super::language::ping_params::Reader) -> capnp_rpc::MessageFuture {
            let mut arena = ExclusiveArena::new(4, 64).expect("result arena");
            super::language::ping_results::Builder::init_root(&self.schema, &mut arena)
                .expect("ping result root")
                .set_value(73)
                .expect("ping result value");
            let response = owned_arena(arena);
            Box::pin(async move { Ok(response) })
        }
    }

    impl super::language::generic_service::Server<String> for LanguageService {}

    struct FactoryService {
        response: Arc<OwnedMessage>,
    }

    impl super::streaming::stream_factory::Server for FactoryService {
        fn open(&self, _params: super::streaming::open_params::Reader) -> capnp_rpc::MessageFuture {
            let response = Arc::clone(&self.response);
            Box::pin(async move { Ok(response) })
        }
    }

    struct ByteService {
        schema: Arc<CompiledSchema>,
        writes: Arc<AtomicUsize>,
    }

    impl super::streaming::byte_stream::Server for ByteService {
        fn write(
            &self,
            _params: super::streaming::write_params::Reader,
        ) -> capnp_rpc::MessageFuture {
            self.writes.fetch_add(1, Ordering::SeqCst);
            let mut arena = ExclusiveArena::new(1, 16).expect("stream result arena");
            super::streaming::stream_result::Builder::init_root(&self.schema, &mut arena)
                .expect("stream result root");
            let response = owned_arena(arena);
            Box::pin(async move { Ok(response) })
        }
    }

    fn cpp_message() -> Arc<OwnedMessage> {
        owned_frame(CPP_FRAME)
    }

    fn owned_frame(bytes: &[u8]) -> Arc<OwnedMessage> {
        let FrameRead::Message { frame, remaining } =
            parse_frame(bytes, FrameLimits::default()).expect("C++ frame parses")
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
    fn generated_generic_brands_are_typed_and_unbound_access_remains_lossless() {
        let schema = Arc::new(
            CompiledSchema::from_code_generator_request(IMPORT_REQUEST, LoadLimits::default())
                .expect("import request loads"),
        );
        let message = owned_frame(LANGUAGE_FRAME);
        let fixture = super::imports::language_fixture::Reader::from_root(
            Arc::clone(&schema),
            Arc::clone(&message),
        )
        .expect("language fixture opens");
        assert_eq!(
            fixture
                .boxed_text()
                .expect("Box(Text)")
                .expect("non-null")
                .value()
                .expect("T resolves to Text"),
            "generic text"
        );
        assert_eq!(
            fixture
                .boxed_data()
                .expect("Box(Data)")
                .expect("non-null")
                .value()
                .expect("T resolves to Data"),
            vec![0, 1, 2, 0xff]
        );
        let pair = fixture.nested_generic().expect("Pair").expect("non-null");
        assert_eq!(pair.first().expect("outer T"), "nested");
        assert_eq!(pair.second().expect("inner U"), vec![0xca, 0xfe]);

        let unbound = super::imports::box_::Reader::from_root(schema, message)
            .expect("unbound generic reader");
        assert!(matches!(
            unbound.value(),
            Ok(capnp_schema::DynamicValue::AnyPointer(
                capnp_schema::DynamicAnyPointer::Struct(_)
            ))
        ));
    }

    #[test]
    fn generated_generic_builder_binds_text_at_the_wire_boundary() {
        let schema = Arc::new(
            CompiledSchema::from_code_generator_request(IMPORT_REQUEST, LoadLimits::default())
                .expect("import request loads"),
        );
        let mut arena = ExclusiveArena::new(8, 128).expect("arena");
        super::imports::box_::Builder::<String>::init_root(&schema, &mut arena)
            .expect("Box(Text) builder")
            .set_value("typed generic".to_owned())
            .expect("generic setter");
        let message = OwnedMessage::new(arena.into_segments(), ReaderLimits::default())
            .expect("message validates");
        assert_eq!(
            super::imports::box_::Reader::<String>::from_root(schema, message)
                .expect("Box(Text) reader")
                .value()
                .expect("typed value"),
            "typed generic"
        );
    }

    #[test]
    fn generated_constants_and_typed_annotations_run_without_static_backing() {
        assert_eq!(super::language::ANSWER, 42);
        assert_eq!(super::language::GREETING, "hello");
        assert_eq!(super::language::SIGNATURE, &[0, 0xca, 0xfe, 0xff]);

        let schema = Arc::new(
            CompiledSchema::from_code_generator_request(LANGUAGE_REQUEST, LoadLimits::default())
                .expect("language request loads"),
        );
        let primes = super::language::primes(Arc::clone(&schema), ReaderLimits::default())
            .expect("list constant opens")
            .expect("list constant is non-null");
        assert_eq!(primes.len().expect("constant length"), 5);
        assert_eq!(primes.get(4).expect("constant element"), 11);
        let sample = super::language::sample_box(Arc::clone(&schema), ReaderLimits::default())
            .expect("struct constant opens")
            .expect("struct constant is non-null");
        assert_eq!(
            sample.value().expect("branded constant field"),
            "constant generic struct"
        );

        let generic_box = schema
            .node(super::language::box_::TYPE_ID)
            .expect("Box schema");
        let annotation = super::language::fixture_tag_annotation::find(&generic_box.annotations)
            .expect("typed annotation is present");
        assert_eq!(
            super::language::fixture_tag_annotation::decode(annotation)
                .expect("annotation value type matches"),
            "generic-box"
        );
        let targets = std::hint::black_box(super::language::fixture_tag_annotation::TARGETS);
        assert!(targets.structure);
        assert!(targets.field);
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

    #[test]
    fn generated_local_client_dispatches_inherited_interface_method() {
        let schema = language_schema();
        let service = Arc::new(super::language::generic_service::LocalServer::<
            LanguageService,
            String,
        >::new(
            Arc::new(LanguageService {
                schema: Arc::clone(&schema),
            }),
            Arc::clone(&schema),
        ));
        let local = capnp_rpc::LocalClient::new(Arc::clone(&schema), service);
        let client = super::language::generic_service::Client::<String>::from_local(local);

        let mut arena = ExclusiveArena::new(1, 16).expect("params arena");
        super::language::ping_params::Builder::init_root(&schema, &mut arena).expect("ping params");
        let call = client.as_base_service().ping(owned_arena(arena));
        let response = block_on(call.response()).expect("inherited ping dispatches");
        assert_eq!(response.value().expect("typed ping result"), 73);
    }

    #[test]
    fn generated_pipeline_and_streaming_completion_are_exact_and_thread_safe() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<super::language::generic_service::Client<String>>();
        assert_send_sync::<super::streaming::open_results::Pipeline>();

        let schema = streaming_schema();
        let mut result_arena = ExclusiveArena::new(2, 32).expect("open result arena");
        super::streaming::open_results::Builder::init_root(&schema, &mut result_arena)
            .expect("open result")
            .set_stream(42)
            .expect("pipeline capability");
        let raw_response = owned_arena(result_arena);
        let factory = Arc::new(super::streaming::stream_factory::LocalServer::new(
            Arc::new(FactoryService {
                response: Arc::clone(&raw_response),
            }),
            Arc::clone(&schema),
        ));
        let factory_client = super::streaming::stream_factory::Client::from_local(
            capnp_rpc::LocalClient::new(Arc::clone(&schema), factory),
        );
        let mut open_arena = ExclusiveArena::new(2, 32).expect("open params arena");
        super::streaming::open_params::Builder::init_root(&schema, &mut open_arena)
            .expect("open params")
            .set_name("fixture")
            .expect("open name");
        let call = factory_client.open(owned_arena(open_arena));
        let stream_pipeline = call.pipeline.stream();
        assert_eq!(stream_pipeline.transform().pointer_fields(), &[0]);
        assert_eq!(
            stream_pipeline
                .resolve(&raw_response)
                .expect("pipeline resolves"),
            Some(42)
        );
        let _ = block_on(call.response()).expect("open response");

        let writes = Arc::new(AtomicUsize::new(0));
        let byte_server = Arc::new(super::streaming::byte_stream::LocalServer::new(
            Arc::new(ByteService {
                schema: Arc::clone(&schema),
                writes: Arc::clone(&writes),
            }),
            Arc::clone(&schema),
        ));
        let byte_client = super::streaming::byte_stream::Client::from_local(
            capnp_rpc::LocalClient::new(Arc::clone(&schema), byte_server),
        );
        let mut write_arena = ExclusiveArena::new(2, 32).expect("write params arena");
        super::streaming::write_params::Builder::init_root(&schema, &mut write_arena)
            .expect("write params")
            .set_bytes(&[1, 2, 3, 4])
            .expect("write bytes");
        let streaming = byte_client.write(owned_arena(write_arena));
        assert_eq!(writes.load(Ordering::SeqCst), 0);
        block_on(streaming.completion()).expect("streaming completion");
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }
}
