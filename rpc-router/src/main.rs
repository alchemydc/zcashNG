//! zcashNG JSON-RPC router.
//!
//! Listens on a single port and dispatches each incoming JSON-RPC method to
//! either Zebra or Zallet by consulting the merged OpenRPC schema. Designed
//! to be the only host-facing JSON-RPC endpoint for the zcashNG stack — Zebra
//! and Zallet RPC are not published outside the internal Docker network.
//!
//! Hardening features beyond raw multiplex:
//!   * Schemas are discovered with retry+backoff and the server returns 503
//!     until both backends have answered `rpc.discover` at least once.
//!   * A background liveness loop probes both backends every 15s and feeds
//!     `GET /health` (returns 200 + per-backend status JSON when both are
//!     fresh, 503 otherwise).
//!   * Optional HTTP Basic auth on the host-facing port (set ZCASHNG_RPC_USER
//!     and ZCASHNG_RPC_PASSWORD to require credentials).
//!   * `healthcheck` CLI subcommand probes localhost /health and exits 0/1 —
//!     used as the Docker HEALTHCHECK on the distroless image.

use std::{
    env,
    net::SocketAddr,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use clap::{Parser, Subcommand};
use http_body_util::{BodyExt, Full};
use hyper::{
    body::Bytes,
    header::{HeaderName, HeaderValue},
    server::conn::http1,
    service::service_fn,
    Request, Response, StatusCode, Uri,
};
use hyper_util::{
    client::legacy::Client as HyperClient,
    rt::{TokioExecutor, TokioIo},
};
use reqwest::Client as ReqwestClient;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::{net::TcpListener, sync::RwLock, time::sleep};
use tracing::{debug, error, info, warn};

mod defaults;

#[cfg(test)]
mod unit_tests;

#[cfg(test)]
mod integration_tests;

// -----------------------------------------------------------------------------
// CLI
// -----------------------------------------------------------------------------

#[derive(Parser)]
#[command(name = "rpc-router", version, about = "zcashNG JSON-RPC router")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Run the JSON-RPC router (default if no subcommand given).
    Serve,
    /// Probe http://127.0.0.1:<LISTEN_PORT>/health and exit 0 on success, 1 otherwise.
    /// Used as the Docker HEALTHCHECK on the distroless image.
    Healthcheck,
}

// -----------------------------------------------------------------------------
// Configuration
// -----------------------------------------------------------------------------

/// Structure to parse incoming JSON-RPC requests.
#[derive(Deserialize, Debug)]
struct RpcRequest {
    method: String,
}

/// Router configuration. Cloneable so handlers can hold an Arc.
#[derive(Clone)]
struct Config {
    zebra_url: String,
    zallet_url: String,
    /// Credentials the router sends TO Zebra/Zallet. Both empty = no outgoing
    /// auth header is attached (the zcashNG stack runs cookie-disabled by default).
    rpc_user: String,
    rpc_password: String,
    /// Credentials the router REQUIRES from clients. None = anonymous access.
    /// Populated only when both ZCASHNG_RPC_USER and ZCASHNG_RPC_PASSWORD are
    /// non-empty.
    incoming_basic_auth: Option<(String, String)>,
    cors_origin: String,
    listen_port: u16,
}

