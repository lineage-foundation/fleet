//! App to run a pre-launch node.

use fleet::configurations::PreLaunchNodeConfig;
use fleet::PreLaunchNode;
use fleet::{
    loop_wait_connnect_to_peers_async, loops_re_connect_disconnect, shutdown_connections,
    ResponseResult,
};
use clap::{App, Arg, ArgMatches};
use config::ConfigError;
use tracing::info;

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() {
    tracing_subscriber::fmt::init();
    let matches = clap_app().get_matches();
    run_node(&matches).await;
}

pub async fn run_node(matches: &ArgMatches<'_>) {
    let config = configuration(load_settings(matches));

    info!("Start node with config {config:?}");
    let node = PreLaunchNode::new(config, Default::default())
        .await
        .unwrap();

    info!("Started node at {}", node.local_address());

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

    let (main, conn, disconn) =
        tokio::join!(main_loop_handle, conn_loop_handle, disconn_loop_handle);

    main.unwrap();
    conn.unwrap();
    disconn.unwrap();
}

pub fn clap_app<'a, 'b>() -> App<'a, 'b> {
    App::new("pre_launch")
        .about("Runs a pre_launch node.")
        .arg(
            Arg::with_name("config")
                .long("config")
                .short("c")
                .help("Run the storage node using the given config file.")
                .takes_value(true),
        )
        .arg(
            Arg::with_name("tls_config")
                .long("tls_config")
                .help("Use file to provide tls configuration options.")
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
            Arg::with_name("type")
                .long("type")
                .help("Run the upgrade for type (mempool, storage)")
                .takes_value(true)
                .required(true),
        )
        .arg(
            Arg::with_name("tls_private_key_override")
                .long("tls_private_key_override")
                .env("ABLOCK_TLS_PRIVATE_KEY")
                .help("Use PKCS8 private key as a string to use for this node TLS certificate.")
                .takes_value(true),
        )
}

fn load_settings(matches: &clap::ArgMatches) -> config::Config {
    use fleet_core::config_load::{build, rebuild};

    let setting_file = matches
        .value_of("config")
        .unwrap_or("src/bin/node_settings.toml");
    let tls_setting_file = matches
        .value_of("tls_config")
        .unwrap_or("src/bin/tls_certificates.json");

    let mut settings = build(|b| {
        Ok(b.set_default("storage_node_idx", 0)?
            .set_default("mempool_node_idx", 0)?
            .add_source(config::File::with_name(setting_file))
            .add_source(config::File::with_name(tls_setting_file)))
    });

    if let Err(ConfigError::NotFound(_)) = settings.get_int("peer_limit") {
        settings = rebuild(settings, |b| Ok(b.set_override("peer_limit", 1000)?));
    }

    if let Some(index) = matches.value_of("index") {
        let mut mempool_db_mode = settings.get_table("mempool_db_mode").unwrap();
        if let Some(test_idx) = mempool_db_mode.get_mut("Test") {
            *test_idx = config::Value::new(None, index);
        }
        let mut storage_db_mode = settings.get_table("storage_db_mode").unwrap();
        if let Some(test_idx) = storage_db_mode.get_mut("Test") {
            *test_idx = config::Value::new(None, index);
        }
        settings = rebuild(settings, |b| {
            Ok(b.set_override("mempool_node_idx", index)?
                .set_override("mempool_db_mode", mempool_db_mode)?
                .set_override("storage_node_idx", index)?
                .set_override("storage_db_mode", storage_db_mode)?)
        });
    }

    if let Some(key) = matches.value_of("tls_private_key_override") {
        let mut tls_config = settings.get_table("tls_config").unwrap();
        tls_config.insert(
            "pem_pkcs8_private_key_override".to_owned(),
            config::Value::new(None, key),
        );
        settings = rebuild(settings, |b| Ok(b.set_override("tls_config", tls_config)?));
    }

    let node_type = match matches.value_of("type").unwrap() {
        "mempool" => "Mempool",
        "storage" => "Storage",
        v => panic!("expect type mempool or storage: {}", v),
    };
    rebuild(settings, |b| Ok(b.set_override("node_type", node_type)?))
}

fn configuration(settings: config::Config) -> PreLaunchNodeConfig {
    settings.try_deserialize().unwrap()
}
