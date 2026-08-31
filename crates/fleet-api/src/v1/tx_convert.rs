//! Client-transaction DTOs and conversion machinery, ported from the legacy fleet-api
//! handlers. These types let a client submit a transaction as JSON (with hex-encoded
//! signatures/pubkeys/stack bytes) and have it converted into a `tw_chain::Transaction`,
//! and the reverse: turning a stored/serialized `Transaction` back into the same JSON
//! shape.

use fleet_core::utils::{decode_pub_key, decode_signature, StringError};
use serde::de::{Error, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::fmt;
use tw_chain::crypto::sign_ed25519::{PublicKey, Signature};
use tw_chain::primitives::druid::DdeValues;
use tw_chain::primitives::transaction::{OutPoint, Transaction, TxIn, TxOut};
use tw_chain::script::lang::Script;
use tw_chain::script::{OpCodes, StackEntry};
use utoipa::ToSchema;

/// Stack entry enum which stores Signature and PubKey items as hex strings
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub enum PrettyStackEntry {
    #[schema(value_type = Object)]
    Op(OpCodes),
    Signature(#[serde(deserialize_with = "hex_string_or_bytes")] String),
    PubKey(#[serde(deserialize_with = "hex_string_or_bytes")] String),
    Num(usize),
    Bytes(#[serde(deserialize_with = "hex_string_or_bytes")] String),
}

impl PrettyStackEntry {
    fn to_internal(self) -> Result<StackEntry, StringError> {
        match self {
            Self::Op(op) => Ok(StackEntry::Op(op)),
            Self::Signature(data) => Ok(StackEntry::Signature(
                Signature::from_slice(
                    hex::decode(data)
                        .map_err(|e| StringError(e.to_string()))?
                        .as_slice(),
                )
                .ok_or(StringError(String::default()))?,
            )),
            Self::PubKey(data) => Ok(StackEntry::PubKey(
                PublicKey::from_slice(
                    hex::decode(data)
                        .map_err(|e| StringError(e.to_string()))?
                        .as_slice(),
                )
                .ok_or(StringError(String::default()))?,
            )),
            Self::Num(val) => Ok(StackEntry::Num(val)),
            Self::Bytes(data) => Ok(StackEntry::Bytes(data)),
        }
    }

    fn from_internal(entry: StackEntry) -> Self {
        match entry {
            StackEntry::Op(op) => Self::Op(op),
            StackEntry::Signature(signature) => Self::Signature(hex::encode(signature.as_ref())),
            StackEntry::PubKey(pubkey) => Self::PubKey(hex::encode(pubkey.as_ref())),
            StackEntry::Num(val) => Self::Num(val),
            StackEntry::Bytes(data) => Self::Bytes(data),
        }
    }
}

/// Information needed for the creaion of TxIn script.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub enum CreateTxInScript {
    #[allow(non_camel_case_types)]
    //unfortunately, this has to be lower-case in order to ensure that we can deserialize the JSON
    // format returned by /transactions_by_key and similar API routes
    stack(Vec<PrettyStackEntry>),
    Pay2PkH {
        /// Data to sign
        signable_data: Option<String>,
        /// Hex encoded signature
        signature: String,
        /// Hex encoded complete public key
        public_key: String,
        /// Optional address version field
        address_version: Option<u64>,
    },
}

/// Information needed for the creaion of TxIn.
/// This API would change if types are modified.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateTxIn {
    /// The previous_out to use
    #[schema(value_type = Object)]
    pub previous_out: Option<OutPoint>,
    /// script info
    pub script_signature: Option<CreateTxInScript>,
}

/// Information necessary for the creation of a Transaction
/// This API would change if types are modified.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CreateTransaction {
    /// String to sign in each inputs
    pub inputs: Vec<CreateTxIn>,
    #[schema(value_type = Vec<Object>)]
    pub outputs: Vec<TxOut>,
    pub version: usize,
    #[schema(value_type = Option<Vec<Object>>)]
    pub fees: Option<Vec<TxOut>>,
    #[schema(value_type = Object)]
    pub druid_info: Option<DdeValues>,
}

/// A Transaction which has been serialized to JSON.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct JsonSerializedTransaction {
    pub txn_hash_hex: String,
    pub txn_hex: String,
}

/// Expect optional field
pub fn with_opt_field<T>(field: Option<T>, e: &str) -> Result<T, StringError> {
    field.ok_or_else(|| StringError(e.to_owned()))
}

