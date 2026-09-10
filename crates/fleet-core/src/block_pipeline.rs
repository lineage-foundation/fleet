use crate::configurations::UnicornFixedInfo;
use crate::constants::{MINER_PARTICIPATION_UN, WINNING_MINER_UN};
use crate::interfaces::WinningPoWInfo;
use crate::raft_util::RaftContextKey;
use crate::unicorn::{construct_seed, construct_unicorn, UnicornFixedParam, UnicornInfo};
use keccak_prime::fortuna::Fortuna;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::convert::{TryFrom, TryInto};
use std::fmt;
use std::net::SocketAddr;
use tracing::log::{debug, info};
use prime::primitives::asset::TokenAmount;
use prime::primitives::block::Block;
use prime::primitives::transaction::Transaction;

/// Different states of the mining pipeline
#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub enum MiningPipelineStatus {
    #[default]
    Halted,
    ParticipantOnlyIntake,
    AllItemsIntake,
}

/// Change in phase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MiningPipelinePhaseChange {
    StartPhasePowIntake,
    StartPhaseHalted,
    Reset,
    ReSelect,
}

/// Different types of items that can be proposed to the block pipeline
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum MiningPipelineItem {
    MiningParticipant(SocketAddr, MiningPipelineStatus),
    CompleteParticipant,
    WinningPoW(SocketAddr, Box<WinningPoWInfo>),
    CompleteMining,
    ResetPipeline,
    MiningParticipantDropped(SocketAddr),
}

/// Participants collection (unsorted: given order, and lookup collection)
#[derive(Default, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Participants {
    pub unsorted: Vec<SocketAddr>,
    pub lookup: BTreeSet<SocketAddr>,
}

impl fmt::Debug for Participants {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.unsorted.fmt(f)
    }
}

impl Participants {
    pub fn len(&self) -> usize {
        self.unsorted.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &'_ SocketAddr> + '_ {
        self.unsorted.iter()
    }

    pub fn contains(&self, k: &SocketAddr) -> bool {
        self.lookup.contains(k)
    }

    pub fn lookup(&self) -> &BTreeSet<SocketAddr> {
        &self.lookup
    }

    pub fn push(&mut self, k: SocketAddr) -> bool {
        if self.lookup.insert(k) {
            self.unsorted.push(k);
            true
        } else {
            false
        }
    }
}

/// Extra info needed to process pipeline event
#[derive(Default, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct PipelineEventInfo {
    pub proposer_id: u64,
    pub sufficient_majority: usize,
    pub unanimous_majority: usize,
    pub partition_full_size: usize,
}

/// Rolling info particular to a specific mining pipeline
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct MiningPipelineInfo {
    /// Participants for intake phase
    participants_intake: BTreeMap<u64, Participants>,
    /// Participants during actual mining
    participants_mining: BTreeMap<u64, Participants>,
    /// Empty Participants collection
    empty_participants: Participants,
    /// The last round winning hashes
    last_winning_hashes: BTreeSet<String>,
    /// The wining PoWs for selection
    all_winning_pow: Vec<(SocketAddr, WinningPoWInfo)>,
    /// The unicorn info for the selections
    unicorn_info: UnicornInfo,
    /// The selected wining PoW
    winning_pow: Option<(SocketAddr, WinningPoWInfo)>,
    /// The current status
    mining_pipeline_status: MiningPipelineStatus,
    /// The timeout ids
    current_phase_timeout_peer_ids: BTreeSet<u64>,
    /// The timeout ids for a forceful pipeline change
    current_phase_reset_pipeline_peer_ids: BTreeSet<u64>,
    /// Fixed info for unicorn generation
    unicorn_fixed_param: UnicornFixedParam,
    /// Index of the last block,
    current_block_num: Option<u64>,
    /// Current block ready to mine (consensused).
    current_block: Option<Block>,
    /// All transactions present in current_block (consensused).
    current_block_tx: BTreeMap<String, Transaction>,
    /// The current reward for a given mempool node
    current_reward: TokenAmount,
    /// Proposed keys for current mining pipeline cycle
    proposed_keys: BTreeSet<RaftContextKey>,

    /// [AM] The total number of hashes since ASERT activation
    #[serde(default)]
    asert_winning_hashes_count: u64,

    /// [AM] the block height at which ASERT activates
    //
    // note: this data structure has a subtly complex interaction between:
    //       - new fields being added (schema migrations)
    //       - values for those fields being set via configuration
    //       - raft snapshotting
    //
    //       the default here will result in the activation height being
    //       set to the hard-coded main network activation height, however
    //       for testnets and local runs, this is overridable in configuration.
    //       because of this default, the fact that this configuration is
    //       almost certainly lost when a snapshot created before these fields
    //       were added to the struct is restored is annoying.
    //
    //       it is at least safe for mainnet.
    //
    //       this may be frustrating to debug without this code comment because
    //       it will work until it doesn't. running locally, blowing away the
    //       database between runs, the config value will be passed in, and it
    //       will make it into future snapshots, so even restored snapshots
    //       will behave themselves.
    //
    //       this quirk will only manifest when restoring a snapshot that was
    //       created from a version of this struct without this field, as the
    //       field value will be defaulted to the ACTIVATION_HEIGHT_ASERT
    //       constant, and not the value from configuration.
    //
    //       as an aside, it appears that `unicorn_info` has the same issue,
    //       although there may not be any instances of a raft snapshot without
    //       unicorn parameters out there in the wild. if the unicorn parameters
    //       are ever changed (through configuration) then it's unlikely that
    //       the change will be applied to an instance that restores from
    //       snapshot.
    #[serde(default = "activation_height_asert")]
    activation_height_asert: u64,

    /// Proposer ids that have voted a selected miner as dropped, per miner
    /// address, for the current phase.
    #[serde(default)]
    current_phase_dropped_peer_ids: BTreeMap<SocketAddr, BTreeSet<u64>>,
}

