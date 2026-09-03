//! App to run a storage node.

use fleet_core::configurations::StorageNodeConfig;
use fleet_storage::StorageNode;
use fleet_core::{
    loop_wait_connnect_to_peers_async, loops_re_connect_disconnect, shutdown_connections,
    ResponseResult,
};
use fleet_api::ApiState;
use clap::{App, Arg, ArgMatches};
use config::ConfigError;
use std::net::SocketAddr;
use tracing::info;

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() {
    tracing_subscriber::fmt::init();
    let matches = clap_app().get_matches();
    run_node(&matches).await;
}

async fn run_node(matches: &ArgMatches<'_>) {
    let config = configuration(load_settings(matches));

    info!("Start node with config {config:?}");
    let node = StorageNode::new(config, Default::default()).await.unwrap();

    info!("Started node at {}", node.local_address());

    let (node_conn, addrs_to_connect, expected_connected_addrs) = node.connect_info_peers();
    let api_inputs = node.api_inputs();

    let local_event_tx = node.local_event_tx().clone();

    // PERMANENT CONNEXION/DISCONNECTION HANDLING
    let ((conn_loop_handle, stop_re_connect_tx), (disconn_loop_handle, stop_disconnect_tx)) = {
        let (re_connect, disconnect_test) =
            loops_re_connect_disconnect(node_conn.clone(), addrs_to_connect, local_event_tx);

        (
            (tokio::spawn(re_connect.0), re_connect.1),
            (tokio::spawn(disconnect_test.0), disconnect_test.1),
        )
    };

    // Need to connect first so Raft messages can be sent.
    loop_wait_connnect_to_peers_async(node_conn.clone(), expected_connected_addrs).await;

    // RAFT HANDLING
    let raft_loop_handle = {
        let raft_loop = node.raft_loop();
        tokio::spawn(async move {
            info!("Peer connect complete, start Raft");
            raft_loop.await;
            info!("Raft complete");
        })
    };

    // REST API
    let api_handle = tokio::spawn({
        let (db, api_addr, api_tls, api_keys, api_pow_info) = api_inputs;

        info!("REST API started on port {:?}", api_addr.port());
        info!("");

        let mut bind_address = "[::]:0".parse::<SocketAddr>().unwrap();
        bind_address.set_port(api_addr.port());
        let node_conn_debug = node_conn.clone();

        async move {
            let app = fleet_api::storage_router(ApiState::storage(
                node_conn_debug,
                db,
                api_keys,
                api_pow_info,
            ));

            if let Some(api_tls) = api_tls {
                let config = match axum_server::tls_rustls::RustlsConfig::from_pem(
                    api_tls.pem_certs.into_bytes(),
                    api_tls.pem_pkcs8_private_keys.into_bytes(),
                )
                .await
                {
                    Ok(config) => config,
                    Err(e) => {
                        tracing::error!("Failed to load TLS config for REST API: {e:?}");
                        return;
                    }
                };
                if let Err(e) = axum_server::bind_rustls(bind_address, config)
                    .serve(app.into_make_service())
                    .await
                {
                    tracing::error!("REST API server error: {e:?}");
                }
            } else if let Err(e) = axum_server::bind(bind_address)
                .serve(app.into_make_service())
                .await
            {
                tracing::error!("REST API server error: {e:?}");
            }
        }
    });

    // REQUEST HANDLING
    let main_loop_handle = tokio::spawn({
        let mut node = node;
        let mut node_conn = node_conn.clone();

        async move {
            node.send_startup_requests().await.unwrap();

            let mut exit = std::future::pending();
            while let Some(response) = node.handle_next_event(&mut exit).await {
                if node.handle_next_event_response(response).await == ResponseResult::Exit {
                    break;
                }
            }
            stop_re_connect_tx.send(()).unwrap();
            stop_disconnect_tx.send(()).unwrap();

            node.close_raft_loop().await;
            shutdown_connections(&mut node_conn).await;
        }
    });

    let (main, api, raft, conn, disconn) = tokio::join!(
        main_loop_handle,
        api_handle,
        raft_loop_handle,
        conn_loop_handle,
        disconn_loop_handle
    );

    main.unwrap();
    api.unwrap();
    raft.unwrap();
    conn.unwrap();
    disconn.unwrap();
}

