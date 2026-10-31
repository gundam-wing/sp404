//! `sp404-server` binds `SP404_BIND` (default `127.0.0.1:4040`).

#![forbid(unsafe_code)]

use std::process::ExitCode;

use sp404_server::{config_from_env, router};

#[tokio::main]
async fn main() -> ExitCode {
    let config = match config_from_env() {
        Ok(config) => config,
        Err(message) => {
            eprintln!("error: {message}");
            return ExitCode::from(1);
        }
    };
    let listener = match tokio::net::TcpListener::bind(&config.bind).await {
        Ok(listener) => listener,
        Err(bind_err) => {
            eprintln!("error: bind {}: {bind_err}", config.bind);
            return ExitCode::from(1);
        }
    };
    match listener.local_addr() {
        Ok(addr) => eprintln!("sp404-server listening on {addr}"),
        Err(addr_err) => {
            eprintln!("error: local address: {addr_err}");
            return ExitCode::from(1);
        }
    }
    let app = router(config);
    if let Err(serve_err) = axum::serve(listener, app).await {
        eprintln!("error: {serve_err}");
        return ExitCode::from(1);
    }
    ExitCode::SUCCESS
}
