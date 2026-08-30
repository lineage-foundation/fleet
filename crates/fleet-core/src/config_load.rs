//! Helpers for building [`config::Config`] without deprecated `Config::merge` / `Config::set` / `Config::set_default`.

use config::builder::DefaultState;
use config::{Config, ConfigBuilder};

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