/// A dirty patch to enable continuation of mining off of pre-difficulty snapshots
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct MiningPipelineInfoPreDifficulty {
    /// Participants for intake phase
    participants_intake: BTreeMap<u64, Participants>,
    /// Participants during actual mining
    participants_mining: BTreeMap<u64, Participants>,
    /// Empty Participants collection
    empty_participants: Participants,
    /// The last round winning hashes
    last_winning_hashes: BTreeSet<String>,
    /// The wining PoWs for selection
    all_winning_pow: Vec<(SocketAddr, WinningPoWInfo)>,
    /// The unicorn info for the selections
    unicorn_info: UnicornInfo,
    /// The selected wining PoW
    winning_pow: Option<(SocketAddr, WinningPoWInfo)>,
    /// The current status
    mining_pipeline_status: MiningPipelineStatus,
    /// The timeout ids
    current_phase_timeout_peer_ids: BTreeSet<u64>,
    /// The timeout ids for a forceful pipeline change
    current_phase_reset_pipeline_peer_ids: BTreeSet<u64>,
    /// Fixed info for unicorn generation
    unicorn_fixed_param: UnicornFixedParam,
    /// Index of the last block,
    current_block_num: Option<u64>,
    /// Current block ready to mine (consensused).
    current_block: Option<Block>,
    /// All transactions present in current_block (consensused).
    current_block_tx: BTreeMap<String, Transaction>,
    /// The current reward for a given mempool node
    current_reward: TokenAmount,
    /// Proposed keys for current mining pipeline cycle
    proposed_keys: BTreeSet<RaftContextKey>,
}

/// A dirty patch to enable continuation of mining off of snapshots created
/// before the introduction of the mining-participant-dropped vote.
/// Byte-identical to `MiningPipelineInfo` minus `current_phase_dropped_peer_ids`.
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
pub struct MiningPipelineInfoPreDropped {
    /// Participants for intake phase
    participants_intake: BTreeMap<u64, Participants>,
    /// Participants during actual mining
    participants_mining: BTreeMap<u64, Participants>,
    /// Empty Participants collection
    empty_participants: Participants,
    /// The last round winning hashes
    last_winning_hashes: BTreeSet<String>,
    /// The wining PoWs for selection
    all_winning_pow: Vec<(SocketAddr, WinningPoWInfo)>,
    /// The unicorn info for the selections
    unicorn_info: UnicornInfo,
    /// The selected wining PoW
    winning_pow: Option<(SocketAddr, WinningPoWInfo)>,
    /// The current status
    mining_pipeline_status: MiningPipelineStatus,
    /// The timeout ids
    current_phase_timeout_peer_ids: BTreeSet<u64>,
    /// The timeout ids for a forceful pipeline change
    current_phase_reset_pipeline_peer_ids: BTreeSet<u64>,
    /// Fixed info for unicorn generation
    unicorn_fixed_param: UnicornFixedParam,
    /// Index of the last block,
    current_block_num: Option<u64>,
    /// Current block ready to mine (consensused).
    current_block: Option<Block>,
    /// All transactions present in current_block (consensused).
    current_block_tx: BTreeMap<String, Transaction>,
    /// The current reward for a given mempool node
    current_reward: TokenAmount,
    /// Proposed keys for current mining pipeline cycle
    proposed_keys: BTreeSet<RaftContextKey>,

    /// [AM] The total number of hashes since ASERT activation
    #[serde(default)]
    asert_winning_hashes_count: u64,

    /// [AM] the block height at which ASERT activates
    #[serde(default = "activation_height_asert")]
    activation_height_asert: u64,
}

const fn activation_height_asert() -> u64 {
    crate::constants::ACTIVATION_HEIGHT_ASERT
}

pub struct MiningPipelineInfoImport {
    pub unicorn_fixed_param: UnicornFixedParam,
    pub current_block_num: Option<u64>,
    pub current_block: Option<Block>,
    pub asert_winning_hashes_count: u64,
    pub activation_height_asert: u64,
}

