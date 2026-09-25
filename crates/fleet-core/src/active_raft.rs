use crate::db_utils::{SimpleDb, SimpleDbError};
use crate::raft::{
    CommitReceiver, RaftCmd, RaftCmdSender, RaftCommit, RaftCommitData, RaftData,
    RaftMessageWrapper, RaftMsgReceiver, RaftNode,
};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Provide RAFT loop and in/out channels to interact with it.
///
pub struct ActiveRaft {
    /// false if RAFT is bypassed.
    use_raft: bool,
    /// The raft peer id.
    peer_id: u64,
    /// Raft node used for running loop: only use for run_raft_loop.
    raft_node: Arc<Mutex<RaftNode>>,
    /// Channel to send command to the running RaftNode.
    cmd_tx: RaftCmdSender,
    /// Channel to receive messages from the running RaftNode to pass arround.
    msg_out_rx: Arc<Mutex<RaftMsgReceiver>>,
    /// Channel to receive commited entries from the running RaftNode to process.
    /// and extra data not processed yet.
    committed_rx: Arc<Mutex<(CommitReceiver, VecDeque<RaftCommit>)>>,
    /// Map to the address of the peers.
    peer_addr: HashMap<u64, SocketAddr>,
    /// Collection of the peer this node is responsible to connect to.
    raft_peers_to_connect: Vec<SocketAddr>,
    /// Collection of the peer expected to be connected.
    raft_peer_addrs: Vec<SocketAddr>,
}

impl ActiveRaft {
    /// Create ActiveRaft, need to spawn the raft loop to use raft.
    pub fn new(
        node_idx: usize,
        node_specs: &[SocketAddr],
        use_raft: bool,
        tick_timeout_duration: Duration,
        raft_db: SimpleDb,
    ) -> Self {
        let peers: Vec<u64> = (0..node_specs.len()).map(|idx| idx as u64 + 1).collect();
        let peer_id = peers[node_idx];

        let peer_addr_vec: Vec<(u64, SocketAddr)> = peers
            .iter()
            .zip(node_specs.iter())
            .map(|(idx, spec)| (*idx, *spec))
            .filter(|(idx, _)| use_raft || *idx == peer_id)
            .collect();

        let (raft_config, raft_channels) = RaftNode::init_config(
            raft::Config {
                id: peer_id,
                peers,
                max_size_per_msg: 4096,
                max_inflight_msgs: 256,
                pre_vote: true,
                check_quorum: true,
                tag: format!("[id={peer_id}]"),
                ..Default::default()
            },
            raft_db,
            tick_timeout_duration,
        );

        let peer_addr: HashMap<u64, SocketAddr> = peer_addr_vec.iter().cloned().collect();

        // Dial every other RAFT peer bidirectionally. Each node holds an OUTBOUND connection to
        // every peer, keyed under that peer's STABLE address (see `Node::connect_to_peer`, which
        // pins the entry to the stable address via `key_override`). RAFT sends target the stable
        // address (`next_msg` returns `peer_addr`), so they always hit this outbound connection
        // regardless of the peer's advertised (source) address. Previously nodes only dialed
        // LOWER ids, so the lowest id dialed nobody and had only inbound connections keyed under
        // peers' advertised addresses; when its advertised address diverged from DNS (e.g. a
        // Railway restart) its RAFT sends to the stable address missed and, as leader, it could
        // not replicate -> no quorum. Dialing all peers gives every node a stable-keyed path.
        let raft_peers_to_connect = peer_addr_vec
            .iter()
            .filter(|(idx, _)| *idx != peer_id)
            .map(|(_, addr)| *addr)
            .collect();

        let raft_peer_addrs = peer_addr_vec
            .iter()
            .filter(|(idx, _)| *idx != peer_id)
            .map(|(_, addr)| *addr)
            .collect();

        Self {
            use_raft,
            peer_id,
            raft_node: Arc::new(Mutex::new(RaftNode::new(raft_config))),
            cmd_tx: raft_channels.cmd_tx,
            msg_out_rx: Arc::new(Mutex::new(raft_channels.msg_out_rx)),
            committed_rx: Arc::new(Mutex::new((raft_channels.committed_rx, VecDeque::new()))),
            peer_addr,
            raft_peers_to_connect,
            raft_peer_addrs,
        }
    }

    /// Returns a boolean of whether or not the raft is bypassed. False for bypassed. True for not bypassed.
    pub fn use_raft(&self) -> bool {
        self.use_raft
    }

    /// Returns the peer ID of this raft
    pub fn peer_id(&self) -> u64 {
        self.peer_id
    }

    /// Returns a map to the addresses of this raft's peers
    pub fn peers_len(&self) -> usize {
        self.peer_addr.len()
    }

    /// All the peers to connect to when using raft.
    /// Returns an iterator that iterates over the addresses of the peers
    pub fn raft_peer_to_connect(&self) -> impl Iterator<Item = &SocketAddr> {
        self.raft_peers_to_connect.iter()
    }

    /// All the peers expected to be connected when raft is running.
    pub fn raft_peer_addrs(&self) -> impl Iterator<Item = &SocketAddr> {
        self.raft_peer_addrs.iter()
    }

