//! Runs the game server: `cake-server [--listen <addr:port>] [--client-dir <dir>]`. The client
//! build in `--client-dir` is served as is, and its `cakes/` are the cakes rounds are played on.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use cake_server::catalog::Catalog;
use cake_server::room::{Room, RoomConfig};

const DEFAULT_LISTEN: &str = "127.0.0.1:8080";
const DEFAULT_CLIENT_DIR: &str = "build/client";

struct Args {
    listen: SocketAddr,
    client_dir: PathBuf,
}

fn main() -> ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("cake-server: {message}");
            return ExitCode::from(2);
        }
    };
    let catalog = match Catalog::load(&args.client_dir.join("cakes")) {
        Ok(catalog) => catalog,
        Err(error) => {
            eprintln!("cake-server: {error}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("{} cakes from {}", catalog.get_len(), args.client_dir.display());
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("cake-server: starting the async runtime: {error}");
            return ExitCode::FAILURE;
        }
    };
    let room = Room::create(RoomConfig::default(), catalog);
    let result = runtime.block_on(async {
        let listening = |error: std::io::Error| format!("listening on {}: {error}", args.listen);
        let listener = tokio::net::TcpListener::bind(args.listen).await.map_err(listening)?;
        eprintln!("listening on http://{}", listener.local_addr().map_err(listening)?);
        cake_server::serve(listener, args.client_dir, room).await.map_err(|error| error.to_string())
    });
    if let Err(message) = result {
        eprintln!("cake-server: {message}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut listen = DEFAULT_LISTEN.to_string();
    let mut client_dir = PathBuf::from(DEFAULT_CLIENT_DIR);
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag}: missing value"))?;
        match flag.as_str() {
            "--listen" => listen = value,
            "--client-dir" => client_dir = PathBuf::from(value),
            _ => return Err(format!("unknown argument {flag:?}")),
        }
    }
    let listen = listen.parse().map_err(|error| format!("--listen {listen:?}: {error}"))?;
    Ok(Args { listen, client_dir })
}
