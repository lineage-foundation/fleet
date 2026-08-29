use crate::utils::create_valid_transaction;
use crate::wallet::WalletDb;
use tw_chain::primitives::{asset::Asset, transaction::OutPoint};

/// Creates a "fake" transaction to save to the local wallet
/// for testing. The transaction will contain 4 tokens
///
/// NOTE: This is a test util function
/// ### Arguments
///
/// * `wallet_db`    - &WalletDb object. Reference to a wallet database
pub async fn create_and_save_fake_to_wallet(
    wallet_db: &mut WalletDb,
) -> Result<(), Box<dyn std::error::Error>> {
    let (final_address, address_keys) = wallet_db.generate_payment_address();
    let (receiver_addr, _) = wallet_db.generate_payment_address();

    let (t_hash, _payment_tx) = create_valid_transaction(
        "00000",
        0,
        &receiver_addr,
        &address_keys.public_key,
        &address_keys.secret_key,
    );
    let tx_out_p = OutPoint::new(t_hash, 0);
    let payment_to_save = Asset::token_u64(4000);
    let payments = vec![(tx_out_p.clone(), payment_to_save, final_address, 0)];
    wallet_db
        .save_usable_payments_to_wallet(payments, 0, false)
        .await
        .unwrap();

    Ok(())
}
