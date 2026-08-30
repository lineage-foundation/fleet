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
}
