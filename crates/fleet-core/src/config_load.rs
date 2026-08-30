//! Helpers for building [`config::Config`] without deprecated `Config::merge` / `Config::set` / `Config::set_default`.

use config::builder::DefaultState;
use config::{Config, ConfigBuilder, Environment};

pub fn build(
    f: impl FnOnce(ConfigBuilder<DefaultState>) -> Result<ConfigBuilder<DefaultState>, config::ConfigError>,
) -> Config {
    f(Config::builder()).unwrap().build().unwrap()
}

pub fn rebuild(
    base: Config,
    f: impl FnOnce(ConfigBuilder<DefaultState>) -> Result<ConfigBuilder<DefaultState>, config::ConfigError>,
) -> Config {
    f(Config::builder().add_source(base))
        .unwrap()
        .build()
        .unwrap()
}

/// Build a Config from the caller's defaults + file sources, then layer `LINEAGE_*`
/// environment overrides on top. Nested keys use `__`; values are type-parsed so numeric
/// and boolean overrides deserialize correctly. Highest-precedence layer below explicit
/// CLI flags (which callers apply afterward via `rebuild`).
pub fn build_with_env_overrides(
    f: impl FnOnce(ConfigBuilder<DefaultState>) -> Result<ConfigBuilder<DefaultState>, config::ConfigError>,
) -> Config {
    f(Config::builder())
        .unwrap()
        .add_source(
            Environment::with_prefix("LINEAGE")
                .prefix_separator("_")
                .separator("__")
                .try_parsing(true),
        )
        .build()
        .unwrap()
}

/// Read a node-address list from config under `key`, accepting either an array (of
/// `{ address = "..." }` tables or bare address strings) or a single comma-separated
/// address string (env override, e.g. `LINEAGE_MINER_NODES=http://a:1,http://b:2`).
/// Returns the addresses in order; whitespace trimmed, empty entries dropped.
/// Returns an empty Vec if the key is absent.
pub fn node_addresses(settings: &Config, key: &str) -> Vec<String> {
    if let Ok(arr) = settings.get_array(key) {
        arr.into_iter()
            .filter_map(|v| {
                if let Ok(table) = v.clone().into_table() {
                    table.get("address").map(|a| a.to_string())
                } else {
                    v.into_string().ok()
                }
            })
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else if let Ok(s) = settings.get_string(key) {
        s.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::sync::Mutex;

    // Env is process-global; serialize env-mutating tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[derive(Deserialize, Debug, PartialEq)]
    struct Sample {
        mempool_api_port: u16,
        jurisdiction: String,
    }

    #[test]
    fn env_overrides_file_scalar_and_parses_type() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LINEAGE_MEMPOOL_API_PORT", "3999");
        let cfg = build_with_env_overrides(|b| {
            Ok(b.set_default("mempool_api_port", 3002)?
                .set_default("jurisdiction", "US")?)
        });
        let s: Sample = cfg.try_deserialize().unwrap();
        std::env::remove_var("LINEAGE_MEMPOOL_API_PORT");
        assert_eq!(s, Sample { mempool_api_port: 3999, jurisdiction: "US".into() });
    }

    #[test]
    fn node_addresses_reads_array_of_tables() {
        let mut node_a = std::collections::HashMap::new();
        node_a.insert("address".to_owned(), "http://a:1".to_owned());
        let mut node_b = std::collections::HashMap::new();
        node_b.insert("address".to_owned(), "http://b:2".to_owned());

        let cfg = build(|b| {
            Ok(b.set_default(
                "miner_nodes",
                vec![
                    config::Value::new(None, node_a),
                    config::Value::new(None, node_b),
                ],
            )?)
        });

        assert_eq!(
            node_addresses(&cfg, "miner_nodes"),
            vec!["http://a:1".to_string(), "http://b:2".to_string()]
        );
    }

    #[test]
    fn node_addresses_reads_comma_separated_env_string() {
        let _g = ENV_LOCK.lock().unwrap();
        std::env::set_var("LINEAGE_MINER_NODES", "http://a:1,http://b:2");
        let cfg = build_with_env_overrides(|b| Ok(b));
        std::env::remove_var("LINEAGE_MINER_NODES");

        assert_eq!(
            node_addresses(&cfg, "miner_nodes"),
            vec!["http://a:1".to_string(), "http://b:2".to_string()]
        );
    }

    #[test]
    fn node_addresses_absent_key_returns_empty() {
        let cfg = build(|b| Ok(b));
        assert_eq!(node_addresses(&cfg, "miner_nodes"), Vec::<String>::new());
    }
}
