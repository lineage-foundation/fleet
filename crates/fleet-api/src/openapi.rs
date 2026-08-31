//! OpenAPI document aggregation and Swagger UI mounting.

use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityScheme};
use utoipa::{Modify, OpenApi};

use crate::error::ApiProblem;
#[allow(unused_imports)]
use crate::v1::balances::{
    __path_get_balances, __path_query_balances, get_balances, query_balances, AddressesQuery, BalancesResponse,
};
#[allow(unused_imports)]
use crate::v1::blockchain::{
    __path_get_blockchain_entry, __path_query_blockchain_entries, get_blockchain_entry, query_blockchain_entries,
    BlockchainEntryResponse, BlockchainItemMetaResponse, KeysQuery,
};
#[allow(unused_imports)]
use crate::v1::blocks::{
    __path_get_block_by_num, __path_get_blocks_batch, __path_get_latest_block, get_block_by_num, get_blocks_batch,
    get_latest_block, LatestBlockResponse,
};
#[allow(unused_imports)]
use crate::v1::debug::{__path_get_debug, get_debug, DebugData, PeerInfo};
#[allow(unused_imports)]
use crate::v1::mining::{__path_get_current_block, get_current_block, CurrentBlockResponse};
#[allow(unused_imports)]
use crate::v1::supply::{__path_get_supply, get_supply, SupplyResponse};
#[allow(unused_imports)]
use crate::v1::transactions::{
    __path_get_outgoing_txs, __path_get_transaction_status, __path_query_transaction_status, get_outgoing_txs,
    get_transaction_status, query_transaction_status, HashesQuery, OutgoingTxsResponse, TxStatusResponse,
    TxStatusTypeResponse,
};
#[allow(unused_imports)]
use crate::v1::wallet::{
    __path_get_keypairs, __path_get_wallet_info, __path_post_new_address, __path_put_passphrase,
    get_keypairs, get_wallet_info, post_new_address, put_passphrase, ChangePassphraseRequest, KeypairsResponse,
    NewAddressResponse, WalletInfoQuery, WalletInfoResponse,
};

/// Aggregates every `/v1` operation ported so far into one OpenAPI 3.1 document,
/// served at `/v1/openapi.json` with Swagger UI at `/v1/docs`.
///
/// Not every operation listed here is mounted on every node's router (e.g. the
/// `blocks`/`blockchain-entries` resources are storage-only); this single document
/// describes the API surface as a whole. Per-node OpenAPI subsets are a future
/// refinement.
#[derive(OpenApi)]
#[openapi(
    paths(
        get_debug,
        get_latest_block,
        get_block_by_num,
        get_blocks_batch,
        get_blockchain_entry,
        query_blockchain_entries,
        get_supply,
        get_balances,
        query_balances,
        get_transaction_status,
        query_transaction_status,
        get_outgoing_txs,
        get_wallet_info,
        get_keypairs,
        post_new_address,
        put_passphrase,
        get_current_block,
    ),
    components(schemas(
        DebugData,
        PeerInfo,
        LatestBlockResponse,
        BlockchainEntryResponse,
        BlockchainItemMetaResponse,
        KeysQuery,
        SupplyResponse,
        BalancesResponse,
        AddressesQuery,
        TxStatusResponse,
        TxStatusTypeResponse,
        HashesQuery,
        OutgoingTxsResponse,
        WalletInfoResponse,
        WalletInfoQuery,
        KeypairsResponse,
        NewAddressResponse,
        ChangePassphraseRequest,
        CurrentBlockResponse,
        ApiProblem,
    )),
    modifiers(&SecurityAddon),
    info(title = "fleet REST API", description = "REST API for fleet nodes, served under /v1.")
)]
pub struct ApiDoc;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        let components = openapi.components.get_or_insert_with(Default::default);
        components.add_security_scheme(
            "api_key",
            SecurityScheme::ApiKey(ApiKey::Header(ApiKeyValue::new("x-api-key"))),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openapi_is_valid_3_1_and_lists_exactly_the_mounted_paths() {
        let spec = ApiDoc::openapi();
        let value = serde_json::to_value(&spec).expect("openapi doc serializes");

        assert_eq!(value["openapi"], "3.1.0");

        let mut paths: Vec<&str> = value["paths"]
            .as_object()
            .expect("paths object")
            .keys()
            .map(String::as_str)
            .collect();
        paths.sort_unstable();

        assert_eq!(
            paths,
            vec![
                "/v1/balances",
                "/v1/balances/query",
                "/v1/blockchain-entries/query",
                "/v1/blockchain-entries/{key}",
                "/v1/blocks",
                "/v1/blocks/latest",
                "/v1/blocks/{num}",
                "/v1/debug",
                "/v1/mining/current-block",
                "/v1/supply",
                "/v1/transactions/outgoing",
                "/v1/transactions/status",
                "/v1/transactions/status:query",
                "/v1/wallet",
                "/v1/wallet/addresses",
                "/v1/wallet/keypairs",
                "/v1/wallet/passphrase",
            ]
        );
    }

    #[test]
    fn openapi_declares_the_api_key_security_scheme() {
        let spec = ApiDoc::openapi();
        let components = spec.components.expect("components present");
        assert!(components.security_schemes.contains_key("api_key"));
    }
}