impl From<MiningPipelineInfoPreDifficulty> for MiningPipelineInfo {
    fn from(value: MiningPipelineInfoPreDifficulty) -> Self {
        Self {
            unicorn_fixed_param: value.unicorn_fixed_param,
            current_block_num: value.current_block_num,
            current_block: value.current_block,
            participants_intake: value.participants_intake,
            participants_mining: value.participants_mining,
            empty_participants: value.empty_participants,
            last_winning_hashes: value.last_winning_hashes,
            all_winning_pow: value.all_winning_pow,
            unicorn_info: value.unicorn_info,
            winning_pow: value.winning_pow,
            mining_pipeline_status: value.mining_pipeline_status,
            current_phase_timeout_peer_ids: value.current_phase_timeout_peer_ids,
            current_phase_reset_pipeline_peer_ids: value.current_phase_reset_pipeline_peer_ids,
            current_block_tx: value.current_block_tx,
            current_reward: value.current_reward,
            proposed_keys: value.proposed_keys,
            asert_winning_hashes_count: 0,
            activation_height_asert: activation_height_asert(),
            current_phase_dropped_peer_ids: Default::default(),
        }
    }
}

impl From<MiningPipelineInfoPreDropped> for MiningPipelineInfo {
    fn from(value: MiningPipelineInfoPreDropped) -> Self {
        Self {
            unicorn_fixed_param: value.unicorn_fixed_param,
            current_block_num: value.current_block_num,
            current_block: value.current_block,
            participants_intake: value.participants_intake,
            participants_mining: value.participants_mining,
            empty_participants: value.empty_participants,
            last_winning_hashes: value.last_winning_hashes,
            all_winning_pow: value.all_winning_pow,
            unicorn_info: value.unicorn_info,
            winning_pow: value.winning_pow,
            mining_pipeline_status: value.mining_pipeline_status,
            current_phase_timeout_peer_ids: value.current_phase_timeout_peer_ids,
            current_phase_reset_pipeline_peer_ids: value.current_phase_reset_pipeline_peer_ids,
            current_block_tx: value.current_block_tx,
            current_reward: value.current_reward,
            proposed_keys: value.proposed_keys,
            asert_winning_hashes_count: value.asert_winning_hashes_count,
            activation_height_asert: value.activation_height_asert,
            current_phase_dropped_peer_ids: Default::default(),
        }
    }
}

impl MiningPipelineInfo {
    /// Specify the unicorn fixed params
    pub fn with_unicorn_fixed_param(mut self, unicorn_fixed_info: UnicornFixedInfo) -> Self {
        self.unicorn_fixed_param = UnicornFixedParam {
            modulus: unicorn_fixed_info.modulus,
            iterations: unicorn_fixed_info.iterations,
            security: unicorn_fixed_info.security,
        };
        self
    }

    /// Specify the height at which the ASERT algorithm activates
    pub fn with_activation_height_asert(mut self, height: u64) -> Self {
        self.activation_height_asert = height;
        self
    }

    /// Returns the number of winning hashes submitted since the ASERT DAA activated
    pub fn get_asert_winning_hashes_count(&self) -> u64 {
        self.asert_winning_hashes_count
    }

    /// Returns the height at which the ASERT DAA activated, or will activate
    pub fn get_activation_height_asert(&self) -> u64 {
        self.activation_height_asert
    }

    /// Gets the current reward for a given block
    pub fn get_current_reward(&self) -> &TokenAmount {
        &self.current_reward
    }

    /// Check if computing the first block.
    pub fn current_block_num(&self) -> Option<u64> {
        self.current_block_num
    }

    /// Current block to mine or being mined.
    pub fn get_mining_block(&self) -> &Option<Block> {
        &self.current_block
    }

    /// Current block to mine or being mined.
    pub fn get_mining_block_tx(&self) -> &BTreeMap<String, Transaction> {
        &self.current_block_tx
    }

    /// Set consensused committed block to mine.
    pub fn set_committed_mining_block(
        &mut self,
        block: Block,
        block_tx: BTreeMap<String, Transaction>,
    ) {
        self.current_block = Some(block);
        self.current_block_tx = block_tx;
    }

    /// Take mining block when mining is completed, use to populate mined block.
    pub fn take_mining_block(&mut self) -> Option<(Block, BTreeMap<String, Transaction>)> {
        let block = std::mem::take(&mut self.current_block);
        let block_tx = std::mem::take(&mut self.current_block_tx);
        block.map(|b| (b, block_tx))
    }

    pub fn apply_ready_block_stored_info(&mut self, block_num: u64, reward: TokenAmount) {
        // Reset block if not mined
        self.current_block = Default::default();
        self.current_block_tx = Default::default();

        self.current_block_num = Some(block_num);
        self.current_reward = reward;
    }

    /// Initialize block pipeline
    pub fn init_block_pipeline_status(mut self, extra: PipelineEventInfo) -> Self {
        if self.current_block.is_some() {
            self.start_items_intake(extra);
        }
        self
    }

