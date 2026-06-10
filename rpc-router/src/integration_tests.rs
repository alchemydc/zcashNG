use httpmock::prelude::*;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::net::TcpListener;

use super::*;

// --- Mock backend helpers ---

async fn start_zebra_mock() -> MockServer {
    let server = MockServer::start_async().await;

    server
        .mock_async(|when, then| {
            when.method(POST).body_contains("rpc.discover");
            then.status(200).json_body(json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {
                    "openrpc": "1.2.6",
                    "info": { "title": "Zebra", "version": "1.0.0" },
                    "methods": [
                        { "name": "getblock",  "params": [] },
                        { "name": "getinfo",   "params": [] }
                    ],
                    "components": { "schemas": { "BlockHash": { "type": "string" } } }
                }
            }));
        })
        .await;

    server
        .mock_async(|when, then| {
            when.method(POST);
            then.status(200)
                .json_body(json!({ "jsonrpc": "2.0", "id": 1, "result": "zebra-response" }));
        })
        .await;

    server
}

async fn start_zallet_mock() -> MockServer {
    let server = MockServer::start_async().await;

    server
        .mock_async(|when, then| {
            when.method(POST).body_contains("rpc.discover");
            then.status(200).json_body(json!({
                "jsonrpc": "2.0", "id": 1,
                "result": {
                    "openrpc": "1.2.6",
                    "info": { "title": "Zallet", "version": "1.0.0" },
                    "methods": [
                        { "name": "getwalletinfo", "params": [] },
                        { "name": "z_sendmany",    "params": [] }
                    ],
                    "components": { "schemas": { "WalletInfo": { "type": "object" } } }
                }
            }));
        })
        .await;

    server
        .mock_async(|when, then| {
            when.method(POST);
            then.status(200)
                .json_body(json!({ "jsonrpc": "2.0", "id": 1, "result": "zallet-response" }));
        })
        .await;

    server
}

// --- Router startup helper ---