/// Deserializer for hex strings which accepts both hex string literals and arrays of bytes.
fn hex_string_or_bytes<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    struct HexStringOrBytes();

    impl<'de> Visitor<'de> for HexStringOrBytes {
        type Value = String;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("hex string or byte array")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: Error,
        {
            // Validate that the hex string can be decoded
            hex::decode(value).map_err(E::custom)?;

            Ok(value.to_owned())
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut elts = Vec::new();
            while let Some(elt) = seq.next_element::<u8>()? {
                elts.push(elt);
            }
            Ok(hex::encode(elts))
        }
    }

    deserializer.deserialize_any(HexStringOrBytes())
}

/// Create a `Transaction` from a `CreateTransaction`
pub fn to_transaction(data: CreateTransaction) -> Result<Transaction, StringError> {
    let CreateTransaction {
        inputs,
        outputs,
        version,
        druid_info,
        fees,
    } = data;

    let inputs = {
        let mut tx_ins = Vec::new();
        for i in inputs {
            let previous_out = with_opt_field(i.previous_out, "Invalid previous_out")?;
            let script_signature = with_opt_field(i.script_signature, "Invalid script_signature")?;

            let tx_in = match script_signature {
                CreateTxInScript::stack(stack) => TxIn {
                    previous_out: Some(previous_out),
                    script_signature: Script::from(
                        stack
                            .into_iter()
                            .map(PrettyStackEntry::to_internal)
                            .collect::<Result<Vec<_>, _>>()?,
                    ),
                },
                CreateTxInScript::Pay2PkH {
                    signable_data,
                    signature,
                    public_key,
                    address_version,
                } => {
                    let final_signable_data = if let Some(sd) = signable_data {
                        sd
                    } else {
                        "".to_string()
                    };

                    let signature =
                        with_opt_field(decode_signature(&signature).ok(), "Invalid signature")?;
                    let public_key =
                        with_opt_field(decode_pub_key(&public_key).ok(), "Invalid public_key")?;

                    TxIn {
                        previous_out: Some(previous_out),
                        script_signature: Script::pay2pkh(
                            final_signable_data,
                            signature,
                            public_key,
                            address_version,
                        ),
                    }
                }
            };

            tx_ins.push(tx_in);
        }
        tx_ins
    };

    Ok(Transaction {
        inputs,
        outputs,
        version,
        fees: fees.unwrap_or_default(),
        druid_info,
    })
}

/// Create a `CreateTransaction` from a hex string representing a serialized `Transaction`
pub fn from_hex_transaction(data: String) -> Result<CreateTransaction, StringError> {
    let bytes = hex::decode(data).map_err(|e| StringError(e.to_string()))?;
    let tx = bincode::deserialize::<Transaction>(bytes.as_slice()).map_err(|e| StringError(e.to_string()))?;
    Ok(from_transaction(tx))
}

/// Create a `CreateTransaction` from a hex string representing a serialized `Transaction`
fn from_transaction(tx: Transaction) -> CreateTransaction {
    let Transaction {
        inputs,
        outputs,
        version,
        fees,
        druid_info,
    } = tx;

    let inputs = {
        let mut tx_ins = Vec::new();
        for i in inputs {
            //TODO: determine if the transaction is P2PKH or something else (?)
            tx_ins.push(CreateTxIn {
                previous_out: i.previous_out,
                script_signature: Some(CreateTxInScript::stack(
                    i.script_signature
                        .stack
                        .into_iter()
                        .map(PrettyStackEntry::from_internal)
                        .collect::<Vec<_>>(),
                )),
            });
        }
        tx_ins
    };

    CreateTransaction {
        inputs,
        outputs,
        version,
        fees: Some(fees),
        druid_info,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tw_chain::primitives::asset::{Asset, TokenAmount};

    #[test]
    fn to_transaction_then_from_hex_transaction_roundtrips_outputs_and_version() {
        let output = TxOut {
            value: Asset::Token(TokenAmount(42)),
            locktime: 0,
            script_public_key: Some("some_address".to_owned()),
        };
        let create_tx = CreateTransaction {
            inputs: Vec::new(),
            outputs: vec![output],
            version: 1,
            fees: None,
            druid_info: None,
        };

        let tx = to_transaction(create_tx).expect("valid transaction");
        let bytes = bincode::serialize(&tx).expect("serializable transaction");
        let hex_tx = hex::encode(bytes);

        let round_tripped = from_hex_transaction(hex_tx).expect("valid hex transaction");

        assert_eq!(round_tripped.version, 1);
        assert_eq!(round_tripped.outputs, tx.outputs);
    }

    #[test]
    fn to_transaction_errors_on_missing_previous_out() {
        let create_tx = CreateTransaction {
            inputs: vec![CreateTxIn {
                previous_out: None,
                script_signature: Some(CreateTxInScript::stack(Vec::new())),
            }],
            outputs: Vec::new(),
            version: 1,
            fees: None,
            druid_info: None,
        };

        let result = to_transaction(create_tx);

        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), StringError("Invalid previous_out".to_owned()));
    }
}