    /// Start participants intake phase
    pub fn start_items_intake(&mut self, extra: PipelineEventInfo) {
        self.mining_pipeline_status = if self.participants_intake.is_empty() {
            MiningPipelineStatus::ParticipantOnlyIntake
        } else {
            self.unicorn_select_participants_mining(extra.partition_full_size);
            MiningPipelineStatus::AllItemsIntake
        };

        // Only keep relevant info for this phase
        self.last_winning_hashes = Default::default();
        self.all_winning_pow = Default::default();
        self.winning_pow = Default::default();
        self.current_phase_timeout_peer_ids = Default::default();
        self.current_phase_reset_pipeline_peer_ids = Default::default();
        self.current_phase_dropped_peer_ids = Default::default();

        debug!("MINING PIPELINE STATUS: {:?}", self.mining_pipeline_status);
        debug!("Participating Miners: {:?}", self.participants_mining);
    }

    /// Handle a mining pipeline item
    /// # Arguments
    /// * `pipeline_item` - The mining pipeline item (either a mining participant or PoW entry)
    /// * `extra`         - Extra info needed to process the item
    ///
    /// # Note
    /// Participant and PoW entries will only be added to the current `block_pipeline`
    /// if the mining pipeline state allows it.
    pub async fn handle_mining_pipeline_item(
        &mut self,
        pipeline_item: MiningPipelineItem,
        extra: PipelineEventInfo,
    ) -> Option<MiningPipelinePhaseChange> {
        use MiningPipelineItem::*;
        use MiningPipelineStatus::*;

        let pipeline_status = self.get_mining_pipeline_status().clone();
        match (pipeline_item, &pipeline_status) {
            (MiningParticipant(addr, ParticipantOnlyIntake), ParticipantOnlyIntake)
            | (MiningParticipant(addr, AllItemsIntake), AllItemsIntake) => {
                self.add_to_participants(extra.proposer_id, addr);
            }
            (CompleteParticipant, ParticipantOnlyIntake) => {
                self.append_current_phase_timeout(extra.proposer_id);
            }
            (WinningPoW(addr, info), AllItemsIntake) => {
                self.add_to_winning_pow(extra.proposer_id, (addr, *info));
            }
            (CompleteMining, AllItemsIntake) => {
                self.append_current_phase_timeout(extra.proposer_id);
            }
            (MiningParticipantDropped(addr), AllItemsIntake) => {
                self.append_dropped_vote(addr, extra.proposer_id);
                if self.is_drop_confirmed(&addr) {
                    self.evict_mining_participant(&addr);
                }
            }
            (ResetPipeline, _) => {
                self.append_reset_pipeline_timeout(extra.proposer_id);
            }
            (item, status) => {
                debug!(
                    "Failed to add entry {:?} with pipeline status: {:?}",
                    item, status
                );
            }
        }

        // There's been a vote for a forceful pipeline change instead of default flow
        if self.has_ready_reset_pipeline(extra.unanimous_majority) {
            return self.handle_reset_pipeline(extra);
        }

        match &pipeline_status {
            Halted => (),
            ParticipantOnlyIntake => {
                if self.has_ready_select_participating_miners(extra.sufficient_majority) {
                    self.start_items_intake(extra);
                    return Some(MiningPipelinePhaseChange::StartPhasePowIntake);
                }
            }
            AllItemsIntake => {
                // A committed drop can drain every remaining participant: reopen
                // intake and re-select rather than mine an empty round.
                if self.round_is_drained() {
                    self.reset_to_intake(extra);
                    return Some(MiningPipelinePhaseChange::ReSelect);
                }
                if self.has_ready_select_winning_miner(extra.sufficient_majority) {
                    self.start_winning_pow_halted();
                    return Some(MiningPipelinePhaseChange::StartPhaseHalted);
                }
            }
        }

        None
    }

    /// Clear all proposed keys
    pub fn clear_proposed_keys(&mut self) {
        self.proposed_keys = Default::default();
    }

    /// Record a vote from `proposer_id` that the selected miner `addr` has
    /// dropped from the current mining phase.
    ///
    /// ### Arguments
    ///
    /// * `addr`        - The dropped mining participant's address.
    /// * `proposer_id` - The proposer casting the vote.
    pub fn append_dropped_vote(&mut self, addr: SocketAddr, proposer_id: u64) {
        self.current_phase_dropped_peer_ids
            .entry(addr)
            .or_default()
            .insert(proposer_id);
    }

    /// Returns true if the accumulated dropped votes for `addr` have reached the
    /// sufficient majority.
    ///
    /// ### Arguments
    ///
    /// * `addr`                - The dropped mining participant's address.
    /// * `sufficient_majority` - The vote threshold to reach.
    pub fn dropped_has_majority(&self, addr: &SocketAddr, sufficient_majority: usize) -> bool {
        self.current_phase_dropped_peer_ids
            .get(addr)
            .map(|votes| votes.len() >= sufficient_majority)
            .unwrap_or(false)
    }