impl Config {
    fn from_env() -> Self {
        let incoming_user = env::var("ZCASHNG_RPC_USER").unwrap_or_default();
        let incoming_password = env::var("ZCASHNG_RPC_PASSWORD").unwrap_or_default();
        let incoming_basic_auth = if !incoming_user.is_empty() && !incoming_password.is_empty() {
            Some((incoming_user, incoming_password))
        } else {
            None
        };

        Self {
            zebra_url: env::var("ZEBRA_URL").unwrap_or_else(|_| defaults::ZEBRA_URL.to_string()),
            zallet_url: env::var("ZALLET_URL")
                .unwrap_or_else(|_| defaults::ZALLET_URL.to_string()),
            rpc_user: env::var("RPC_USER").unwrap_or_default(),
            rpc_password: env::var("RPC_PASSWORD").unwrap_or_default(),
            incoming_basic_auth,
            cors_origin: env::var("CORS_ORIGIN")
                .unwrap_or_else(|_| defaults::CORS_ORIGIN.to_string()),
            listen_port: env::var("LISTEN_PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(defaults::LISTEN_PORT),
        }
    }
}

// -----------------------------------------------------------------------------
// Schemas + health state
// -----------------------------------------------------------------------------

/// Merged schema and method lists for routing.
#[derive(Clone)]
struct Z3Schema {
    zebra_methods: Vec<Value>,
    zallet_methods: Vec<Value>,
    merged: Value,
}

/// Probe interval after initial schemas load (steady-state liveness).
const LIVENESS_INTERVAL_SECS: u64 = 15;
/// Initial retry delay for schema discovery (doubles per failure).
const STARTUP_BACKOFF_INITIAL_SECS: u64 = 5;
/// Cap on the schema-discovery retry delay.
const STARTUP_BACKOFF_MAX_SECS: u64 = 60;
/// A backend is "fresh" if it answered a probe within this window.
const LIVENESS_FRESHNESS_MS: i64 = 60_000;

/// Shared mutable state read by /health and the JSON-RPC handler.
struct HealthState {
    /// Unix millis of the last successful probe (0 = never).
    last_zebra_ok: AtomicI64,
    last_zallet_ok: AtomicI64,
    /// Schemas are written once when both backends first respond to
    /// rpc.discover; from then on, JSON-RPC dispatch reads them on every
    /// request. None means the router is still starting and returns 503.
    schemas: RwLock<Option<Z3Schema>>,
}

impl HealthState {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            last_zebra_ok: AtomicI64::new(0),
            last_zallet_ok: AtomicI64::new(0),
            schemas: RwLock::new(None),
        })
    }

    /// (zebra_ok, zallet_ok) where ok = answered a probe within LIVENESS_FRESHNESS_MS.
    fn liveness_snapshot(&self) -> (bool, bool) {
        let cutoff = now_ms() - LIVENESS_FRESHNESS_MS;
        (
            self.last_zebra_ok.load(Ordering::Relaxed) > cutoff,
            self.last_zallet_ok.load(Ordering::Relaxed) > cutoff,
        )
    }

    async fn schemas(&self) -> Option<Z3Schema> {
        self.schemas.read().await.clone()
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Background loop that probes both backends. Until schemas are loaded, runs
/// with exponential backoff (5s → 60s cap) and merges the OpenRPC schemas on
/// the first dual-success. After schemas load, drops to steady-state 15s
/// liveness probes that keep /health's freshness timestamps current.
async fn health_loop(config: Arc<Config>, state: Arc<HealthState>) {
    let mut delay_secs = STARTUP_BACKOFF_INITIAL_SECS;
    let mut schemas_loaded = false;

    loop {
        let (zebra_result, zallet_result) = tokio::join!(
            call_rpc_discover(&config.zebra_url, &config.rpc_user, &config.rpc_password),
            call_rpc_discover(&config.zallet_url, &config.rpc_user, &config.rpc_password),
        );

        let zebra_ok = zebra_result.is_ok();
        let zallet_ok = zallet_result.is_ok();

        if zebra_ok {
            state.last_zebra_ok.store(now_ms(), Ordering::Relaxed);
        }
        if zallet_ok {
            state.last_zallet_ok.store(now_ms(), Ordering::Relaxed);
        }

        if !schemas_loaded {
            if let (Ok(zr), Ok(wr)) = (zebra_result, zallet_result) {
                match merge_openrpc_schemas(zr["result"].clone(), wr["result"].clone()) {
                    Ok(s) => {
                        *state.schemas.write().await = Some(s);
                        info!("Backend schemas loaded; router serving JSON-RPC traffic.");
                        schemas_loaded = true;
                        delay_secs = LIVENESS_INTERVAL_SECS;
                    }
                    Err(e) => {
                        error!(
                            "Failed to merge backend schemas: {e}; retrying in {}s",
                            delay_secs
                        );
                        delay_secs = (delay_secs * 2).min(STARTUP_BACKOFF_MAX_SECS);
                    }
                }
            } else {
                warn!(
                    "Schemas not loaded yet (zebra ok: {}, zallet ok: {}); retrying in {}s",
                    zebra_ok, zallet_ok, delay_secs
                );
                delay_secs = (delay_secs * 2).min(STARTUP_BACKOFF_MAX_SECS);
            }
        } else if !zebra_ok || !zallet_ok {
            warn!(
                "Liveness probe failure (zebra ok: {}, zallet ok: {})",
                zebra_ok, zallet_ok
            );
        } else {
            debug!("Liveness probes ok for both backends.");
        }

        sleep(Duration::from_secs(delay_secs)).await;
    }
}

// -----------------------------------------------------------------------------
// HTTP handlers
// -----------------------------------------------------------------------------

/// Forwards the incoming request to the specified target URL. Adds Basic auth
/// only if both rpc_user and rpc_password are non-empty (zcashNG defaults run
/// the backends without auth).
async fn forward_request(
    req: Request<Full<Bytes>>,
    target_url: &str,
    rpc_user: &str,
    rpc_password: &str,
) -> Result<Response<Full<Bytes>>> {
    let client = HyperClient::builder(TokioExecutor::new()).build_http();

    let uri_string = format!(
        "{}{}",
        target_url,
        req.uri()
            .path_and_query()
            .map(|x| x.as_str())
            .unwrap_or("/")
    );
    let uri: Uri = uri_string.parse()?;

    let (parts, body) = req.into_parts();
    let mut new_req = Request::builder()
        .method(parts.method)
        .uri(uri)
        .version(parts.version);

    if !rpc_user.is_empty() || !rpc_password.is_empty() {
        let auth = general_purpose::STANDARD.encode(format!("{}:{}", rpc_user, rpc_password));
        new_req = new_req.header(
            hyper::header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {}", auth))?,
        );
    }

    for (k, v) in parts.headers {
        if let Some(key) = k {
            // Don't blindly forward Authorization — we set our own (or none).
            if key != hyper::header::AUTHORIZATION {
                new_req = new_req.header(key, v);
            }
        }
    }

    let new_req = new_req.body(body)?;
    let res = client.request(new_req).await?;

    let (parts, body) = res.into_parts();
    let body_bytes = body.collect().await?.to_bytes();
    Ok(Response::from_parts(parts, Full::new(body_bytes)))
}

/// Adds CORS headers to the response.
fn add_cors_headers(mut resp: Response<Full<Bytes>>, cors_origin: &str) -> Response<Full<Bytes>> {
    let headers = resp.headers_mut();
    headers.insert(
        HeaderName::from_static("access-control-allow-origin"),
        HeaderValue::from_str(cors_origin).unwrap_or(HeaderValue::from_static("*")),
    );
    for &(name, value) in &[
        ("access-control-allow-methods", "POST, OPTIONS"),
        ("access-control-allow-headers", "Content-Type, Authorization"),
        ("access-control-max-age", "86400"),
    ] {
        headers.insert(
            HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }

    resp
}

/// Builds the response for GET /health. 200 with per-backend status when both
/// are fresh; 503 otherwise. Body is JSON in both cases.
fn build_health_response(state: &HealthState) -> Response<Full<Bytes>> {
    let (zebra_ok, zallet_ok) = state.liveness_snapshot();
    let body = json!({
        "zebra":  if zebra_ok  { "ok" } else { "down" },
        "zallet": if zallet_ok { "ok" } else { "down" },
    });
    let status = if zebra_ok && zallet_ok {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    Response::builder()
        .status(status)
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .expect("health response should be valid")
}

/// Checks the incoming Authorization header against the configured credentials.
/// Returns Some(<401 response>) if rejection is required, None if the request
/// should proceed.
fn check_incoming_auth(
    req: &Request<hyper::body::Incoming>,
    config: &Config,
) -> Option<Response<Full<Bytes>>> {
    let Some((user, password)) = &config.incoming_basic_auth else {
        return None;
    };
    let expected = format!(
        "Basic {}",
        general_purpose::STANDARD.encode(format!("{}:{}", user, password))
    );
    let provided = req
        .headers()
        .get(hyper::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    if provided == Some(expected.as_str()) {
        None
    } else {
        Some(
            Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("WWW-Authenticate", "Basic realm=\"rpc-router\"")
                .body(Full::new(Bytes::from("Unauthorized")))
                .expect("401 response should be valid"),
        )
    }
}

/// Main request handler.
async fn handler(
    req: Request<hyper::body::Incoming>,
    config: Arc<Config>,
    state: Arc<HealthState>,
) -> Result<Response<Full<Bytes>>> {
    // /health is always anonymous-readable so HEALTHCHECK and monitoring work.
    if req.uri().path() == "/health" {
        return Ok(build_health_response(&state));
    }

    // Optional Basic auth gate on everything else.
    if let Some(unauthorized) = check_incoming_auth(&req, &config) {
        return Ok(unauthorized);
    }

    // CORS preflight.
    if req.method() == hyper::Method::OPTIONS {
        return Ok(add_cors_headers(
            Response::builder()
                .status(StatusCode::NO_CONTENT)
                .body(Full::new(Bytes::new()))
                .unwrap(),
            &config.cors_origin,
        ));
    }

    // Only POST for JSON-RPC.
    if req.method() != hyper::Method::POST {
        return Ok(Response::builder()
            .status(StatusCode::METHOD_NOT_ALLOWED)
            .body(Full::new(Bytes::from("Method Not Allowed")))
            .unwrap());
    }

    // 503 until backends have responded at least once.
    let z3 = match state.schemas().await {
        Some(s) => s,
        None => {
            return Ok(Response::builder()
                .status(StatusCode::SERVICE_UNAVAILABLE)
                .header(hyper::header::CONTENT_TYPE, "application/json")
                .body(Full::new(Bytes::from(
                    json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32603,
                            "message": "Router is still discovering backend schemas; retry shortly."
                        },
                        "id": null
                    })
                    .to_string(),
                )))
                .unwrap());
        }
    };

    let (parts, body) = req.into_parts();
    let body_bytes = body.collect().await?.to_bytes();

    let target_url = if let Ok(rpc_req) = serde_json::from_slice::<RpcRequest>(&body_bytes) {
        if rpc_req.method == "rpc.discover" {
            info!("Routing rpc.discover to merged schema");
            return Ok(add_cors_headers(
                Response::builder()
                    .status(StatusCode::OK)
                    .header(hyper::header::CONTENT_TYPE, "application/json")
                    .body(Full::new(Bytes::from(serde_json::to_string(&z3.merged)?)))
                    .expect("z3 merged schema response should be valid"),
                &config.cors_origin,
            ));
        }

        if z3.zebra_methods.iter().any(|m| m["name"] == rpc_req.method) {
            info!("Routing {} to Zebra", rpc_req.method);
            &config.zebra_url
        } else if z3
            .zallet_methods
            .iter()
            .any(|m| m["name"] == rpc_req.method)
        {
            info!("Routing {} to Zallet", rpc_req.method);
            &config.zallet_url
        } else {
            warn!(
                "Method '{}' not in Zebra or Zallet schema; falling back to Zebra",
                rpc_req.method
            );
            &config.zebra_url
        }
    } else {
        warn!("Failed to parse JSON-RPC body; defaulting to Zebra");
        &config.zebra_url
    };

    let new_req = Request::from_parts(parts, Full::new(body_bytes));

    match forward_request(new_req, target_url, &config.rpc_user, &config.rpc_password).await {
        Ok(res) => Ok(add_cors_headers(res, &config.cors_origin)),
        Err(e) => {
            error!("Forwarding error: {}", e);
            Ok(Response::builder()
                .status(StatusCode::BAD_GATEWAY)
                .body(Full::new(Bytes::from(format!("Bad Gateway: {}", e))))
                .unwrap())
        }
    }
}

// -----------------------------------------------------------------------------
// rpc.discover client + schema merging
// -----------------------------------------------------------------------------

/// Calls rpc.discover on the given URL and returns the parsed JSON response.
async fn call_rpc_discover(
    url: &str,
    rpc_user: &str,
    rpc_password: &str,
) -> Result<serde_json::Value> {
    let client = ReqwestClient::new();
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "rpc.discover",
        "params": []
    });

    let mut req_builder = client
        .post(url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_string());
    if !rpc_user.is_empty() || !rpc_password.is_empty() {
        req_builder = req_builder.basic_auth(rpc_user, Some(rpc_password));
    }

    let text = req_builder.send().await?.error_for_status()?.text().await?;
    Ok(serde_json::from_str::<serde_json::Value>(&text)?)
}

