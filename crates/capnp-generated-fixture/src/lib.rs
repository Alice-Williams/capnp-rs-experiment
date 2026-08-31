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
    use std::sync::{Arc, Mutex};
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

    trait FutureIsSend: Future + Send {
        fn is_send(&self) -> bool {
            true
        }
    }

    impl<T: Future + Send> FutureIsSend for T {}
}
