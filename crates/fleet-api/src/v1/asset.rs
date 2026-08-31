//! `ApiAsset` — the typed, OpenAPI-described rendering of a chain asset, and
//! `TxOutputSummary`, used in write-endpoint responses (create transactions, and — in
//! later phases — items and payments). A typed representation keeps the asset shape in
//! the OpenAPI spec rather than an opaque `Object`.

use std::collections::BTreeMap;

use serde::Serialize;
use tw_chain::primitives::asset::Asset;
use utoipa::ToSchema;

/// A chain asset, tagged by `kind` so each variant is fully described by the schema.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ApiAsset {
    /// A quantity of the native token.
    Token {
        /// Token amount, in raw token units.
        amount: u64,
    },
    /// An item asset.
    Item {
        /// The number of items.
        amount: u64,
        /// The genesis transaction hash the item derives from, when known.
        genesis_hash: Option<String>,
        /// Optional item metadata.
        metadata: Option<String>,
    },
}

impl From<&Asset> for ApiAsset {
    fn from(asset: &Asset) -> Self {
        match asset {
            Asset::Token(amount) => ApiAsset::Token { amount: amount.0 },
            Asset::Item(item) => ApiAsset::Item {
                amount: item.amount,
                genesis_hash: item.genesis_hash.clone(),
                metadata: item.metadata.clone(),
            },
        }
    }
}

/// A single transaction output: the destination address and the asset sent to it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TxOutputSummary {
    /// The output's destination address (empty when the output has no script public key).
    pub address: String,
    /// The asset held by this output.
    pub asset: ApiAsset,
}

/// A per-transaction output summary keyed by transaction hash.
pub type TxOutputMap = BTreeMap<String, TxOutputSummary>;