/// Extracts the methods array from the OpenRPC schema.
fn extract_methods_array(schema: &Value) -> Vec<Value> {
    schema["methods"].as_array().cloned().unwrap_or_default()
}

/// Annotates each method with its origin server.
fn annotate_methods_with_server(methods: &mut Vec<Value>, server_name: &str) {
    for m in methods {
        if let Some(obj) = m.as_object_mut() {
            obj.insert("x-server".to_string(), json!(server_name));
        } else {
            warn!(
                "Skipping non-object method entry while annotating for {}",
                server_name
            );
        }
    }
}

/// Merges the components.schemas from the given schema into the combined map.
fn merge_components_schemas(schema: &Value, combined: &mut serde_json::Map<String, Value>) {
    if let Some(obj) = schema["components"]["schemas"].as_object() {
        for (k, v) in obj {
            combined.insert(k.clone(), v.clone());
        }
    }
}

/// Merges the OpenRPC schemas from Zebra and Zallet.
fn merge_openrpc_schemas(zebra: Value, zallet: Value) -> Result<Z3Schema> {
    let mut zebra_methods = extract_methods_array(&zebra);
    let mut zallet_methods = extract_methods_array(&zallet);
    annotate_methods_with_server(&mut zebra_methods, "zebra");
    annotate_methods_with_server(&mut zallet_methods, "zallet");

    let mut combined_schemas = serde_json::Map::new();
    merge_components_schemas(&zebra, &mut combined_schemas);
    merge_components_schemas(&zallet, &mut combined_schemas);

    let mut combined_methods = Vec::new();
    combined_methods.extend(zebra_methods.clone());
    combined_methods.extend(zallet_methods.clone());

    let merged = json!({
        "openrpc": zebra["openrpc"].clone(),
        "info": {
            "title":  env!("CARGO_PKG_NAME"),
            "description": env!("CARGO_PKG_DESCRIPTION"),
            "version": env!("CARGO_PKG_VERSION"),
        },
        "servers": [
            { "name": "router",  "url": "http://localhost:8232/" },
        ],
        "methods": combined_methods,
        "components": { "schemas": combined_schemas }
    });

    Ok(Z3Schema {
        zebra_methods,
        zallet_methods,
        merged,
    })
}