    /// Returns true once a `MiningParticipantDropped` vote for `addr` has been
    /// committed by its owning proposer — i.e. the drop is confirmed and the
    /// participant can be evicted.
    ///
    /// The threshold is deliberately ONE committed vote, not the sufficient
    /// majority. Miners are 1:1 with their owning mempool's proposer bucket
    /// (`participants_mining` is keyed by proposer_id), so only that one mempool
    /// observes the miner's liveness and can ever propose its drop. No other
    /// node has the miner in its bucket, so a `MiningParticipantDropped` vote
    /// can never accumulate more than a single proposer — requiring a
    /// `sufficient_majority` (e.g. 2 of 3) would be unsatisfiable in a
    /// multi-mempool cluster and eviction would never fire, wrongly falling
    /// back to the slow watchdog. The owning mempool is therefore authoritative
    /// for its own miners: its single committed drop suffices. This remains
    /// consensus-safe because the vote is a committed RAFT item applied
    /// deterministically (via `evict_mining_participant`) on every node, so all
    /// nodes converge on the same participant set — no fork.
    pub fn is_drop_confirmed(&self, addr: &SocketAddr) -> bool {
        self.dropped_has_majority(addr, 1)
    }

    /// Remove a single mining participant from every `participants_mining`
    /// bucket. Idempotent: removing an absent address is a no-op.
    ///
    /// ### Arguments
    ///
    /// * `addr` - The mining participant's address to evict.
    pub fn evict_mining_participant(&mut self, addr: &SocketAddr) {
        for (_, participants) in self.participants_mining.iter_mut() {
            participants.unsorted.retain(|a| a != addr);
            participants.lookup.remove(addr);
        }
    }

    /// Given locally-detected unreachable miner addresses, return the subset
    /// that are *currently selected* mining participants for `proposer_id`,
    /// deduplicated and preserving the order of `unreachable`.
    ///
    /// These are the only addresses whose committed mining state may be
    /// evicted, and even then only via a committed `MiningParticipantDropped`
    /// vote (see `handle_mining_pipeline_item`) — never by local mutation.
    /// Outside `AllItemsIntake` the mining set is empty, so intake-pool /
    /// request-list peers never appear here; they are pruned only from
    /// per-node flood bookkeeping.
    ///
    /// Pure read: this never mutates any participant state.
    pub fn mining_participants_to_drop(
        &self,
        proposer_id: u64,
        unreachable: &[SocketAddr],
    ) -> Vec<SocketAddr> {
        let participants = self.get_mining_participants(proposer_id);
        let mut seen = BTreeSet::new();
        unreachable
            .iter()
            .copied()
            .filter(|addr| participants.contains(addr))
            .filter(|addr| seen.insert(*addr))
            .collect()
    }

    /// Get proposed RaftContextKey set
    pub fn get_proposed_keys(&self) -> &BTreeSet<RaftContextKey> {
        &self.proposed_keys
    }

    /// Add a RaftContextKey to the proposed set
    pub fn add_proposed_key(&mut self, key: RaftContextKey) {
        self.proposed_keys.insert(key);
    }

    /// Returns true when no mining participants remain across all
    /// `participants_mining` buckets.
    pub fn round_is_drained(&self) -> bool {
        self.participants_mining
            .values()
            .all(|p| p.unsorted.is_empty())
    }

    /// Clear the per-phase vote/participant state and reopen participant intake,
    /// preserving the consensused `current_block`/`current_block_tx`. Shared by
    /// the reset and re-select transitions.
    fn reset_to_intake(&mut self, extra: PipelineEventInfo) {
        self.current_phase_timeout_peer_ids = Default::default();
        self.current_phase_reset_pipeline_peer_ids = Default::default();
        self.current_phase_dropped_peer_ids = Default::default();

        // Clear participants intake
        self.participants_intake = Default::default();
        self.participants_mining = Default::default();
        self.start_items_intake(extra);
    }

    pub fn handle_reset_pipeline(
        &mut self,
        extra: PipelineEventInfo,
    ) -> Option<MiningPipelinePhaseChange> {
        self.reset_to_intake(extra);
        Some(MiningPipelinePhaseChange::Reset)
    }

    /// New mining event to propose
    pub fn mining_event_at_timeout(&mut self) -> Option<MiningPipelineItem> {
        use MiningPipelineStatus::*;
        match self.get_mining_pipeline_status() {
            ParticipantOnlyIntake => Some(MiningPipelineItem::CompleteParticipant),
            AllItemsIntake => Some(MiningPipelineItem::CompleteMining),
            Halted => None,
        }
    }

    /// Process all the mining phase in a single step
    pub fn test_skip_mining(&mut self, winning_pow: (SocketAddr, WinningPoWInfo), seed: Vec<u8>) {
        info!("test_skip_mining PoW entry: {:?} ({:?})", winning_pow, seed);

        let block = self.current_block.as_mut().unwrap();
        block.header.seed_value = seed;

        self.unicorn_select_participants_mining(usize::MAX);
        self.all_winning_pow.push(winning_pow);
        self.start_winning_pow_halted();
    }

