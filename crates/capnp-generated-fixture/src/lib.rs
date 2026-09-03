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
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::task::{Context, Poll, Wake, Waker};

    use capnp_io::{FrameLimits, FrameRead, parse_frame};
    use capnp_message::{BorrowedMessage, ExclusiveArena, OwnedMessage, ReaderLimits};
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

    struct RecordingService {
        response: Arc<OwnedMessage>,
        methods: Arc<Mutex<Vec<u16>>>,
    }

    impl capnp_rpc::LocalService for RecordingService {
        fn dispatch(
            self: Arc<Self>,
            _interface_id: u64,
            method_id: u16,
            _params: Arc<OwnedMessage>,
        ) -> capnp_rpc::MessageFuture {
            self.methods.lock().expect("recording lock").push(method_id);
            let response = Arc::clone(&self.response);
            Box::pin(async move { Ok(response) })
        }
    }

    struct PipelineFactoryService {
        response: Arc<OwnedMessage>,
        stream: capnp_rpc::LocalClient,
        provisional: bool,
    }

    struct MembraneEchoService {
        response: Arc<OwnedMessage>,
        observed: Arc<Mutex<Vec<capnp_rpc::LocalClient>>>,
    }

    impl capnp_rpc::LocalService for MembraneEchoService {
        fn dispatch(
            self: Arc<Self>,
            _interface_id: u64,
            _method_id: u16,
            _params: Arc<OwnedMessage>,
        ) -> capnp_rpc::MessageFuture {
            let response = Arc::clone(&self.response);
            Box::pin(async move { Ok(response) })
        }

        fn dispatch_request(
            self: Arc<Self>,
            _interface_id: u64,
            _method_id: u16,
            request: capnp_rpc::LocalRequest,
        ) -> capnp_rpc::LocalCall {
            let incoming = request
                .capabilities()
                .get(0)
                .expect("request capability slot")
                .expect("request capability");
            self.observed
                .lock()
                .expect("observed lock")
                .push(incoming.clone());
            let capabilities = capnp_rpc::CapabilityList::from_clients([Some(incoming.clone())], 4)
                .expect("response capability table");
            let response = capnp_rpc::LocalResponse::with_capabilities(
                Arc::clone(&self.response),
                capabilities,
            );
            let mut pipeline = capnp_rpc::PipelineBuilder::default();
            pipeline
                .set_capability(
                    capnp_rpc::PipelineTransform::root().pointer_field(0),
                    incoming,
                )
                .expect("provisional capability");
            capnp_rpc::LocalCall::new(Box::pin(async move { Ok(response) }))
                .with_pipeline(pipeline)
                .expect("pipeline installed once")
        }
    }

    struct NeverService;

    impl capnp_rpc::LocalService for NeverService {
        fn dispatch(
            self: Arc<Self>,
            _interface_id: u64,
            _method_id: u16,
            _params: Arc<OwnedMessage>,
        ) -> capnp_rpc::MessageFuture {
            Box::pin(std::future::pending())
        }
    }

    impl capnp_rpc::LocalService for PipelineFactoryService {
        fn dispatch(
            self: Arc<Self>,
            _interface_id: u64,
            _method_id: u16,
            _params: Arc<OwnedMessage>,
        ) -> capnp_rpc::MessageFuture {
            let response = Arc::clone(&self.response);
            Box::pin(async move { Ok(response) })
        }

        fn dispatch_call(
            self: Arc<Self>,
            _interface_id: u64,
            _method_id: u16,
            _params: Arc<OwnedMessage>,
        ) -> capnp_rpc::LocalCall {
            let capabilities =
                capnp_rpc::CapabilityList::from_clients([Some(self.stream.clone())], 4)
                    .expect("bounded capability table");
            let response = capnp_rpc::LocalResponse::with_capabilities(
                Arc::clone(&self.response),
                capabilities,
            );
            let response_future: capnp_rpc::LocalResponseFuture = if self.provisional {
                Box::pin(std::future::pending())
            } else {
                Box::pin(async move { Ok(response) })
            };
            let mut call = capnp_rpc::LocalCall::new(response_future);
            if self.provisional {
                let mut pipeline = capnp_rpc::PipelineBuilder::default();
                pipeline
                    .set_capability(
                        capnp_rpc::PipelineTransform::root().pointer_field(0),
                        self.stream.clone(),
                    )
                    .expect("unique provisional path");
                call.set_pipeline(pipeline).expect("pipeline set once");
            }
            call
        }
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
    fn borrowed_generated_reader_uses_constant_slots_and_borrowed_blobs()
    -> Result<(), Box<dyn std::error::Error>> {
        let FrameRead::Message { frame, remaining } =
            parse_frame(CPP_FRAME, FrameLimits::default())?
        else {
            return Err("fixture is empty".into());
        };
        assert!(remaining.is_empty());
        let segments = frame
            .segments()
            .iter()
            .map(|segment| segment.bytes())
            .collect::<Vec<_>>();
        let message = BorrowedMessage::new(&segments, ReaderLimits::default())
            .expect("borrowed fixture validates");
        let reader = wire_fixture::BorrowedReader::from_root(&message)
            .expect("borrowed generated root opens");
        assert_eq!(reader.uint32_value(), 4_000_000_000);
        assert_eq!(reader.color(), Color::Blue);
        assert_eq!(reader.color_ordinal(), 2);
        assert_eq!(reader.defaulted(), 0);
        let choice = reader.choice();
        assert_eq!(choice.which(), super::wire::choice::Which::Number);
        assert_eq!(choice.number(), 12_345_678_901_234_567_890);
        let metadata = reader.metadata();
        assert_eq!(metadata.created(), 9_876_543_210);
        assert!(metadata.valid());
        assert_eq!(reader.node().expect("borrowed node").value(), 10);
        let values = reader.uint16s().expect("borrowed primitive list");
        assert_eq!(values.len(), 3);
        assert_eq!(values.get(2).expect("borrowed primitive element"), u16::MAX);
        let colors = reader.colors().expect("borrowed enum list");
        assert_eq!(colors.len(), 3);
        assert_eq!(colors.get(2).expect("borrowed enum element"), Color::Blue);
        let nodes = reader.structs().expect("borrowed struct list");
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes.get(1).expect("borrowed struct element").value(), 2);
        let texts = reader.texts().expect("borrowed text list");
        assert_eq!(texts.len(), 3);
        assert_eq!(
            texts.get(2).expect("borrowed text element").to_str(),
            Ok("βeta")
        );
        let blobs = reader.data_blobs().expect("borrowed data list");
        assert_eq!(blobs.len(), 2);
        assert_eq!(
            blobs.get(1).expect("borrowed data element").as_bytes(),
            &[0xde, 0xad, 0xbe, 0xef]
        );
        let nested = reader.nested_lists().expect("borrowed nested lists");
        let first = nested.get(0).expect("borrowed nested primitive list");
        assert_eq!(first.len(), 3);
        assert_eq!(first.get(2).expect("borrowed nested element"), u16::MAX);
        assert!(
            reader
                .text()
                .expect("borrowed text")
                .to_str()
                .expect("fixture UTF-8")
                .contains("UTF-8: λ")
        );
        assert!(!reader.data().expect("borrowed data").is_empty());
        Ok(())
    }

    #[test]
    fn borrowed_generated_reader_defaults_a_short_data_section() {
        let mut arena = ExclusiveArena::new(16, 256).expect("arena");
        arena
            .init_root_struct(1, 0)
            .expect("short root")
            .set_u32(1, 77, 0)
            .expect("in-range scalar");
        let storage = arena.into_segments();
        let segments = storage
            .iter()
            .map(|segment| segment.as_ref())
            .collect::<Vec<_>>();
        let message = BorrowedMessage::new(&segments, ReaderLimits::default())
            .expect("short message validates");
        let reader =
            wire_fixture::BorrowedReader::from_root(&message).expect("short generated root opens");
        assert_eq!(reader.int32_value(), 77);
        assert_eq!(reader.uint64_value(), 0);
        assert_eq!(reader.defaulted(), 123_456);
        assert_eq!(reader.color(), Color::Red);
        assert_eq!(reader.color_ordinal(), 0);
        assert!(
            reader
                .uint16s()
                .expect("missing list defaults empty")
                .is_empty()
        );
        assert!(
            reader
                .colors()
                .expect("missing enum list defaults empty")
                .is_empty()
        );
        assert!(
            reader
                .structs()
                .expect("missing struct list defaults empty")
                .is_empty()
        );
        assert!(
            reader
                .texts()
                .expect("missing text list defaults empty")
                .is_empty()
        );
        assert!(
            reader
                .data_blobs()
                .expect("missing data list defaults empty")
                .is_empty()
        );
        assert!(
            reader
                .nested_lists()
                .expect("missing nested list defaults empty")
                .is_empty()
        );
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
        let storage = arena.into_segments();
        {
            let segments = storage
                .iter()
                .map(|segment| segment.as_ref())
                .collect::<Vec<_>>();
            let message = BorrowedMessage::new(&segments, ReaderLimits::default())
                .expect("borrowed message validates");
            let reader =
                wire_fixture::BorrowedReader::from_root(&message).expect("borrowed generated root");
            assert_eq!(
                reader.choice().which(),
                super::wire::choice::Which::Unrecognized(55)
            );
        }
        let message =
            OwnedMessage::new(storage, ReaderLimits::default()).expect("message validates");
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
        assert_eq!(
            writes.load(Ordering::SeqCst),
            1,
            "streaming dispatch is eager to preserve call order"
        );
        block_on(streaming.completion()).expect("streaming completion");
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn promise_clients_queue_in_order_and_fail_stably() {
        let schema = streaming_schema();
        let response = {
            let mut arena = ExclusiveArena::new(1, 16).expect("response arena");
            super::streaming::stream_result::Builder::init_root(&schema, &mut arena)
                .expect("response root");
            owned_arena(arena)
        };
        let methods = Arc::new(Mutex::new(Vec::new()));
        let service = Arc::new(RecordingService {
            response,
            methods: Arc::clone(&methods),
        });
        let (promise, resolver) = capnp_rpc::LocalClient::promise(Arc::clone(&schema));
        let params = {
            let mut arena = ExclusiveArena::new(1, 16).expect("params arena");
            super::streaming::stream_result::Builder::init_root(&schema, &mut arena)
                .expect("params root");
            owned_arena(arena)
        };
        let first = promise.call_untyped(1, 7, Arc::clone(&params));
        let second = promise.call_untyped(1, 9, Arc::clone(&params));
        assert!(methods.lock().expect("recording lock").is_empty());
        resolver
            .fulfill(capnp_rpc::LocalClient::new(Arc::clone(&schema), service))
            .expect("promise resolves once");
        block_on(first.response()).expect("first queued response");
        block_on(second.response()).expect("second queued response");
        assert_eq!(*methods.lock().expect("recording lock"), [7, 9]);

        let (left, left_resolver) = capnp_rpc::LocalClient::promise(Arc::clone(&schema));
        let (right, right_resolver) = capnp_rpc::LocalClient::promise(Arc::clone(&schema));
        left_resolver.fulfill(right.clone()).expect("first link");
        assert!(matches!(
            right_resolver.fulfill(left),
            Err(capnp_rpc::RpcError::PromiseCycle)
        ));

        for failed in [
            capnp_rpc::LocalClient::broken(Arc::clone(&schema), "fixture failure"),
            capnp_rpc::LocalClient::disabled(Arc::clone(&schema)),
        ] {
            let error = block_on(failed.call_untyped(1, 0, Arc::clone(&params)).response())
                .expect_err("failed clients reject every call");
            assert!(matches!(error, capnp_rpc::RpcError::Shared(_)));
        }
    }

    #[test]
    fn response_and_provisional_pipelines_preserve_local_client_identity() {
        let schema = streaming_schema();
        let writes = Arc::new(AtomicUsize::new(0));
        let stream = capnp_rpc::LocalClient::new(
            Arc::clone(&schema),
            Arc::new(super::streaming::byte_stream::LocalServer::new(
                Arc::new(ByteService {
                    schema: Arc::clone(&schema),
                    writes: Arc::clone(&writes),
                }),
                Arc::clone(&schema),
            )),
        );
        let response = {
            let mut arena = ExclusiveArena::new(2, 32).expect("result arena");
            super::streaming::open_results::Builder::init_root(&schema, &mut arena)
                .expect("result root")
                .set_stream(0)
                .expect("capability index");
            owned_arena(arena)
        };
        for provisional in [false, true] {
            let factory =
                super::streaming::stream_factory::Client::from_local(capnp_rpc::LocalClient::new(
                    Arc::clone(&schema),
                    Arc::new(PipelineFactoryService {
                        response: Arc::clone(&response),
                        stream: stream.clone(),
                        provisional,
                    }),
                ));
            let mut params = ExclusiveArena::new(2, 32).expect("params arena");
            super::streaming::open_params::Builder::init_root(&schema, &mut params)
                .expect("params root");
            let pipeline = factory.open(owned_arena(params)).send_for_pipeline();
            let client = pipeline
                .stream()
                .client()
                .expect("generated pipeline binds client");
            assert!(client.local().when_resolved().is_send());

            let mut write = ExclusiveArena::new(2, 32).expect("write arena");
            super::streaming::write_params::Builder::init_root(&schema, &mut write)
                .expect("write root");
            block_on(client.write(owned_arena(write)).completion()).expect("pipelined write");
        }

        let target_factory = capnp_rpc::LocalClient::new(
            Arc::clone(&schema),
            Arc::new(PipelineFactoryService {
                response,
                stream,
                provisional: true,
            }),
        );
        let (promised_factory, resolver) = capnp_rpc::LocalClient::promise(Arc::clone(&schema));
        let promised_factory =
            super::streaming::stream_factory::Client::from_local(promised_factory);
        let mut params = ExclusiveArena::new(2, 32).expect("promised params arena");
        super::streaming::open_params::Builder::init_root(&schema, &mut params)
            .expect("promised params root");
        let promised_stream = promised_factory
            .open(owned_arena(params))
            .send_for_pipeline()
            .stream()
            .client()
            .expect("promised generated pipeline");
        resolver
            .fulfill(target_factory)
            .expect("factory promise resolves");
        let mut write = ExclusiveArena::new(2, 32).expect("promised write arena");
        super::streaming::write_params::Builder::init_root(&schema, &mut write)
            .expect("promised write root");
        block_on(promised_stream.write(owned_arena(write)).completion())
            .expect("promise pipeline forwards provisional client");
        assert_eq!(writes.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn dynamic_inheritance_pipeline_tail_call_and_server_set_obey_capability_rules() {
        let language_schema = language_schema();
        let language_local = capnp_rpc::LocalClient::new(
            Arc::clone(&language_schema),
            Arc::new(super::language::generic_service::LocalServer::<
                LanguageService,
                String,
            >::new(
                Arc::new(LanguageService {
                    schema: Arc::clone(&language_schema),
                }),
                Arc::clone(&language_schema),
            )),
        );
        let dynamic = capnp_rpc::DynamicCapability::new(
            Arc::clone(&language_schema),
            language_local,
            super::language::generic_service::TYPE_ID,
        )
        .expect("dynamic interface");
        let inherited = dynamic.method("ping").expect("inherited method");
        assert_eq!(
            inherited.interface_id(),
            super::language::base_service::TYPE_ID
        );
        let base = dynamic
            .upcast(super::language::base_service::TYPE_ID)
            .expect("declared upcast");
        assert!(
            base.upcast(super::language::generic_service::TYPE_ID)
                .is_err(),
            "upcast cannot move down the inheritance graph"
        );
        assert_eq!(
            base.cast(super::language::generic_service::TYPE_ID)
                .expect("explicit dynamic reinterpretation")
                .interface_id(),
            Some(super::language::generic_service::TYPE_ID)
        );
        let mut ping = ExclusiveArena::new(1, 16).expect("ping params");
        super::language::ping_params::Builder::init_root(&language_schema, &mut ping)
            .expect("ping root");
        let response = block_on(
            dynamic
                .call("ping", owned_arena(ping))
                .expect("dynamic call")
                .response(),
        )
        .expect("dynamic response");
        assert_eq!(
            response.result().type_id(),
            super::language::ping_results::TYPE_ID
        );

        let server_schema = Arc::clone(&language_schema);
        let dynamic_server = capnp_rpc::DynamicCapability::from_server(
            Arc::clone(&language_schema),
            super::language::generic_service::TYPE_ID,
            Arc::new(move |call: capnp_rpc::DynamicServerCall| {
                assert_eq!(call.method().name(), "ping");
                assert_eq!(
                    call.params().type_id(),
                    super::language::ping_params::TYPE_ID
                );
                let mut arena = ExclusiveArena::new(4, 64).expect("dynamic server result");
                super::language::ping_results::Builder::init_root(&server_schema, &mut arena)
                    .expect("dynamic server root")
                    .set_value(81)
                    .expect("dynamic server value");
                let response = owned_arena(arena);
                capnp_rpc::LocalCall::new(Box::pin(async move {
                    Ok(capnp_rpc::LocalResponse::new(response))
                }))
            }),
        )
        .expect("dynamic server client");
        let mut server_ping = ExclusiveArena::new(1, 16).expect("server ping params");
        super::language::ping_params::Builder::init_root(&language_schema, &mut server_ping)
            .expect("server ping root");
        let server_response = block_on(
            dynamic_server
                .call("ping", owned_arena(server_ping))
                .expect("dynamic server call")
                .response(),
        )
        .expect("dynamic server response");
        assert!(matches!(
            server_response
                .result()
                .get("value")
                .expect("dynamic value"),
            capnp_schema::DynamicValue::UInt32(81)
        ));

        let streaming_schema = streaming_schema();
        let raw_response = {
            let mut arena = ExclusiveArena::new(2, 32).expect("result arena");
            super::streaming::open_results::Builder::init_root(&streaming_schema, &mut arena)
                .expect("result root");
            owned_arena(arena)
        };
        let methods = Arc::new(Mutex::new(Vec::new()));
        let server = Arc::new(RecordingService {
            response: Arc::clone(&raw_response),
            methods,
        });
        let set = capnp_rpc::CapabilityServerSet::new(Arc::clone(&streaming_schema));
        let registered = set.add(Arc::clone(&server));
        assert!(Arc::ptr_eq(
            &set.try_get_local_server(&registered).expect("sync unwrap"),
            &server
        ));
        let other_set =
            capnp_rpc::CapabilityServerSet::<RecordingService>::new(Arc::clone(&streaming_schema));
        assert!(other_set.try_get_local_server(&registered).is_none());
        let ephemeral_server = Arc::new(RecordingService {
            response: Arc::clone(&raw_response),
            methods: Arc::new(Mutex::new(Vec::new())),
        });
        let ephemeral = other_set.add(Arc::clone(&ephemeral_server));
        assert!(
            other_set
                .this_client(&ephemeral_server)
                .expect("live registration")
                .same_identity(&ephemeral)
        );
        drop(ephemeral);
        assert!(other_set.this_client(&ephemeral_server).is_none());
        assert_eq!(
            Arc::strong_count(&ephemeral_server),
            1,
            "the set must not keep a dropped client/server registration alive"
        );
        let (promised, resolver) = capnp_rpc::LocalClient::promise(Arc::clone(&streaming_schema));
        assert!(
            set.try_get_local_server(&promised).is_none(),
            "sync unwrap must not follow unresolved promises"
        );
        resolver
            .fulfill(registered.clone())
            .expect("server promise");
        assert!(Arc::ptr_eq(
            &block_on(set.get_local_server(promised))
                .expect("async unwrap")
                .expect("registered server"),
            &server
        ));

        let capabilities =
            capnp_rpc::CapabilityList::from_clients([Some(registered)], 2).expect("cap table");
        let tail_service = Arc::new(PipelineFactoryService {
            response: raw_response,
            stream: capabilities.get(0).expect("cap slot").expect("cap"),
            provisional: false,
        });
        let tail_client = capnp_rpc::LocalClient::new(Arc::clone(&streaming_schema), tail_service);
        let params = {
            let mut arena = ExclusiveArena::new(1, 16).expect("tail params");
            super::streaming::stream_result::Builder::init_root(&streaming_schema, &mut arena)
                .expect("tail params root");
            owned_arena(arena)
        };
        let transferred = block_on(capnp_rpc::direct_tail_call(&tail_client, 1, 0, params))
            .expect("tail response");
        assert_eq!(
            transferred.capabilities().len(),
            1,
            "tail calls transfer cap tables without proxying"
        );

        let dynamic_factory_service = Arc::new(PipelineFactoryService {
            response: Arc::clone(transferred.message()),
            stream: transferred
                .capabilities()
                .get(0)
                .expect("dynamic cap slot")
                .expect("dynamic cap"),
            provisional: true,
        });
        let dynamic_factory = capnp_rpc::DynamicCapability::new(
            Arc::clone(&streaming_schema),
            capnp_rpc::LocalClient::new(Arc::clone(&streaming_schema), dynamic_factory_service),
            super::streaming::stream_factory::TYPE_ID,
        )
        .expect("dynamic factory");
        let mut open = ExclusiveArena::new(2, 32).expect("dynamic open params");
        super::streaming::open_params::Builder::init_root(&streaming_schema, &mut open)
            .expect("dynamic open root");
        let dynamic_open = dynamic_factory
            .call("open", owned_arena(open))
            .expect("dynamic open call");
        let dynamic_stream = dynamic_open
            .pipeline()
            .capability(&["stream"])
            .expect("dynamic pipeline field");
        assert_eq!(
            dynamic_stream.interface_id(),
            Some(super::streaming::byte_stream::TYPE_ID)
        );
    }

    #[test]
    fn membrane_wraps_each_crossing_once_across_requests_results_pipelines_and_copies() {
        let schema = streaming_schema();
        let response = {
            let mut arena = ExclusiveArena::new(2, 32).expect("membrane response arena");
            super::streaming::open_results::Builder::init_root(&schema, &mut arena)
                .expect("membrane response root")
                .set_stream(0)
                .expect("membrane response cap");
            owned_arena(arena)
        };
        let original = capnp_rpc::LocalClient::new(
            Arc::clone(&schema),
            Arc::new(RecordingService {
                response: Arc::clone(&response),
                methods: Arc::new(Mutex::new(Vec::new())),
            }),
        );
        let observed = Arc::new(Mutex::new(Vec::new()));
        let target = capnp_rpc::LocalClient::new(
            Arc::clone(&schema),
            Arc::new(MembraneEchoService {
                response: Arc::clone(&response),
                observed: Arc::clone(&observed),
            }),
        );
        let membrane = capnp_rpc::Membrane::new(Arc::new(PassPolicy));
        let wrapped = membrane.wrap(target.clone());
        assert!(wrapped.same_identity(&membrane.wrap(target.clone())));
        assert!(target.same_identity(&membrane.reverse_wrap(wrapped.clone())));

        let request_caps = capnp_rpc::CapabilityList::from_clients([Some(original.clone())], 4)
            .expect("request cap table");
        let request =
            capnp_rpc::LocalRequest::with_capabilities(Arc::clone(&response), request_caps);
        let call = wrapped.call_untyped_request(1, 0, request.clone());
        let pipelined = call
            .pipeline
            .clone()
            .client(capnp_rpc::PipelineTransform::root().pointer_field(0));
        let pipelined = block_on(pipelined.when_resolved()).expect("membrane pipeline resolves");
        assert!(pipelined.same_identity(&original));
        let returned = block_on(call.response()).expect("membrane response");
        let returned = returned
            .capabilities()
            .get(0)
            .expect("returned cap slot")
            .expect("returned cap");
        assert!(returned.same_identity(&original));

        let seen = observed.lock().expect("observed lock")[0].clone();
        assert!(!seen.same_identity(&original));
        assert!(seen.same_identity(&membrane.reverse_wrap(original.clone())));

        let copied_inside = membrane
            .copy_request_into(&request)
            .expect("copy request into membrane");
        let copied_outside = membrane
            .copy_request_out(&copied_inside)
            .expect("copy request out of membrane");
        assert!(
            copied_outside
                .capabilities()
                .get(0)
                .expect("copied cap slot")
                .expect("copied cap")
                .same_identity(&original)
        );

        let (promise, resolver) = capnp_rpc::LocalClient::promise(Arc::clone(&schema));
        let wrapped_promise = membrane.wrap(promise.clone());
        assert!(promise.same_identity(&membrane.reverse_wrap(wrapped_promise.clone())));
        resolver.fulfill(target).expect("promise resolves");
        let first = block_on(wrapped_promise.when_resolved()).expect("first resolution");
        let second = block_on(wrapped_promise.when_resolved()).expect("second resolution");
        assert!(first.same_identity(&second));

        let (concurrent, concurrent_resolver) =
            capnp_rpc::LocalClient::promise(Arc::clone(&schema));
        let concurrent = membrane.wrap(concurrent);
        let barrier = Arc::new(Barrier::new(3));
        let threads = (0..2)
            .map(|_| {
                let client = concurrent.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    block_on(client.when_resolved()).expect("concurrent membrane resolution")
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        concurrent_resolver
            .fulfill(first)
            .expect("concurrent promise resolves");
        let resolved = threads
            .into_iter()
            .map(|thread| thread.join().expect("resolution thread"))
            .collect::<Vec<_>>();
        assert!(resolved[0].same_identity(&resolved[1]));
    }

    #[test]
    fn revocable_server_cancels_outstanding_and_rejects_later_calls_without_leaks() {
        let schema = streaming_schema();
        let response = {
            let mut arena = ExclusiveArena::new(1, 16).expect("revocable params arena");
            super::streaming::stream_result::Builder::init_root(&schema, &mut arena)
                .expect("revocable params root");
            owned_arena(arena)
        };
        let revocable =
            capnp_rpc::RevocableServer::new(Arc::clone(&schema), Arc::new(NeverService));
        assert!(!revocable.is_in_use());
        let client = revocable.get_client();
        assert!(revocable.is_in_use());
        let call = client.call_untyped(1, 0, Arc::clone(&response));
        drop(client);
        assert!(revocable.is_in_use(), "outstanding call retains use");
        let wake_count = Arc::new(WakeCount(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&wake_count));
        let mut context = Context::from_waker(&waker);
        let mut response_future = pin!(call.response());
        assert!(matches!(
            response_future.as_mut().poll(&mut context),
            Poll::Pending
        ));
        revocable.revoke().expect("first revoke");
        assert!(
            wake_count.0.load(Ordering::SeqCst) > 0,
            "revocation wakes an outstanding call"
        );
        let revoked_result = response_future.as_mut().poll(&mut context);
        assert!(
            matches!(revoked_result, Poll::Ready(Err(_))),
            "revoked call must complete with an error"
        );
        let Poll::Ready(Err(error)) = revoked_result else {
            return;
        };
        assert!(format!("{error:?}").to_lowercase().contains("revoked"));
        assert!(!revocable.is_in_use());

        let error = block_on(
            revocable
                .get_client()
                .call_untyped(1, 0, Arc::clone(&response))
                .response(),
        )
        .expect_err("later call is rejected");
        assert!(format!("{error:?}").to_lowercase().contains("revoked"));
        assert!(!revocable.is_in_use());
        assert!(matches!(
            revocable.revoke(),
            Err(capnp_rpc::RpcError::MembraneAlreadyRevoked)
        ));

        let policy_calls = Arc::new(AtomicUsize::new(0));
        let membrane = capnp_rpc::Membrane::new(Arc::new(CountPolicy {
            calls: Arc::clone(&policy_calls),
        }));
        let target = capnp_rpc::LocalClient::new(
            Arc::clone(&schema),
            Arc::new(RecordingService {
                response: Arc::clone(&response),
                methods: Arc::new(Mutex::new(Vec::new())),
            }),
        );
        let client = membrane.wrap(target);
        membrane.revoke("policy test").expect("membrane revoke");
        block_on(client.call_untyped(1, 0, response).response())
            .expect_err("revoked forward fails");
        assert_eq!(
            policy_calls.load(Ordering::SeqCst),
            1,
            "new calls still consult policy with a broken target after revocation"
        );
    }

    struct PassPolicy;

    impl capnp_rpc::MembranePolicy for PassPolicy {}

    struct CountPolicy {
        calls: Arc<AtomicUsize>,
    }

    impl capnp_rpc::MembranePolicy for CountPolicy {
        fn inbound_call(
            &self,
            _interface_id: u64,
            _method_id: u16,
            _target: &capnp_rpc::LocalClient,
        ) -> capnp_rpc::MembraneDecision {
            self.calls.fetch_add(1, Ordering::SeqCst);
            capnp_rpc::MembraneDecision::Forward
        }
    }

    struct WakeCount(AtomicUsize);

    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct RedirectPolicy {
        redirect: capnp_rpc::LocalClient,
    }

    impl capnp_rpc::MembranePolicy for RedirectPolicy {
        fn inbound_call(
            &self,
            _interface_id: u64,
            method_id: u16,
            _target: &capnp_rpc::LocalClient,
        ) -> capnp_rpc::MembraneDecision {
            match method_id {
                1 => capnp_rpc::MembraneDecision::Redirect(self.redirect.clone()),
                2 => capnp_rpc::MembraneDecision::Reject(capnp_rpc::CapabilityFailure::Denied(
                    "blocked by membrane policy".to_owned(),
                )),
                _ => capnp_rpc::MembraneDecision::Forward,
            }
        }

        fn outbound_call(
            &self,
            _interface_id: u64,
            method_id: u16,
            _target: &capnp_rpc::LocalClient,
        ) -> capnp_rpc::MembraneDecision {
            self.inbound_call(0, method_id, _target)
        }

        fn should_resolve_before_redirecting(&self) -> bool {
            true
        }
    }

    #[test]
    fn membrane_resolves_reflected_promises_before_redirecting_but_intercepts_settled_calls() {
        let schema = streaming_schema();
        let response = {
            let mut arena = ExclusiveArena::new(1, 16).expect("policy response arena");
            super::streaming::stream_result::Builder::init_root(&schema, &mut arena)
                .expect("policy response root");
            owned_arena(arena)
        };
        let outside_methods = Arc::new(Mutex::new(Vec::new()));
        let outside = capnp_rpc::LocalClient::new(
            Arc::clone(&schema),
            Arc::new(RecordingService {
                response: Arc::clone(&response),
                methods: Arc::clone(&outside_methods),
            }),
        );
        let redirect_methods = Arc::new(Mutex::new(Vec::new()));
        let redirect = capnp_rpc::LocalClient::new(
            Arc::clone(&schema),
            Arc::new(RecordingService {
                response: Arc::clone(&response),
                methods: Arc::clone(&redirect_methods),
            }),
        );
        let membrane = capnp_rpc::Membrane::new(Arc::new(RedirectPolicy { redirect }));
        let inside_view = membrane.reverse_wrap(outside.clone());
        let (promise, resolver) = capnp_rpc::LocalClient::promise(Arc::clone(&schema));
        let outside_promise = membrane.wrap(promise);
        resolver
            .fulfill(inside_view.clone())
            .expect("reflected promise resolves");

        block_on(
            outside_promise
                .call_untyped(1, 1, Arc::clone(&response))
                .response(),
        )
        .expect("reflected promise bypasses redirect after unwrapping");
        assert_eq!(*outside_methods.lock().expect("outside methods"), [1]);
        assert!(
            redirect_methods
                .lock()
                .expect("redirect methods")
                .is_empty()
        );

        let error = block_on(
            outside_promise
                .call_untyped(1, 2, Arc::clone(&response))
                .response(),
        )
        .expect_err("policy rejects blocked method");
        assert!(format!("{error}").contains("blocked by membrane policy"));
        assert_eq!(*outside_methods.lock().expect("outside methods"), [1]);

        block_on(inside_view.call_untyped(1, 1, response).response())
            .expect("settled outbound call redirects");
        assert_eq!(*redirect_methods.lock().expect("redirect methods"), [1]);
    }

    #[test]
    fn membrane_limits_reject_before_dispatch_or_registry_growth() {
        let schema = streaming_schema();
        let response = {
            let mut arena = ExclusiveArena::new(1, 16).expect("limit response arena");
            super::streaming::stream_result::Builder::init_root(&schema, &mut arena)
                .expect("limit response root");
            owned_arena(arena)
        };
        let methods = Arc::new(Mutex::new(Vec::new()));
        let make_client = || {
            capnp_rpc::LocalClient::new(
                Arc::clone(&schema),
                Arc::new(RecordingService {
                    response: Arc::clone(&response),
                    methods: Arc::clone(&methods),
                }),
            )
        };
        let wrapper_limited = capnp_rpc::Membrane::with_limits(
            Arc::new(PassPolicy),
            capnp_rpc::MembraneLimits {
                max_wrappers: 1,
                max_outstanding_calls: 4,
            },
        );
        let first = wrapper_limited.wrap(make_client());
        block_on(first.call_untyped(1, 0, Arc::clone(&response)).response())
            .expect("first wrapper is admitted");
        let rejected = wrapper_limited.wrap(make_client());
        let error = block_on(
            rejected
                .call_untyped(1, 0, Arc::clone(&response))
                .response(),
        )
        .expect_err("wrapper limit rejects");
        assert!(format!("{error:?}").contains("MembraneLimit"));
        assert_eq!(*methods.lock().expect("methods"), [0]);

        let call_limited = capnp_rpc::Membrane::with_limits(
            Arc::new(PassPolicy),
            capnp_rpc::MembraneLimits {
                max_wrappers: 1,
                max_outstanding_calls: 0,
            },
        );
        let rejected = call_limited.wrap(make_client());
        let error = block_on(rejected.call_untyped(1, 3, response).response())
            .expect_err("outstanding-call limit rejects");
        assert!(format!("{error:?}").contains("MembraneLimit"));
        assert_eq!(
            *methods.lock().expect("methods"),
            [0],
            "limit failure occurs before target dispatch"
        );
    }

    trait FutureIsSend: Future + Send {
        fn is_send(&self) -> bool {
            true
        }
    }

    impl<T: Future + Send> FutureIsSend for T {}
}