fn clap_app<'a, 'b>() -> App<'a, 'b> {
    App::new("storage")
        .about("Runs a basic storage node.")
        .arg(
            Arg::with_name("config")
                .long("config")
                .short("c")
                .env("CONFIG")
                .help("Run the storage node using the given config file.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("tls_config")
                .long("tls_config")
                .env("TLS_CONFIG")
                .help("Use file to provide tls configuration options.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("api_config")
                .long("api_config")
                .env("API_CONFIG")
                .help("Use file to provide api configuration options.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("index")
                .short("i")
                .long("index")
                .help("Run the specified storage node index from config file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("api_port")
                .short("p")
                .long("api_port")
                .env("API_PORT")
                .help("Run the API for the storage node as the specified port")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("api_use_tls")
                .long("api_use_tls")
                .env("API_USE_TLS")
                .help("Whether to use TLS for API: 0 to disable")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("tls_private_key_override")
                .long("tls_private_key_override")
                .env("TLS_PRIVATE_KEY")
                .help("Use PKCS8 private key as a string to use for this node TLS certificate.")
                .takes_value(true),
        )
}

fn load_settings(matches: &clap::ArgMatches) -> config::Config {
    use fleet_core::config_load::{build_with_env_overrides, rebuild};

    let setting_file = matches
        .value_of("config")
        .unwrap_or("src/bin/node_settings.toml");
    let tls_setting_file = matches
        .value_of("tls_config")
        .unwrap_or("src/bin/tls_certificates.json");
    let api_setting_file = matches
        .value_of("api_config")
        .unwrap_or("src/bin/api_config.json");

    let mut settings = build_with_env_overrides(|b| {
        Ok(b.set_default("api_keys", Vec::<String>::new())?
            .set_default("storage_node_idx", 0)?
            .set_default("storage_raft", 0)?
            .set_default("storage_api_port", 3001)?
            .set_default("storage_api_use_tls", true)?
            .set_default("storage_raft_tick_timeout", 10)?
            .set_default("storage_catchup_duration", 1000)?
            .add_source(config::File::with_name(setting_file))
            .add_source(config::File::with_name(tls_setting_file))
            .add_source(config::File::with_name(api_setting_file)))
    });

    if let Err(ConfigError::NotFound(_)) = settings.get_int("peer_limit") {
        settings = rebuild(settings, |b| Ok(b.set_override("peer_limit", 1000)?));
    }

    if let Some(port) = matches.value_of("api_port") {
        settings = rebuild(settings, |b| Ok(b.set_override("storage_api_port", port)?));
    }
    if let Some(use_tls) = matches.value_of("api_use_tls") {
        settings = rebuild(settings, |b| Ok(b.set_override("storage_api_use_tls", use_tls)?));
    }

    if let Some(index) = matches.value_of("index") {
        let mut db_mode = settings.get_table("storage_db_mode").unwrap();
        let update_db_mode = if let Some(test_idx) = db_mode.get_mut("Test") {
            *test_idx = config::Value::new(None, index);
            true
        } else {
            false
        };
        settings = if update_db_mode {
            rebuild(settings, |b| {
                Ok(b.set_override("storage_node_idx", index)?
                    .set_override("storage_db_mode", db_mode)?)
            })
        } else {
            rebuild(settings, |b| Ok(b.set_override("storage_node_idx", index)?))
        };
    }

    if let Some(key) = matches.value_of("tls_private_key_override") {
        let mut tls_config = settings.get_table("tls_config").unwrap();
        tls_config.insert(
            "pem_pkcs8_private_key_override".to_owned(),
            config::Value::new(None, key),
        );
        settings = rebuild(settings, |b| Ok(b.set_override("tls_config", tls_config)?));
    }

    settings
}

fn configuration(settings: config::Config) -> StorageNodeConfig {
    let mut settings: StorageNodeConfig = settings.try_deserialize().unwrap();

    // todo: patch this at the point of usage or leave it here?
    if let Some(height) = settings.activation_height_asert {
        if height < 2 {
            settings.activation_height_asert = Some(2);
        }
    }

    settings
}
