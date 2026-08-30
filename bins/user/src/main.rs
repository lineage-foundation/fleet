//! App to run a user node.

use fleet_core::configurations::UserNodeConfig;
use fleet_core::interfaces::{UserApiRequest, UserRequest, UtxoFetchType};
use fleet_core::{
    loop_wait_connnect_to_peers_async, loops_re_connect_disconnect, shutdown_connections,
    ResponseResult,
};
use fleet_api::routes;
use fleet_user::UserNode;
use clap::{App, Arg, ArgMatches};
use config::{ConfigError, Value};
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::time::{self, Duration};
use tracing::{info, trace, warn};

//================== BIN CONSTANTS ==================//

/// Interval between requested UTXO realignment, in seconds
const UTXO_REALIGN_INTERVAL: u64 = 120;

/// Default user API port
const DEFAULT_USER_API_PORT: i64 = 3000;

/// Default peer limit
const DEFAULT_PEER_LIMIT: i64 = 1000;

//===================================================//

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() {
    tracing_subscriber::fmt::init();
    let matches = clap_app().get_matches();
    run_node(&matches).await;
}

async fn run_node(matches: &ArgMatches<'_>) {
    let config = configuration(load_settings(matches));

    info!("Starting node with config: {config:?}");
    info!("");

    let node = UserNode::new(config, Default::default()).await.unwrap();

    info!("Started node at {}", node.local_address());

    let (node_conn, addrs_to_connect, expected_connected_addrs) = node.connect_info_peers();
    let local_event_tx = node.local_event_tx().clone();
    let threaded_calls_tx = node.threaded_call_tx().clone();
    let api_inputs = node.api_inputs();
    let peer_node = node.get_node().clone();
    let wallet_db = node.get_wallet_db().clone();

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

    // REQUEST HANDLING
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

    // Warp API
    let warp_handle = tokio::spawn({
        let (db, node, api_addr, api_tls, api_keys, api_pow_info) = api_inputs;
        let threaded_calls_tx = threaded_calls_tx.clone();

        info!("Warp API started on port {:?}", api_addr.port());
        info!("");

        let mut bind_address = "0.0.0.0:0".parse::<SocketAddr>().unwrap();
        bind_address.set_port(api_addr.port());

        async move {
            let serve = warp::serve(routes::user_node_routes(
                api_keys,
                api_pow_info,
                db,
                node,
                threaded_calls_tx,
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

    // Rolling update of the running total
    let update_handle = tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(UTXO_REALIGN_INTERVAL));

        loop {
            interval.tick().await;
            trace!("Updating running total in loop");
            let known_addresses = wallet_db.get_known_addresses();

            let request = UserRequest::UserApi(UserApiRequest::UpdateWalletFromUtxoSet {
                address_list: UtxoFetchType::AnyOf(known_addresses),
            });

            if let Err(e) = peer_node.inject_next_event(peer_node.local_address(), request) {
                warn!("route:update_running_total error: {:?}", e);
            }
        }
    });

    let (main_result, warp_result, conn, disconn, update_result) = tokio::join!(
        main_loop_handle,
        warp_handle,
        conn_loop_handle,
        disconn_loop_handle,
        update_handle
    );
    main_result.unwrap();
    warp_result.unwrap();
    conn.unwrap();
    disconn.unwrap();
    update_result.unwrap();
}

fn clap_app<'a, 'b>() -> App<'a, 'b> {
    App::new("user")
        .about("Runs a basic User node.")
        .arg(
            Arg::with_name("config")
                .long("config")
                .short("c")
                .env("CONFIG")
                .help("Run the user node using the given config file.")
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
            Arg::with_name("auto_donate")
                .long("auto_donate")
                .help("The amount of tokens to send any requester")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("index")
                .short("i")
                .long("index")
                .help("Run the specified user node index from config file")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("mempool_index")
                .long("mempool_index")
                .help("Endpoint index of a mempool node that the user should connect to")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("passphrase")
                .long("passphrase")
                .help("Enter a password or passphase for the encryption of the Wallet.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("address")
                .long("address")
                .help("Run node index at the given address")
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

fn load_settings(matches: &clap::ArgMatches) -> config::Config {
    use fleet_core::config_load::{build_with_env_overrides, node_addresses, rebuild};
    let mut settings;
    let mut node_index = 0;
    let setting_file = matches
        .value_of("config")
        .unwrap_or("src/bin/node_settings.toml");
    let intial_block_setting_file = matches
        .value_of("initial_block_config")
        .unwrap_or("src/bin/initial_block.json");
    let tls_setting_file = matches
        .value_of("tls_config")
        .unwrap_or("src/bin/tls_certificates.json");
    let api_setting_file = matches
        .value_of("api_config")
        .unwrap_or("src/bin/api_config.json");

    settings = build_with_env_overrides(|b| {
        Ok(b.set_default("api_keys", Vec::<String>::new())?
            .set_default("user_api_port", DEFAULT_USER_API_PORT)?
            .set_default("user_api_use_tls", true)?
            .set_default("user_mempool_node_idx", 0)?
            .set_default("user_auto_donate", 0)?
            .set_default(
                "user_test_auto_gen_setup",
                default_user_test_auto_gen_setup(),
            )?
            .add_source(config::File::with_name(setting_file))
            .add_source(config::File::with_name(intial_block_setting_file))
            .add_source(config::File::with_name(tls_setting_file))
            .add_source(config::File::with_name(api_setting_file)))
    });

    if let Err(ConfigError::NotFound(_)) = settings.get_int("peer_limit") {
        settings = rebuild(settings, |b| Ok(b.set_override("peer_limit", DEFAULT_PEER_LIMIT)?));
    }

    if let Some(idx) = matches.value_of("index") {
        node_index = idx.parse::<usize>().unwrap();
    } else if let Some(address) = matches.value_of("address") {
        let mut user_nodes = node_addresses(&settings, "user_nodes");

        node_index = match user_nodes.iter().position(|a| a == address) {
            Some(i) => i,
            None => {
                user_nodes.push(address.to_owned());
                user_nodes.len() - 1
            }
        };
        settings = rebuild(settings, |b| Ok(b.set_override("user_address", address)?));
    }

    if matches.value_of("address").is_none() {
        let user_nodes = node_addresses(&settings, "user_nodes");
        let addr = user_nodes
            .get(node_index)
            .expect("No user_nodes entry at the resolved index");
        settings = rebuild(settings, |b| Ok(b.set_override("user_address", addr.clone())?));
    }

    let mut db_mode = settings.get_table("user_db_mode").unwrap();
    if let Some(test_idx) = db_mode.get_mut("Test") {
        let index = node_index
            + test_idx
                .clone()
                .try_deserialize::<usize>()
                .expect("user_db_mode.Test must be usize-compatible");
        *test_idx = Value::new(None, index.to_string());
        settings = rebuild(settings, |b| Ok(b.set_override("user_db_mode", db_mode)?));
    }

    if let Ok(user_wallet_seeds) = settings.get_array("user_wallet_seeds") {
        settings = rebuild(settings, |b| {
            Ok(b.set_override("user_wallet_seeds", user_wallet_seeds[node_index].clone())?)
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

    if let Some(api_port) = matches.value_of("api_port") {
        settings = rebuild(settings, |b| Ok(b.set_override("user_api_port", api_port)?));
    }

    if let Some(index) = matches.value_of("mempool_index") {
        settings = rebuild(settings, |b| Ok(b.set_override("user_mempool_node_idx", index)?));
    }

    if let Some(index) = matches.value_of("passphrase") {
        settings = rebuild(settings, |b| Ok(b.set_override("passphrase", index)?));
    }

    if let Some(api_port) = matches.value_of("auto_donate") {
        settings = rebuild(settings, |b| Ok(b.set_override("user_auto_donate", api_port)?));
    }
    if let Some(use_tls) = matches.value_of("api_use_tls") {
        settings = rebuild(settings, |b| Ok(b.set_override("user_api_use_tls", use_tls)?));
    }

    settings
}

fn configuration(settings: config::Config) -> UserNodeConfig {
    settings.try_deserialize().unwrap()
}

fn default_user_test_auto_gen_setup() -> HashMap<String, Value> {
    let mut value = HashMap::new();
    let zero = Value::new(None, 0);
    let empty = Value::new(None, Vec::<String>::new());
    value.insert("user_initial_transactions".to_owned(), empty);
    value.insert("user_setup_tx_chunk_size".to_owned(), zero.clone());
    value.insert("user_setup_tx_in_per_tx".to_owned(), zero.clone());
    value.insert("user_setup_tx_max_count".to_owned(), zero);
    value
}