struct RouterHandle {
    pub port: u16,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RouterHandle {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn test_config(zebra_url: &str, zallet_url: &str) -> Arc<Config> {
    Arc::new(Config {
        zebra_url: zebra_url.to_string(),
        zallet_url: zallet_url.to_string(),
        rpc_user: String::new(),
        rpc_password: String::new(),
        incoming_basic_auth: None,
        cors_origin: "*".to_string(),
        listen_port: 0,
    })
}

fn test_config_with_auth(
    zebra_url: &str,
    zallet_url: &str,
    user: &str,
    password: &str,
) -> Arc<Config> {
    Arc::new(Config {
        zebra_url: zebra_url.to_string(),
        zallet_url: zallet_url.to_string(),
        rpc_user: String::new(),
        rpc_password: String::new(),
        incoming_basic_auth: Some((user.to_string(), password.to_string())),
        cors_origin: "*".to_string(),
        listen_port: 0,
    })
}

/// Pre-populates HealthState with schemas built from the live mocks, so tests
/// don't race the background loader.
async fn start_router_with_state(config: Arc<Config>, state: Arc<HealthState>) -> RouterHandle {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let task = tokio::spawn(async move {
        if let Err(e) = run(config, listener, state).await {
            eprintln!("Router error in test: {}", e);
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    RouterHandle { port, task }
}

async fn start_router(zebra_url: &str, zallet_url: &str) -> RouterHandle {
    let config = test_config(zebra_url, zallet_url);
    let state = HealthState::new();

    // Eagerly load schemas + mark backends fresh, so tests can hit JSON-RPC immediately.
    let z = call_rpc_discover(&config.zebra_url, &config.rpc_user, &config.rpc_password)
        .await
        .unwrap()["result"]
        .clone();
    let w = call_rpc_discover(&config.zallet_url, &config.rpc_user, &config.rpc_password)
        .await
        .unwrap()["result"]
        .clone();
    *state.schemas.write().await = Some(merge_openrpc_schemas(z, w).unwrap());
    state.last_zebra_ok.store(now_ms(), Ordering::Relaxed);
    state.last_zallet_ok.store(now_ms(), Ordering::Relaxed);

    start_router_with_state(config, state).await
}

// --- Tests ---

#[tokio::test]
async fn test_health_returns_200_with_per_backend_status_when_both_fresh() {
    let zebra = start_zebra_mock().await;
    let zallet = start_zallet_mock().await;
    let router = start_router(&zebra.base_url(), &zallet.base_url()).await;

    let resp = Client::new()
        .get(format!("http://127.0.0.1:{}/health", router.port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["zebra"], "ok");
    assert_eq!(body["zallet"], "ok");
}

#[tokio::test]
async fn test_health_returns_503_when_no_probes_have_completed() {
    let zebra = start_zebra_mock().await;
    let zallet = start_zallet_mock().await;
    // Empty HealthState: no last_*_ok timestamps, no schemas.
    let state = HealthState::new();
    let config = test_config(&zebra.base_url(), &zallet.base_url());
    let router = start_router_with_state(config, state).await;

    let resp = Client::new()
        .get(format!("http://127.0.0.1:{}/health", router.port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 503);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["zebra"], "down");
    assert_eq!(body["zallet"], "down");
}

#[tokio::test]
async fn test_jsonrpc_returns_503_until_schemas_load() {
    let zebra = start_zebra_mock().await;
    let zallet = start_zallet_mock().await;
    let state = HealthState::new(); // schemas: None
    let config = test_config(&zebra.base_url(), &zallet.base_url());
    let router = start_router_with_state(config, state).await;

    let resp = Client::new()
        .post(format!("http://127.0.0.1:{}/", router.port))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "getblock", "params": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 503);
    let body: Value = resp.json().await.unwrap();
    assert!(body["error"]["message"].as_str().unwrap().contains("schemas"));
}

#[tokio::test]
async fn test_non_post_returns_405() {
    let zebra = start_zebra_mock().await;
    let zallet = start_zallet_mock().await;
    let router = start_router(&zebra.base_url(), &zallet.base_url()).await;

    let resp = Client::new()
        .get(format!("http://127.0.0.1:{}/", router.port))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn test_cors_preflight_returns_204_with_headers() {
    let zebra = start_zebra_mock().await;
    let zallet = start_zallet_mock().await;
    let router = start_router(&zebra.base_url(), &zallet.base_url()).await;

    let resp = Client::new()
        .request(
            reqwest::Method::OPTIONS,
            format!("http://127.0.0.1:{}/", router.port),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 204);
    assert!(resp.headers().contains_key("access-control-allow-origin"));
    assert!(resp.headers().contains_key("access-control-allow-methods"));
}

#[tokio::test]
async fn test_rpc_discover_returns_merged_schema() {
    let zebra = start_zebra_mock().await;
    let zallet = start_zallet_mock().await;
    let router = start_router(&zebra.base_url(), &zallet.base_url()).await;

    let resp = Client::new()
        .post(format!("http://127.0.0.1:{}/", router.port))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "rpc.discover", "params": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);

    let body: Value = resp.json().await.unwrap();
    let methods = body["methods"].as_array().unwrap();
    let names: Vec<&str> = methods.iter().map(|m| m["name"].as_str().unwrap()).collect();

    assert!(names.contains(&"getblock"));
    assert!(names.contains(&"getinfo"));
    assert!(names.contains(&"getwalletinfo"));
    assert!(names.contains(&"z_sendmany"));
}

#[tokio::test]
async fn test_zebra_method_routing() {
    let zebra = start_zebra_mock().await;
    let zallet = start_zallet_mock().await;
    let router = start_router(&zebra.base_url(), &zallet.base_url()).await;

    let resp = Client::new()
        .post(format!("http://127.0.0.1:{}/", router.port))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "getblock", "params": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["result"], "zebra-response");
}

#[tokio::test]
async fn test_zallet_method_routing() {
    let zebra = start_zebra_mock().await;
    let zallet = start_zallet_mock().await;
    let router = start_router(&zebra.base_url(), &zallet.base_url()).await;

    let resp = Client::new()
        .post(format!("http://127.0.0.1:{}/", router.port))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "getwalletinfo", "params": [] }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["result"], "zallet-response");
}

#[tokio::test]
async fn test_unknown_method_falls_back_to_zebra() {
    let zebra = start_zebra_mock().await;
    let zallet = start_zallet_mock().await;
    let router = start_router(&zebra.base_url(), &zallet.base_url()).await;

    let resp = Client::new()
        .post(format!("http://127.0.0.1:{}/", router.port))
        .json(
            &json!({ "jsonrpc": "2.0", "id": 1, "method": "z_getaddressforaccount", "params": [] }),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["result"], "zebra-response");
}

#[tokio::test]
async fn test_incoming_basic_auth_rejects_anonymous() {
    let zebra = start_zebra_mock().await;
    let zallet = start_zallet_mock().await;
    let config = test_config_with_auth(&zebra.base_url(), &zallet.base_url(), "alice", "s3cr3t");

    let state = HealthState::new();
    let z = call_rpc_discover(&config.zebra_url, &config.rpc_user, &config.rpc_password)
        .await
        .unwrap()["result"]
        .clone();
    let w = call_rpc_discover(&config.zallet_url, &config.rpc_user, &config.rpc_password)
        .await
        .unwrap()["result"]
        .clone();
    *state.schemas.write().await = Some(merge_openrpc_schemas(z, w).unwrap());
    state.last_zebra_ok.store(now_ms(), Ordering::Relaxed);
    state.last_zallet_ok.store(now_ms(), Ordering::Relaxed);

    let router = start_router_with_state(config, state).await;

    // No Authorization header → 401.
    let resp = Client::new()
        .post(format!("http://127.0.0.1:{}/", router.port))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "getblock", "params": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    assert!(resp.headers().contains_key("www-authenticate"));

    // Correct credentials → 200.
    let resp = Client::new()
        .post(format!("http://127.0.0.1:{}/", router.port))
        .basic_auth("alice", Some("s3cr3t"))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "getblock", "params": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Wrong credentials → 401.
    let resp = Client::new()
        .post(format!("http://127.0.0.1:{}/", router.port))
        .basic_auth("alice", Some("wrong"))
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": "getblock", "params": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);

    // /health is exempt — anonymous GET still works.
    let resp = Client::new()
        .get(format!("http://127.0.0.1:{}/health", router.port))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
}
