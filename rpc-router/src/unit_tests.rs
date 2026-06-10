use super::*;
use serial_test::serial;

fn zebra_schema() -> Value {
    json!({
        "openrpc": "1.2.6",
        "methods": [
            { "name": "getblock", "params": [] },
            { "name": "getinfo",  "params": [] }
        ],
        "components": {
            "schemas": {
                "BlockHash": { "type": "string" }
            }
        }
    })
}

fn zallet_schema() -> Value {
    json!({
        "openrpc": "1.2.6",
        "methods": [
            { "name": "getwalletinfo", "params": [] },
            { "name": "z_sendmany",    "params": [] }
        ],
        "components": {
            "schemas": {
                "WalletInfo": { "type": "object" }
            }
        }
    })
}

// --- extract_methods_array ---

#[test]
fn test_extract_methods_array_returns_methods() {
    let schema = zebra_schema();
    let methods = extract_methods_array(&schema);
    assert_eq!(methods.len(), 2);
    assert_eq!(methods[0]["name"], "getblock");
    assert_eq!(methods[1]["name"], "getinfo");
}

#[test]
fn test_extract_methods_array_missing_key_returns_empty() {
    let schema = json!({ "openrpc": "1.2.6" });
    let methods = extract_methods_array(&schema);
    assert!(methods.is_empty());
}

// --- annotate_methods_with_server ---

#[test]
fn test_annotate_methods_sets_x_server() {
    let mut methods = extract_methods_array(&zebra_schema());
    annotate_methods_with_server(&mut methods, "zebra");
    for m in &methods {
        assert_eq!(m["x-server"], "zebra");
    }
}

#[test]
fn test_annotate_methods_non_object_entry_does_not_panic() {
    let schema = json!({ "methods": ["not-an-object", { "name": "getblock" }] });
    let mut methods = extract_methods_array(&schema);
    annotate_methods_with_server(&mut methods, "zebra");
    assert_eq!(methods[1]["x-server"], "zebra");
}

// --- merge_components_schemas ---

#[test]
fn test_merge_components_schemas_combines_keys() {
    let mut combined = serde_json::Map::new();
    merge_components_schemas(&zebra_schema(), &mut combined);
    merge_components_schemas(&zallet_schema(), &mut combined);
    assert!(combined.contains_key("BlockHash"));
    assert!(combined.contains_key("WalletInfo"));
}

#[test]
fn test_merge_components_schemas_missing_components_is_noop() {
    let schema = json!({ "methods": [] });
    let mut combined = serde_json::Map::new();
    merge_components_schemas(&schema, &mut combined);
    assert!(combined.is_empty());
}

#[test]
fn test_merge_components_schemas_last_write_wins_on_conflict() {
    let schema_a = json!({ "components": { "schemas": { "Foo": { "type": "string" } } } });
    let schema_b = json!({ "components": { "schemas": { "Foo": { "type": "integer" } } } });
    let mut combined = serde_json::Map::new();
    merge_components_schemas(&schema_a, &mut combined);
    merge_components_schemas(&schema_b, &mut combined);
    assert_eq!(combined["Foo"]["type"], "integer");
}

// --- merge_openrpc_schemas ---

#[test]
fn test_merge_openrpc_schemas_combined_method_count() {
    let z3 = merge_openrpc_schemas(zebra_schema(), zallet_schema()).unwrap();
    assert_eq!(z3.zebra_methods.len(), 2);
    assert_eq!(z3.zallet_methods.len(), 2);
    assert_eq!(z3.merged["methods"].as_array().unwrap().len(), 4);
}

#[test]
fn test_merge_openrpc_schemas_methods_annotated() {
    let z3 = merge_openrpc_schemas(zebra_schema(), zallet_schema()).unwrap();
    for m in &z3.zebra_methods {
        assert_eq!(m["x-server"], "zebra");
    }
    for m in &z3.zallet_methods {
        assert_eq!(m["x-server"], "zallet");
    }
}

