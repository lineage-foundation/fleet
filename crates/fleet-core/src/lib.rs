//! # Fleet Core
//!
//! Shared consensus, comms, and mining primitives for Lineage network nodes.
#![allow(dead_code)]

pub mod active_raft;
pub mod asert;
pub mod block_pipeline;
pub mod bounded_hash_set;
pub mod comms_handler;
pub mod configurations;
pub mod constants;
pub mod db_utils;
pub mod interfaces;
pub mod key_creation;
pub mod miner_pow;
pub mod raft;
pub mod raft_store;
pub mod raft_util;
pub mod threaded_call;
pub mod tracked_utxo;
pub mod transactor;
pub mod unicorn;
pub mod utils;

pub use constants::SANC_LIST_PROD;
pub use interfaces::Rs2JsMsg;
pub use interfaces::{MempoolRequest, MinerInterface, Response, StorageInterface};
pub use utils::LocalEvent;
pub use utils::{
    create_valid_transaction, get_sanction_addresses, get_test_common_unicorn,
    loop_connnect_to_peers_async, loop_wait_connnect_to_peers_async,
    loops_re_connect_disconnect, shutdown_connections, ResponseResult,
};

#[cfg(not(feature = "mock"))]
pub use comms_handler::Node;