    /// Retrieves the current UNICORN for this pipeline
    pub fn get_unicorn(&self) -> &UnicornInfo {
        &self.unicorn_info
    }

    /// Retrieves the miners participating in the current round
    pub fn get_mining_participants(&self, proposer_id: u64) -> &Participants {
        self.participants_mining
            .get(&proposer_id)
            .unwrap_or(&self.empty_participants)
    }

    /// Retrieves the winning miner for the current mining round
    pub fn get_winning_miner(&self) -> &Option<(SocketAddr, WinningPoWInfo)> {
        &self.winning_pow
    }

    /// Number of winning PoW entries accumulated for the current phase but not
    /// yet resolved into a winning miner. Used by the mempool progress watchdog
    /// to treat a newly committed PoW as forward progress.
    pub fn accumulated_winning_pow_count(&self) -> usize {
        self.all_winning_pow.len()
    }

    /// Get the mining pipeline status
    pub fn get_mining_pipeline_status(&self) -> &MiningPipelineStatus {
        &self.mining_pipeline_status
    }

    /// Add a new participant to the eligible list, if possible
    pub fn add_to_participants(&mut self, proposer_id: u64, participant: SocketAddr) {
        let participants = self.participants_intake.entry(proposer_id).or_default();
        if participants.push(participant) {
            debug!(
                "Adding miner participant: {}-{:?}",
                proposer_id, participant
            );
        }
    }

    /// Add winning PoW to the running list
    pub fn add_to_winning_pow(
        &mut self,
        proposer_id: u64,
        winning_pow: (SocketAddr, WinningPoWInfo),
    ) {
        let participants = self.get_mining_participants(proposer_id);
        if !participants.contains(&winning_pow.0) {
            debug!(
                "Ignore PoW entry from miner (Non participant): {}-{:?}",
                proposer_id, winning_pow.0
            );
            return;
        }

        debug!(
            "Adding PoW entry from miner: {}-{:?}",
            proposer_id, winning_pow.0
        );
        self.all_winning_pow.push(winning_pow);
    }

    /// Selects a winning miner from the list via UNICORN
    pub fn has_ready_select_participating_miners(&mut self, sufficient_majority: usize) -> bool {
        self.current_phase_timeout_peer_ids.len() >= sufficient_majority
            && self.participants_intake.len() >= sufficient_majority
    }

    /// Has enough majority vote to change the mining pipeline status forcefully
    pub fn has_ready_reset_pipeline(&mut self, unanimous_majority: usize) -> bool {
        self.current_phase_reset_pipeline_peer_ids.len() >= unanimous_majority
    }

    /// Select miners to mine current block and move to Pow intake
    pub fn unicorn_select_participants_mining(&mut self, partition_full_size: usize) {
        let mut participants = std::mem::take(&mut self.participants_intake);
        for ps in participants.values_mut() {
            if ps.unsorted.len() <= partition_full_size {
                continue;
            }
            self.swap_with_unicorn(MINER_PARTICIPATION_UN, &mut ps.unsorted);
            ps.unsorted.truncate(partition_full_size);
            ps.lookup = ps.unsorted.iter().copied().collect();
        }
        self.participants_mining = participants;
    }

    /// Selects a winning miner from the list via UNICORN
    pub fn has_ready_select_winning_miner(&mut self, sufficient_majority: usize) -> bool {
        if self.current_phase_timeout_peer_ids.len() < sufficient_majority {
            return false;
        }

        !self.all_winning_pow.is_empty()
    }

    /// Selects a winning miner from the list via UNICORN and move to halted state
    pub fn start_winning_pow_halted(&mut self) {
        let all_winning_pow = std::mem::take(&mut self.all_winning_pow);
        let _timeouts = std::mem::take(&mut self.current_phase_timeout_peer_ids);

        // [AM] we need to track the number of solutions since the activation block
        // as this is an input to the mapping function between target-solutions-per-block
        // and the ASERT algorithm's understanding of target-block-interval.
        if let Some(b_num) = self.current_block_num {
            // greater-than-or-equal, greater-than?
            // anchor block is at the activation height.
            // the first block to use ASERT is the one that immediately follows the anchor block.
            // therefore, we collect metrics from the anchor block.
            if b_num >= self.activation_height_asert {
                self.asert_winning_hashes_count += u64::try_from(all_winning_pow.len())
                    .unwrap_or(crate::constants::ASERT_TARGET_HASHES_PER_BLOCK);
            }
        } else {
            panic!("[AM] we've hooked the wrong place; we need the block number and the number of winning hashes in the same place at the same time");
        }

        self.winning_pow = self
            .get_unicorn_item(WINNING_MINER_UN, &all_winning_pow)
            .cloned();
        self.last_winning_hashes = all_winning_pow
            .into_iter()
            .map(|(_, pow)| pow.mining_tx.0)
            .collect();
        self.mining_pipeline_status = MiningPipelineStatus::Halted;

        debug!("MINING PIPELINE STATUS: {:?}", self.mining_pipeline_status);
        info!("Winning PoW Entry: {:?}", self.winning_pow);
    }