// -----------------------------------------------------------------------------
// Entrypoints
// -----------------------------------------------------------------------------

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => {
            tracing_subscriber::fmt::init();
            serve().await
        }
        Command::Healthcheck => healthcheck_cli().await,
    }
}

/// `rpc-router healthcheck` — used as Docker HEALTHCHECK on the distroless image.
async fn healthcheck_cli() -> Result<()> {
    let port = env::var("LISTEN_PORT")
        .ok()
        .and_then(|p| p.parse::<u16>().ok())
        .unwrap_or(defaults::LISTEN_PORT);
    let url = format!("http://127.0.0.1:{}/health", port);
    let client = ReqwestClient::builder()
        .timeout(Duration::from_secs(3))
        .build()?;
    match client.get(&url).send().await {
        Ok(resp) if resp.status().is_success() => Ok(()),
        Ok(resp) => {
            eprintln!("healthcheck: {} returned {}", url, resp.status());
            std::process::exit(1);
        }
        Err(e) => {
            eprintln!("healthcheck: {} unreachable: {}", url, e);
            std::process::exit(1);
        }
    }
}

async fn serve() -> Result<()> {
    let config = Arc::new(Config::from_env());
    let addr = SocketAddr::from(([0, 0, 0, 0], config.listen_port));
    let listener = TcpListener::bind(addr).await?;
    info!("RPC Router listening on {}", addr);
    info!("Playground: {}", defaults::playground_url(addr));
    if config.incoming_basic_auth.is_some() {
        info!("HTTP Basic auth enabled on /, /POST (clients must send ZCASHNG_RPC_USER credentials).");
    } else {
        info!("HTTP Basic auth NOT enabled (set ZCASHNG_RPC_USER + ZCASHNG_RPC_PASSWORD to require credentials).");
    }

    let state = HealthState::new();

    // Spawn the schema-loader + liveness loop in the background.
    let loop_config = config.clone();
    let loop_state = state.clone();
    tokio::spawn(async move { health_loop(loop_config, loop_state).await });

    run(config, listener, state).await
}

/// Accepts connections and dispatches requests. Extracted so integration tests
/// can drive the server directly with a pre-populated HealthState.
async fn run(config: Arc<Config>, listener: TcpListener, state: Arc<HealthState>) -> Result<()> {
    loop {
        let (stream, _) = listener.accept().await?;
        let io = TokioIo::new(stream);
        let config = config.clone();
        let state = state.clone();

        tokio::task::spawn(async move {
            if let Err(err) = http1::Builder::new()
                .serve_connection(
                    io,
                    service_fn(move |req| handler(req, config.clone(), state.clone())),
                )
                .await
            {
                error!("Error serving connection: {:?}", err);
            }
        });
    }
}
