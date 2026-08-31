fn main() {
    if let Err(error) = capnp_cli::run_env() {
        eprintln!("capnp-cli: {error}");
        std::process::exit(2);
    }
}