    /// Inserts a prosper_id into the current_block_complete_timeout_peer_ids.
    /// proposer_id is take from key
    ///
    /// ### Arguments
    ///
    /// * `proposer_id` - The proposer_id to be appended.
    pub fn append_current_phase_timeout(&mut self, proposer_id: u64) {
        self.current_phase_timeout_peer_ids.insert(proposer_id);
    }

    /// Inserts a prosper_id into the current_phase_reset_pipeline_peer_ids.
    /// proposer_id is take from key
    ///
    /// ### Arguments
    ///
    /// * `proposer_id` - The proposer_id to be appended.
    pub fn append_reset_pipeline_timeout(&mut self, proposer_id: u64) {
        self.current_phase_reset_pipeline_peer_ids
            .insert(proposer_id);
    }

    /// Sets the new UNICORN value based on the latest info
    pub fn construct_unicorn(&mut self) {
        let block = self.current_block.as_mut().unwrap();
        let tx_inputs = &block.transactions;

        debug!(
            "Constructing UNICORN value using {:?}, {:?}, {:?}",
            tx_inputs, self.participants_mining, self.last_winning_hashes
        );

        let all_participants = self.participants_mining.values().map(|p| &p.unsorted);
        let all_participants: Vec<_> = all_participants.flatten().copied().collect();
        let seed = construct_seed(tx_inputs, &all_participants, &self.last_winning_hashes);
        self.unicorn_info = construct_unicorn(seed, &self.unicorn_fixed_param);
        block.header.seed_value = get_unicorn_seed_value(&self.unicorn_info);
    }

    /// Gets a UNICORN-generated pseudo random number
    ///
    /// ### Arguments
    ///
    /// * `usage_number` - Usage number for the CSPRNG
    pub fn get_unicorn_prn(&self, usage_number: u128) -> u64 {
        debug!("Using UNICORN value: {:?}", self.unicorn_info);
        let prn_seed: [u8; 32] = self.unicorn_info.g_value.as_bytes()[..32]
            .try_into()
            .unwrap();

        let mut csprng = Fortuna::new(&prn_seed, usage_number).unwrap();

        let val = csprng.get_bytes(8).unwrap();
        u64::from_be_bytes(val[0..8].try_into().unwrap())
    }

    /// Gets a UNICORN-generated pseudo random number
    ///
    /// ### Arguments
    ///
    /// * `usage_number` - Usage number for the CSPRNG
    pub fn get_unicorn_item<'a, T>(&self, usage_number: u128, items: &'a [T]) -> Option<&'a T> {
        if items.is_empty() {
            return None;
        }

        let prn = self.get_unicorn_prn(usage_number);
        let selection = prn as usize % items.len();
        Some(&items[selection])
    }

    /// Swap an array's items with UNICORN
    ///
    /// ### Arguments
    ///
    /// * `usage_number` - Usage number for the CSPRNG
    /// * `items` - Items to be swapped
    pub fn swap_with_unicorn<T>(&self, usage_number: u128, items: &mut [T]) {
        for i in 0..items.len() {
            let prn = self.get_unicorn_prn(usage_number);
            let selection = i + prn as usize % (items.len() - i);
            items.swap(i, selection);
        }
    }

    /// Create MempoolConsensused from imported data in upgrade
    pub fn from_import(value: MiningPipelineInfoImport) -> Self {
        let MiningPipelineInfoImport {
            unicorn_fixed_param,
            current_block_num,
            current_block,
            activation_height_asert,
            asert_winning_hashes_count,
        } = value;

        Self {
            unicorn_fixed_param,
            current_block_num,
            current_block,
            activation_height_asert,
            asert_winning_hashes_count,
            ..Default::default()
        }
    }

    /// Convert to import type
    pub fn into_import(self) -> MiningPipelineInfoImport {
        MiningPipelineInfoImport {
            unicorn_fixed_param: self.unicorn_fixed_param,
            current_block_num: self.current_block_num,
            current_block: self.current_block,
            activation_height_asert: self.activation_height_asert,
            asert_winning_hashes_count: self.asert_winning_hashes_count,
        }
    }
}

