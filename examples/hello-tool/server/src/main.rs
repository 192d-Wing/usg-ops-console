//! `hello-tool-server`. Env:
//! - `HELLO_LISTEN` (default `127.0.0.1:8090`; keep it on a network only the
//!   gateway can reach, the assertion header is trusted only from there)
//! - `HELLO_TOOL_IDS` (default `hello,evil`)
//! - `HELLO_EVIL_ID` (default `evil`; empty disables the hostile fixture)
//! - `HELLO_WEB_DIST` (default `examples/hello-tool/web/dist`)
//! - `CONSOLE_JWKS_URL` (required, e.g. `http://127.0.0.1:8080/.well-known/jwks.json`)
//! - `CONSOLE_ISSUER` (required, the console origin = assertion `iss`)

use std::process::ExitCode;
use std::sync::Arc;

use hello_tool_server::{Config, Verifier, app};
use tracing_subscriber::EnvFilter;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_owned())
}

fn required(key: &str) -> Result<String, String> {
    std::env::var(key)
        .ok()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

async fn run() -> Result<(), String> {
    let issuer = required("CONSOLE_ISSUER")?;
    let jwks_url = required("CONSOLE_JWKS_URL")?;
    let listen = env_or("HELLO_LISTEN", "127.0.0.1:8090");
    let cfg = Config {
        tool_ids: env_or("HELLO_TOOL_IDS", "hello,evil")
            .split(',')
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect(),
        evil_id: Some(env_or("HELLO_EVIL_ID", "evil")).filter(|s| !s.is_empty()),
        web_dist: env_or("HELLO_WEB_DIST", "examples/hello-tool/web/dist").into(),
    };
    let verifier = Arc::new(Verifier::remote(issuer, jwks_url)?);
    let router = app(&cfg, verifier)?;
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .map_err(|e| format!("bind {listen}: {e}"))?;
    tracing::info!(%listen, tools = ?cfg.tool_ids, "hello-tool-server listening");
    axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .map_err(|e| e.to_string())
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            tracing::error!(error = %e, "hello-tool-server failed");
            ExitCode::FAILURE
        }
    }
}
