use capnp_rpc::{new_client, rpc_twoparty_capnp, twoparty, RpcSystem};
use futures::AsyncReadExt;
use std::rc::Rc;

pub mod ping_capnp;

struct PingImpl;

impl ping_capnp::ping::Server for PingImpl {
    async fn ping(
        self: Rc<Self>,
        params: ping_capnp::ping::PingParams,
        mut results: ping_capnp::ping::PingResults,
    ) -> Result<(), capnp::Error> {
        results.get().set_value(params.get()?.get_value() + 1);
        Ok(())
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let iterations = std::env::args()
        .nth(1)
        .ok_or("usage: rust-rpc-benchmark ITERATIONS")?
        .parse::<u64>()?;

    tokio::task::LocalSet::new()
        .run_until(async move {
            let (client_stream, server_stream) = tokio::io::duplex(1024 * 1024);

            let (server_reader, server_writer) =
                tokio_util::compat::TokioAsyncReadCompatExt::compat(server_stream).split();
            let server_network = twoparty::VatNetwork::new(
                futures::io::BufReader::new(server_reader),
                futures::io::BufWriter::new(server_writer),
                rpc_twoparty_capnp::Side::Server,
                Default::default(),
            );
            let server_cap: ping_capnp::ping::Client = new_client(PingImpl);
            let server_rpc = RpcSystem::new(Box::new(server_network), Some(server_cap.client));
            tokio::task::spawn_local(server_rpc);

            let (client_reader, client_writer) =
                tokio_util::compat::TokioAsyncReadCompatExt::compat(client_stream).split();
            let client_network = twoparty::VatNetwork::new(
                futures::io::BufReader::new(client_reader),
                futures::io::BufWriter::new(client_writer),
                rpc_twoparty_capnp::Side::Client,
                Default::default(),
            );
            let mut client_rpc = RpcSystem::new(Box::new(client_network), None);
            let ping: ping_capnp::ping::Client =
                client_rpc.bootstrap(rpc_twoparty_capnp::Side::Server);
            tokio::task::spawn_local(client_rpc);

            let mut checksum = 0_u64;
            for index in 0..iterations {
                let mut request = ping.ping_request();
                request.get().set_value(index);
                checksum ^= request.send().promise.await?.get()?.get_value();
            }

            println!("{checksum}");
            Ok::<(), Box<dyn std::error::Error>>(())
        })
        .await
}
