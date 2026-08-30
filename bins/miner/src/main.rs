//! App to run a mining node.

use fleet_core::configurations::{MinerNodeConfig, UserNodeConfig};
use fleet_core::{
    loop_wait_connnect_to_peers_async, loops_re_connect_disconnect, shutdown_connections,
    ResponseResult,
};
use fleet_api::routes;
use fleet_miner::MinerNode;
use fleet_node_common::ExtraNodeParams;
use fleet_user::UserNode;
use clap::{App, Arg, ArgMatches};
use config::{ConfigError, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use tracing::info;

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() {
    tracing_subscriber::fmt::init();
    let matches = clap_app().get_matches();
    run_node(&matches).await;
}

async fn run_node(matches: &ArgMatches<'_>) {
    let (config, user_config) = configuration(load_settings(matches));
    info!("Start node with config {:?}", config);
    let node = MinerNode::new(config, Default::default()).await.unwrap();
    info!("Started node at {}", node.local_address());

    let miner_api_inputs = node.api_inputs();
    let shared_wallet_db = Some(node.get_wallet_db().clone());
    let (node_conn, addrs_to_connect, expected_connected_addrs) = node.connect_info_peers();
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

    // Miner main loop
    let main_loop_handle = tokio::spawn({
        let mut node = node;
        let mut node_conn = node_conn;

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

            shutdown_connections(&mut node_conn).await;
        }
    });

    match user_config {
        Some(config) => {
            let shared_members = ExtraNodeParams {
                shared_wallet_db,
                ..Default::default()
            };

            info!("Start user node with config {config:?}");
            let user_node = UserNode::new(config, shared_members).await.unwrap();
            let api_inputs = (user_node.api_inputs(), miner_api_inputs);
            info!("Started user node at {}", user_node.local_address());

            let (user_node_conn, user_addrs_to_connect, user_expected_connected_addrs) =
                user_node.connect_info_peers();
            let user_local_event_tx = user_node.local_event_tx().clone();
            let threaded_calls_tx = user_node.threaded_call_tx().clone();

            // PERMANENT CONNEXION/DISCONNECTION HANDLING
            let (
                (user_conn_loop_handle, user_stop_re_connect_tx),
                (user_disconn_loop_handle, user_stop_disconnect_tx),
            ) = {
                let (user_re_connect, user_disconnect_test) = loops_re_connect_disconnect(
                    user_node_conn.clone(),
                    user_addrs_to_connect,
                    user_local_event_tx,
                );

                (
                    (tokio::spawn(user_re_connect.0), user_re_connect.1),
                    (tokio::spawn(user_disconnect_test.0), user_disconnect_test.1),
                )
            };

            // Need to connect first so Raft messages can be sent.
            loop_wait_connnect_to_peers_async(
                user_node_conn.clone(),
                user_expected_connected_addrs,
            )
            .await;

            // User main loop
            let user_main_loop_handle = tokio::spawn({
                let mut node = user_node;
                let mut node_conn = user_node_conn;

                async move {
                    node.send_startup_requests().await.unwrap();

                    let mut exit = std::future::pending();
                    while let Some(response) = node.handle_next_event(&mut exit).await {
                        if node.handle_next_event_response(response).await == ResponseResult::Exit {
                            break;
                        }
                    }
                    user_stop_re_connect_tx.send(()).unwrap();
                    user_stop_disconnect_tx.send(()).unwrap();

                    shutdown_connections(&mut node_conn).await;
                }
            });

            // User / Miner combined warp API
            let warp_handle = tokio::spawn({
                let threaded_calls_tx = threaded_calls_tx;
                let (
                    (db, user_node, api_addr, api_tls, api_keys, api_pow_info),
                    (_, miner_node, _, _, _, current_block, _),
                ) = api_inputs;

                info!("Warp API started on port {:?}", api_addr.port());
                info!("");

                let mut bind_address = "0.0.0.0:0".parse::<SocketAddr>().unwrap();
                bind_address.set_port(api_addr.port());

                async move {
                    let serve = warp::serve(routes::miner_node_with_user_routes(
                        api_keys,
                        api_pow_info,
                        current_block,
                        db,
                        miner_node,
                        threaded_calls_tx,
                        user_node,
                    ));
                    if let Some(api_tls) = api_tls {
                        serve
                            .tls()
                            .key(&api_tls.pem_pkcs8_private_keys)
                            .cert(&api_tls.pem_certs)
                            .run(bind_address)
                            .await;
                    } else {
                        serve.run(bind_address).await;
                    }
                }
            });

            let (result, result_user, conn, conn_user, disconn, disconn_user, warp_result) = tokio::join!(
                main_loop_handle,
                user_main_loop_handle,
                conn_loop_handle,
                user_conn_loop_handle,
                disconn_loop_handle,
                user_disconn_loop_handle,
                warp_handle
            );

            result.unwrap();
            conn.unwrap();
            disconn.unwrap();
            result_user.unwrap();
            conn_user.unwrap();
            disconn_user.unwrap();
            warp_result.unwrap();
        }
        None => {
            // Miner warp API
            let warp_handle = tokio::spawn({
                let (db, miner_node, api_addr, api_tls, api_keys, current_block, api_pow_info) =
                    miner_api_inputs;

                info!("Warp API started on port {:?}", api_addr.port());
                info!("");

                let mut bind_address = "0.0.0.0:0".parse::<SocketAddr>().unwrap();
                bind_address.set_port(api_addr.port());

                async move {
                    let serve = warp::serve(routes::miner_node_routes(
                        api_keys,
                        api_pow_info,
                        current_block,
                        db,
                        miner_node,
                    ));
                    if let Some(api_tls) = api_tls {
                        serve
                            .tls()
                            .key(&api_tls.pem_pkcs8_private_keys)
                            .cert(&api_tls.pem_certs)
                            .run(bind_address)
                            .await;
                    } else {
                        serve.run(bind_address).await;
                    }
                }
            });

            let (result, conn, disconn, warp_result) = tokio::join!(
                main_loop_handle,
                conn_loop_handle,
                disconn_loop_handle,
                warp_handle
            );

            result.unwrap();
            conn.unwrap();
            disconn.unwrap();
            warp_result.unwrap();
        }
    }
}

