# Upgrades, forks and state migrations

This note is the checklist for shipping breaking changes without stranding a
running network. It exists because a testnet wedge in 2026-09 turned into an
unreadable mempool snapshot: an image was rolled forward across a serialization
boundary (vendored prime + difficulty-in-header) with no tested migration and no
coordinated stop, so the new code could not deserialize the old consensus
snapshot (`InvalidTagEncoding`). The chain data was safe in storage, but the
node could not resume. On a testnet that is a reset; on mainnet it is an outage.

## Two kinds of breaking change — keep them separate

They are handled by completely different mechanisms. Most real upgrades are one
or the other; some are both, and then you do both.

### 1. Consensus rule changes (a hard fork)

A change to how blocks or transactions are formed/validated: difficulty in the
header, a new script opcode, a reward change. Old-rule and new-rule nodes
disagree about validity.

**Mechanism: activation heights.** Never change validation retroactively. Gate
the new rule behind a block height `H`:

- pre-`H` blocks are validated by the old rule, post-`H` by the new rule, so the
  existing chain and its history stay valid;
- announce `H` far enough ahead that operators upgrade before it;
- a node that has not upgraded by `H` stops following the chain (by design) — it
  does not corrupt anything.

Fleet already does this for ASERT (`activation_height_asert`): ASERT only applies
from the activation height onward. Every future consensus change follows the same
pattern. The difficulty-in-header change is consensus-gated correctly; what was
missing was the *state migration* below.

### 2. Persisted-state / serialization format changes (a migration)

A change to how a node stores its own data on disk: the RAFT consensus snapshot,
DB column families, `blockchain_item` encoding, wallet fixtures. These do not
change consensus — they change whether a node can read *its own* old data. This
is where the 2026-09 incident actually failed.

**Mechanism: versioned formats + migrations that are tested against real old
data.**

- **Version every persisted format explicitly.** Use additive, discriminant-
  stable encodings: append enum variants, never reorder or insert them; add
  struct fields at the end. For column families, add a new versioned family —
  never relabel a family in place (`DB_COLS_BC` is the versioning path).
- **Write a migration that reads the old version and upgrades on load.** Fleet
  has the shape of this already: `deserialize_consensused_snapshot` tries the
  current format then falls back through `MempoolConsensusedPreDropped` /
  `MempoolConsensusedPreDifficulty`.
- **Test every migration against a captured real old-version snapshot/DB, not
  just freshly-generated data.** This is the rule the incident broke: the
  migration structs existed but were never exercised against an actual old
  snapshot, so they did not match the on-disk bytes and silently failed only in
  production. A migration you have not loaded real old data through is a
  migration you cannot trust. Keep a fixtures directory of real old-version
  snapshots/DBs and a test that loads each one and asserts a clean upgrade.
- Remember nested types: a snapshot embeds `Block`/`Transaction`/`UtxoSet`. A
  change to any nested type is a snapshot format change even if the outer struct
  is untouched. The migration structs must reconstruct the *old nested types*,
  not reuse the current ones.

## Rolling out an upgrade safely

1. **Coordinated stop, upgrade, resume — never hot-swap across a format
   boundary.** Fleet has a coordinated-shutdown-for-upgrade path
   (`SpecialHandling::Shutdown`, "Coordinated Shutdown for upgrade" committed at
   a block). The safe rollout is: reach a clean checkpoint at an agreed height →
   bring every node down → upgrade all nodes with the tested migration → resume.
   No node should ever run new code against old on-disk state mid-flight. The
   2026-09 testnet break was exactly this anti-pattern: images were redeployed
   one at a time across the boundary with no coordinated stop and no migration.
2. **Pin images by digest for a coordinated upgrade.** `latest` is fine for a
   healthy steady state, but during an upgrade pin the exact digest so every
   node moves together and a mid-rollout `latest` rebuild cannot split the fleet.
3. **Back up persisted volumes before the upgrade** so a failed migration can
   roll back.
4. **Stage it on testnet first**, including the migration, against a copy of
   real mainnet-shaped state.

## Make derived state rebuildable from the canonical chain

The strongest protection against state-format breakage: derived state (the
mempool UTXO set / consensus snapshot) should be reconstructible from the
authoritative block chain held by storage. Then a node that cannot migrate its
snapshot is not dead — it re-derives from the chain (a slow resync instead of an
outage), and the same capability powers new-node bootstrap, horizontal scaling
and disaster recovery. Storage does not currently maintain a servable UTXO set;
adding UTXO tracking (or a mempool chain-replay path) is tracked as follow-up and
is the highest-leverage resilience investment for mainnet.

## Mainnet upgrade checklist

- [ ] Consensus change? Gate it behind an activation height; announce it ahead.
- [ ] Persisted-format change (snapshot, DB CF, blockchain_item, nested chain
      types)? Bump the version, write the migration, and **test it against
      captured real old-version data**.
- [ ] Roll out via coordinated stop → upgrade-all → resume; pin by digest.
- [ ] Back up volumes first; have a rollback.
- [ ] Rehearse the whole thing on testnet against mainnet-shaped state.
- [ ] Prefer: derived state rebuildable from the canonical chain.
