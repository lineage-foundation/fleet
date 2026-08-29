use crate::db_utils::{CustomDbSpec, SimpleDb};
use crate::wallet::WalletDb;

/// Extra params for Node construction
#[derive(Default)]
pub struct ExtraNodeParams {
    pub db: Option<SimpleDb>,
    pub raft_db: Option<SimpleDb>,
    pub wallet_db: Option<SimpleDb>,
    pub shared_wallet_db: Option<WalletDb>,
    pub custom_wallet_spec: Option<CustomDbSpec>,
    pub disable_tcp_listener: bool,
}