fn clap_app<'a, 'b>() -> App<'a, 'b> {
    App::new("miner")
        .about("Runs a basic miner node.")
        .arg(
            Arg::with_name("config")
                .long("config")
                .short("c")
                .env("CONFIG")
                .help("Run the miner node using the given config file.")
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
            Arg::with_name("mining_api_key")
                .long("mining_api_key")
                .env("MINING_API_KEY")
                .help("Use an API key to participate in mining.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("initial_block_config")
                .long("initial_block_config")
                .env("INITIAL_BLOCK_CONFIG")
                .help("Run the mempool node using the given initial block config file.")
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
            Arg::with_name("api_port")
                .long("api_port")
                .env("API_PORT")
                .help("The port to run the http API from")
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
            Arg::with_name("address_aggregation_limit")
                .long("address_aggregation_limit")
                .env("ADDRESS_AGGREGATION_LIMIT")
                .help("Limit the amount of addresses that can be kept before aggregation is triggered")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("index")
                .short("i")
                .long("index")
                .help("Run the specified miner node index from config file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("with_user_index")
                .long("with_user_index")
                .env("WITH_USER_INDEX")
                .help("Run the specified user node index from config file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("mempool_index")
                .long("mempool_index")
                .help("Endpoint index of a mempool node that the miner should connect to")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("passphrase")
                .long("passphrase")
                .env("PASSPHRASE")
                .help("Enter a password or passphase for the encryption of the Wallet.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("address")
                .long("address")
                .env("ADDRESS")
                .help("Run node index at the given address")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("with_user_address")
                .long("with_user_address")
                .env("WITH_USER_ADDRESS")
                .help("Run the specified user node index from config file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("tls_certificate_override")
                .long("tls_certificate_override")
                .env("TLS_CERTIFICATE")
                .help("Use PEM certificate as a string to use for this node TLS certificate.")
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

fn load_settings(matches: &clap::ArgMatches) -> (config::Config, Option<config::Config>) {
    use fleet_core::config_load::{build_with_env_overrides, node_addresses, rebuild};

    let mut miner_index: usize = 0;
    let mut user_index: usize = 0;

    let setting_file = matches
        .value_of("config")
        .unwrap_or("src/bin/node_settings.toml");
    let tls_setting_file = matches
        .value_of("tls_config")
        .unwrap_or("src/bin/tls_certificates.json");
    let intial_block_setting_file = matches
        .value_of("initial_block_config")
        .unwrap_or("src/bin/initial_block.json");
    let api_setting_file = matches
        .value_of("api_config")
        .unwrap_or("src/bin/api_config.json");

    let mut settings = build_with_env_overrides(|b| {
        Ok(b.set_default("api_keys", Vec::<String>::new())?
            .set_default("miner_mempool_node_idx", 0)?
            .set_default("miner_storage_node_idx", 0)?
            .set_default("user_api_port", 3000)?
            .set_default("miner_api_port", 3000)?
            .set_default("user_api_use_tls", true)?
            .set_default("miner_api_use_tls", true)?
            .set_default("user_node_idx", 0)?
            .set_default("user_mempool_node_idx", 0)?
            .set_default("peer_user_node_idx", 0)?
            .set_default("user_auto_donate", 0)?
            .set_default(
                "user_test_auto_gen_setup",
                default_user_test_auto_gen_setup(),
            )?
            .add_source(config::File::with_name(setting_file))
            .add_source(config::File::with_name(tls_setting_file))
            .add_source(config::File::with_name(intial_block_setting_file))
            .add_source(config::File::with_name(api_setting_file)))
    });

    if let Some(idx) = matches.value_of("index") {
        miner_index = idx.parse::<usize>().unwrap();
    } else if let Some(address) = matches.value_of("address") {
        let mut miner_nodes = node_addresses(&settings, "miner_nodes");

        miner_index = match miner_nodes.iter().position(|a| a == address) {
            Some(i) => i,
            None => {
                miner_nodes.push(address.to_owned());
                miner_nodes.len() - 1
            }
        };
        settings = rebuild(settings, |b| Ok(b.set_override("miner_address", address)?));
    }

    if let Err(ConfigError::NotFound(_)) = settings.get_int("peer_limit") {
        settings = rebuild(settings, |b| Ok(b.set_override("peer_limit", 1000)?));
    }

    if matches.value_of("address").is_none() {
        let miner_nodes = node_addresses(&settings, "miner_nodes");
        let addr = miner_nodes
            .get(miner_index)
            .expect("No miner_nodes entry at the resolved index");
        settings = rebuild(settings, |b| Ok(b.set_override("miner_address", addr.clone())?));
    }

    let mut db_mode = settings.get_table("miner_db_mode").unwrap();
    if let Some(test_idx) = db_mode.get_mut("Test") {
        *test_idx = Value::new(None, miner_index.to_string());
        settings = rebuild(settings, |b| Ok(b.set_override("miner_db_mode", db_mode.clone())?));
    }

    let mut has_user_settings = false;

    if let Some(idx) = matches.value_of("with_user_index") {
        user_index = idx.parse::<usize>().unwrap();
        let db_mode = settings.get_table("miner_db_mode").unwrap();
        settings = rebuild(settings, |b| Ok(b.set_override("user_db_mode", db_mode)?));
        has_user_settings = true;
    } else if let Some(address) = matches.value_of("with_user_address") {
        let mut user_nodes = node_addresses(&settings, "user_nodes");

        user_index = match user_nodes.iter().position(|a| a == address) {
            Some(i) => i,
            None => {
                user_nodes.push(address.to_owned());
                user_nodes.len() - 1
            }
        };
        has_user_settings = true;
    }

    if has_user_settings {
        let user_nodes = node_addresses(&settings, "user_nodes");
        let addr = user_nodes
            .get(user_index)
            .expect("No user_nodes entry at the resolved index");
        settings = rebuild(settings, |b| Ok(b.set_override("user_address", addr.clone())?));

        if let Ok(user_wallet_seeds) = settings.get_array("user_wallet_seeds") {
            settings = rebuild(settings, |b| {
                Ok(b.set_override("user_wallet_seeds", user_wallet_seeds[user_index].clone())?)
            });
        }
    }

    if let Some(mining_api_key) = matches.value_of("mining_api_key") {
        settings = rebuild(settings, |b| Ok(b.set_override("mining_api_key", mining_api_key)?));
    }

    if let Some(address_aggregation_limit) = matches.value_of("address_aggregation_limit") {
        settings = rebuild(settings, |b| {
            Ok(b.set_override("address_aggregation_limit", address_aggregation_limit)?)
        });
    }

    if let Some(certificate) = matches.value_of("tls_certificate_override") {
        let mut tls_config = settings.get_table("tls_config").unwrap();
        tls_config.insert(
            "pem_certificate_override".to_owned(),
            Value::new(None, certificate),
        );
        settings = rebuild(settings, |b| Ok(b.set_override("tls_config", tls_config)?));
    }
    if let Some(key) = matches.value_of("tls_private_key_override") {
        let mut tls_config = settings.get_table("tls_config").unwrap();
        tls_config.insert(
            "pem_pkcs8_private_key_override".to_owned(),
            Value::new(None, key),
        );
        settings = rebuild(settings, |b| Ok(b.set_override("tls_config", tls_config)?));
    }

    if let Some(index) = matches.value_of("mempool_index") {
        settings = rebuild(settings, |b| {
            Ok(b.set_override("miner_mempool_node_idx", index)?
                .set_override("user_mempool_node_idx", index)?)
        });
    }

    if let Some(index) = matches.value_of("passphrase") {
        settings = rebuild(settings, |b| Ok(b.set_override("passphrase", index)?));
    }

    if let Some(index) = matches.value_of("storage_index") {
        settings = rebuild(settings, |b| Ok(b.set_override("miner_storage_node_idx", index)?));
    }

    if let Some(api_port) = matches.value_of("api_port") {
        settings = rebuild(settings, |b| {
            Ok(b.set_override("user_api_port", api_port)?.set_override("miner_api_port", api_port)?)
        });
    }
    if let Some(use_tls) = matches.value_of("api_use_tls") {
        settings = rebuild(settings, |b| {
            Ok(b.set_override("user_api_use_tls", use_tls)?
                .set_override("miner_api_use_tls", use_tls)?)
        });
    }

    let user_settings = has_user_settings.then(|| settings.clone());
    (settings, user_settings)
}

fn configuration(
    settings: (config::Config, Option<config::Config>),
) -> (MinerNodeConfig, Option<UserNodeConfig>) {
    (
        settings.0.try_deserialize().unwrap(),
        settings.1.map(|v| v.try_deserialize().unwrap()),
    )
}

fn default_user_test_auto_gen_setup() -> HashMap<String, Value> {
    let mut value = HashMap::new();
    let zero = config::Value::new(None, 0);
    let empty = config::Value::new(None, Vec::<String>::new());
    value.insert("user_initial_transactions".to_owned(), empty);
    value.insert("user_setup_tx_chunk_size".to_owned(), zero.clone());
    value.insert("user_setup_tx_in_per_tx".to_owned(), zero.clone());
    value.insert("user_setup_tx_max_count".to_owned(), zero);
    value
}