#[test]
fn test_merge_openrpc_schemas_components_merged() {
    let z3 = merge_openrpc_schemas(zebra_schema(), zallet_schema()).unwrap();
    let schemas = &z3.merged["components"]["schemas"];
    assert!(schemas.get("BlockHash").is_some());
    assert!(schemas.get("WalletInfo").is_some());
}

#[test]
fn test_merge_openrpc_schemas_info_fields_present() {
    let z3 = merge_openrpc_schemas(zebra_schema(), zallet_schema()).unwrap();
    assert!(z3.merged["info"]["title"].is_string());
    assert!(z3.merged["info"]["version"].is_string());
}

// --- Config::from_env ---

#[test]
#[serial]
fn test_config_from_env_outgoing_auth_defaults_empty() {
    env::remove_var("RPC_USER");
    env::remove_var("RPC_PASSWORD");
    let config = Config::from_env();
    assert_eq!(config.rpc_user, "");
    assert_eq!(config.rpc_password, "");
}

#[test]
#[serial]
fn test_config_from_env_reads_outgoing_rpc_credentials_when_set() {
    env::set_var("RPC_USER", "alice");
    env::set_var("RPC_PASSWORD", "s3cr3t");
    let config = Config::from_env();
    assert_eq!(config.rpc_user, "alice");
    assert_eq!(config.rpc_password, "s3cr3t");
    env::remove_var("RPC_USER");
    env::remove_var("RPC_PASSWORD");
}

#[test]
#[serial]
fn test_config_from_env_incoming_auth_none_when_unset() {
    env::remove_var("ZCASHNG_RPC_USER");
    env::remove_var("ZCASHNG_RPC_PASSWORD");
    let config = Config::from_env();
    assert!(config.incoming_basic_auth.is_none());
}

#[test]
#[serial]
fn test_config_from_env_incoming_auth_some_when_both_set() {
    env::set_var("ZCASHNG_RPC_USER", "alice");
    env::set_var("ZCASHNG_RPC_PASSWORD", "s3cr3t");
    let config = Config::from_env();
    assert_eq!(
        config.incoming_basic_auth,
        Some(("alice".to_string(), "s3cr3t".to_string()))
    );
    env::remove_var("ZCASHNG_RPC_USER");
    env::remove_var("ZCASHNG_RPC_PASSWORD");
}

#[test]
#[serial]
fn test_config_from_env_incoming_auth_none_when_only_one_set() {
    env::set_var("ZCASHNG_RPC_USER", "alice");
    env::remove_var("ZCASHNG_RPC_PASSWORD");
    let config = Config::from_env();
    assert!(config.incoming_basic_auth.is_none());
    env::remove_var("ZCASHNG_RPC_USER");
}

// --- HealthState ---

#[test]
fn test_health_state_snapshot_false_when_never_probed() {
    let state = HealthState::new();
    let (zebra_ok, zallet_ok) = state.liveness_snapshot();
    assert!(!zebra_ok);
    assert!(!zallet_ok);
}

#[test]
fn test_health_state_snapshot_true_when_recent_probe() {
    let state = HealthState::new();
    state.last_zebra_ok.store(now_ms(), Ordering::Relaxed);
    state.last_zallet_ok.store(now_ms(), Ordering::Relaxed);
    let (zebra_ok, zallet_ok) = state.liveness_snapshot();
    assert!(zebra_ok);
    assert!(zallet_ok);
}

#[test]
fn test_health_state_snapshot_false_when_probe_stale() {
    let state = HealthState::new();
    // 2 minutes ago — older than LIVENESS_FRESHNESS_MS (60s).
    let stale = now_ms() - 120_000;
    state.last_zebra_ok.store(stale, Ordering::Relaxed);
    state.last_zallet_ok.store(stale, Ordering::Relaxed);
    let (zebra_ok, zallet_ok) = state.liveness_snapshot();
    assert!(!zebra_ok);
    assert!(!zallet_ok);
}

// The test for Config requires PartialEq on the auth tuple comparison; we
// don't derive it on Config itself (no need), so the equality assertion above
// uses Option<(String,String)> directly.