/// Return the seed value for the block based on given unicorn
fn get_unicorn_seed_value(u: &UnicornInfo) -> Vec<u8> {
    format!("{}-{}", u.unicorn.seed, u.witness).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(port: u16) -> SocketAddr {
        format!("127.0.0.1:{port}").parse().unwrap()
    }

    fn participants_with(addrs: &[SocketAddr]) -> Participants {
        let mut p = Participants::default();
        for a in addrs {
            p.push(*a);
        }
        p
    }

    #[test]
    fn append_dropped_vote_accumulates_distinct_proposers() {
        let mut info = MiningPipelineInfo::default();
        let a = addr(1000);

        info.append_dropped_vote(a, 1);
        info.append_dropped_vote(a, 2);
        // Duplicate proposer must not double count.
        info.append_dropped_vote(a, 2);

        let votes = info.current_phase_dropped_peer_ids.get(&a).unwrap();
        assert_eq!(votes.len(), 2);
        assert!(votes.contains(&1));
        assert!(votes.contains(&2));
    }

    #[test]
    fn dropped_has_majority_at_threshold() {
        let mut info = MiningPipelineInfo::default();
        let a = addr(1000);

        assert!(!info.dropped_has_majority(&a, 2));
        info.append_dropped_vote(a, 1);
        assert!(!info.dropped_has_majority(&a, 2));
        info.append_dropped_vote(a, 2);
        assert!(info.dropped_has_majority(&a, 2));
        info.append_dropped_vote(a, 3);
        assert!(info.dropped_has_majority(&a, 2));
    }

    #[test]
    fn dropped_has_majority_unknown_addr_is_false() {
        let info = MiningPipelineInfo::default();
        assert!(!info.dropped_has_majority(&addr(9999), 1));
    }

    #[test]
    fn evict_mining_participant_removes_from_all_buckets_and_is_idempotent() {
        let mut info = MiningPipelineInfo::default();
        let a = addr(1000);
        let b = addr(1001);
        let c = addr(1002);

        info.participants_mining
            .insert(1, participants_with(&[a, b, c]));
        info.participants_mining.insert(2, participants_with(&[a, b]));

        info.evict_mining_participant(&b);

        for participants in info.participants_mining.values() {
            assert!(!participants.unsorted.contains(&b));
            assert!(!participants.lookup.contains(&b));
        }
        // Non-target addresses remain.
        assert!(info.participants_mining.get(&1).unwrap().contains(&a));
        assert!(info.participants_mining.get(&1).unwrap().contains(&c));

        // Idempotent: evicting again is a no-op and does not panic.
        info.evict_mining_participant(&b);
        assert_eq!(info.participants_mining.get(&1).unwrap().len(), 2);
        assert_eq!(info.participants_mining.get(&2).unwrap().len(), 1);
    }

    #[test]
    fn mining_participants_to_drop_returns_intersection_deduped_without_mutating() {
        let mut info = MiningPipelineInfo::default();
        let a = addr(1000);
        let b = addr(1001);
        let c = addr(1002);
        let intake_only = addr(2000);
        let not_selected = addr(3000);

        info.participants_mining
            .insert(1, participants_with(&[a, b, c]));
        // An intake-pool peer must never be considered for a consensus drop.
        info.participants_intake
            .insert(1, participants_with(&[intake_only]));

        let mining_before = info.participants_mining.clone();
        let intake_before = info.participants_intake.clone();

        // Unreachable set contains: two selected participants (with a duplicate),
        // an intake-only peer, and an address that is not tracked at all.
        let unreachable = [a, not_selected, b, intake_only, a];
        let drops = info.mining_participants_to_drop(1, &unreachable);

        // Only currently-selected mining participants, deduped, in input order.
        assert_eq!(drops, vec![a, b]);

        // The local decision is a pure read: committed participant state is
        // untouched. Eviction happens only through the committed vote path.
        assert_eq!(info.participants_mining, mining_before);
        assert_eq!(info.participants_intake, intake_before);
    }

    #[test]
    fn mining_participants_to_drop_empty_outside_all_items_intake() {
        // Outside AllItemsIntake there are no selected mining participants for
        // this proposer, so no unreachable peer (intake-pool or request-list)
        // may trigger a consensus drop.
        let info = MiningPipelineInfo::default();
        let unreachable = [addr(1000), addr(1001)];

        assert!(info
            .mining_participants_to_drop(0, &unreachable)
            .is_empty());
    }

    #[test]
    fn start_items_intake_clears_dropped_votes() {
        let mut info = MiningPipelineInfo::default();
        info.append_dropped_vote(addr(1000), 1);
        assert!(!info.current_phase_dropped_peer_ids.is_empty());

        info.start_items_intake(PipelineEventInfo::default());

        assert!(info.current_phase_dropped_peer_ids.is_empty());
    }

    #[test]
    fn handle_reset_pipeline_clears_dropped_votes() {
        let mut info = MiningPipelineInfo::default();
        info.append_dropped_vote(addr(1000), 1);

        info.handle_reset_pipeline(PipelineEventInfo::default());

        assert!(info.current_phase_dropped_peer_ids.is_empty());
    }

    #[test]
    fn from_pre_dropped_defaults_field_empty() {
        let pre = MiningPipelineInfoPreDropped {
            asert_winning_hashes_count: 42,
            activation_height_asert: 7,
            current_block_num: Some(9),
            ..Default::default()
        };

        let info: MiningPipelineInfo = pre.into();

        assert!(info.current_phase_dropped_peer_ids.is_empty());
        // Existing fields carried over unchanged.
        assert_eq!(info.get_asert_winning_hashes_count(), 42);
        assert_eq!(info.get_activation_height_asert(), 7);
        assert_eq!(info.current_block_num(), Some(9));
    }
}