    /// Blocks & waits for a next event from a peer.
    pub fn raft_loop(&self) -> impl Future<Output = ()> {
        let raft_node = self.raft_node.clone();
        let use_raft = self.use_raft;
        async move {
            if use_raft {
                raft_node.lock().await.run_raft_loop().await;
            }
        }
    }

    /// Signal to the raft loop to complete
    pub async fn close_raft_loop(&mut self) {
        // Ensure the loop is not stalled:
        self.msg_out_rx.lock().await.close();
        self.committed_rx.lock().await.0.close();

        // Close the loop
        self.cmd_tx.send(RaftCmd::Close).unwrap();
    }

    /// Extract persistent storage of a closed raft
    pub async fn take_closed_persistent_store(&mut self) -> SimpleDb {
        self.raft_node.lock().await.take_closed_persistent_store()
    }

    /// Backup persistent storage
    pub async fn backup_persistent_store(&self) -> Result<(), SimpleDbError> {
        self.raft_node.lock().await.backup_persistent_store()
    }

    /// Blocks & waits for a next commit from a peer.
    pub async fn next_commit(&self) -> Option<RaftCommit> {
        let mut committed_rx = self.committed_rx.lock().await;

        loop {
            if let Some(commit) = committed_rx.1.pop_front() {
                return Some(commit);
            } else if let Some(commits) = committed_rx.0.recv().await {
                committed_rx.1.extend(commits.into_iter());
            }
        }
    }

    /// Blocks & waits for a next message to dispatch from a peer.
    /// Message needs to be sent to given peer address.
    pub async fn next_msg(&self) -> Option<(SocketAddr, RaftMessageWrapper)> {
        let msg = self.msg_out_rx.lock().await.recv().await?;
        let addr = *self.peer_addr.get(&msg.to).unwrap();
        Some((addr, RaftMessageWrapper(msg)))
    }

    /// Process a raft message: send to spawned raft loop.
    pub async fn received_message(&mut self, msg: RaftMessageWrapper) {
        self.cmd_tx.send(RaftCmd::Raft(msg)).unwrap();
    }

    /// Propose RaftData to raft if use_raft, or commit it otherwise.
    pub async fn propose_data(&mut self, data: RaftData, context: RaftData) {
        if self.use_raft {
            self.cmd_tx
                .send(RaftCmd::Propose { data, context })
                .unwrap();
        } else {
            self.committed_rx.lock().await.1.push_back(RaftCommit {
                data: RaftCommitData::Proposed(data, context),
                ..RaftCommit::default()
            });
        }
    }

    /// Create a snapshot at the given idx with the given data.
    ///
    /// ## Arguments
    /// * `idx` - The index of the snapshot
    /// * `data` - The data to snapshot
    /// * `backup` - Whether or not to backup the DB
    pub fn create_snapshot(&mut self, idx: u64, data: RaftData, backup: bool) {
        if self.use_raft {
            self.cmd_tx
                .send(RaftCmd::Snapshot { idx, data, backup })
                .unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_active_raft(node_idx: usize, specs: &[SocketAddr]) -> ActiveRaft {
        ActiveRaft::new(
            node_idx,
            specs,
            true,
            Duration::from_millis(1),
            SimpleDb::new_in_memory(&[], None).unwrap(),
        )
    }

    fn specs() -> Vec<SocketAddr> {
        vec![
            "127.0.0.1:1001".parse().unwrap(),
            "127.0.0.1:1002".parse().unwrap(),
            "127.0.0.1:1003".parse().unwrap(),
        ]
    }

    /// Every node dials every OTHER peer (bidirectional), so `raft_peers_to_connect` never
    /// contains the node's own address and always contains all siblings.
    #[test]
    fn dials_all_other_peers_bidirectionally() {
        let specs = specs();

        for node_idx in 0..specs.len() {
            let raft = make_active_raft(node_idx, &specs);
            let to_connect: Vec<SocketAddr> = raft.raft_peer_to_connect().cloned().collect();

            let expected: Vec<SocketAddr> = specs
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != node_idx)
                .map(|(_, addr)| *addr)
                .collect();

            assert_eq!(
                to_connect, expected,
                "node_idx {node_idx} must dial all other peers"
            );
            assert!(
                !to_connect.contains(&specs[node_idx]),
                "node must not dial itself"
            );
        }
    }

    /// Regression for the stalled-testnet bug: the lowest-id node (idx 0, id 1) previously
    /// dialed NOBODY (old filter kept only `idx < peer_id`), leaving it with inbound-only,
    /// advertised-keyed connections its RAFT sends could not reach. It must now dial outbound
    /// to every higher-id peer so it has a stable-keyed path as leader.
    #[test]
    fn lowest_id_node_now_dials_higher_id_peers() {
        let specs = specs();
        let raft = make_active_raft(0, &specs);
        let to_connect: Vec<SocketAddr> = raft.raft_peer_to_connect().cloned().collect();

        assert!(!to_connect.is_empty(), "lowest-id node must not dial nobody");
        assert!(to_connect.contains(&specs[1]));
        assert!(to_connect.contains(&specs[2]));
    }
}
