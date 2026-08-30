#![doc = "Thread-safe Cap'n Proto clients, servers, actors, and transports."]

#[cfg(test)]
#[allow(dead_code)]
mod m02_design_prototype {
    use core::future::Future;
    use core::pin::Pin;
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, SyncSender};

    type DispatchFuture = Pin<Box<dyn Future<Output = Result<(), ()>> + Send + 'static>>;

    trait Server: Send + Sync + 'static {
        fn dispatch(self: Arc<Self>) -> DispatchFuture;
    }

    #[derive(Clone)]
    struct Client {
        core: Arc<ClientCore>,
    }

    struct ClientCore {
        commands: SyncSender<Command>,
    }

    struct Command;

    struct ConnectionActor {
        commands: Receiver<Command>,
        state: ConnectionState,
    }

    struct ConnectionState;

    struct PrototypeServer;

    impl Server for PrototypeServer {
        fn dispatch(self: Arc<Self>) -> DispatchFuture {
            Box::pin(async { Ok(()) })
        }
    }

    fn assert_send_sync<T: Send + Sync>() {}
    fn assert_send<T: Send>(_: &T) {}

    #[test]
    fn client_and_server_future_shapes_are_thread_safe() {
        assert_send_sync::<Client>();
        assert_send_sync::<PrototypeServer>();
        let future = Arc::new(PrototypeServer).dispatch();
        assert_send(&future);
    }
}
