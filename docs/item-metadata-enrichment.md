# Item metadata auto-enrichment — design

## Goal

Make an item's metadata easy to read after it changes ownership, by resolving it
from the item's genesis and auto-attaching it in the SDKs.

## Background

Item metadata is attached only at genesis (the create transaction) and is
deliberately dropped on transfer: `tx_is_valid` requires an on-spent item output
to carry a `genesis_hash` and **no** metadata
(`crates/prime/src/utils/script_utils.rs`). This keeps items fungible within a
genesis class (balances are tracked as `genesis_hash -> amount`), immutable, and
small on-chain. The consequence is poor DX: a holder of a transferred item sees
only its `genesis_hash`, not its metadata, and must manually resolve the genesis
create-transaction to recover it.

Metadata is on-chain and immutable, so this is a pure read-side convenience — no
consensus change, no migration.

## Architecture and data flow

A single node resolver turns a `genesis_hash` into the item's genesis facts.
Every SDK auto-enriches item listings by calling it, deduped + cached + parallel.

Flow for a listing (e.g. `GET /v1/balances`):
1. SDK receives the listing (items appear as `genesis_hash` + amount/outpoints).
2. SDK collects the **distinct** `genesis_hash`es.
3. For each not already in its cache, SDK calls the resolver **in parallel**.
4. SDK caches each result (immutable — cached for the client instance's life).
5. SDK attaches `metadata` to each item object and returns the enriched listing.

Enrichment is SDK-side (chosen over node-side): the node stays lean, the cache is
client-local, and enrichment is opt-out per call.

## Component 1 — Node resolver

**Endpoint:** `GET /v1/items/{genesis_hash}`

**Nodes (implemented):** served by the **storage** node only. Implementation
revealed that neither the mempool nor the user node holds historical transactions
(the user node keeps a wallet DB + latest block + UTXO sets, not full history), so
"proxied to storage" is not a light proxy but a real subsystem (request/response
correlation, a new inbound comms message, storage-address discovery, timeouts). It
is therefore **deferred to its own task**. For now the SDKs call the resolver on
the **storage** base URL (e.g. `storage.lineage.to`). Restoring the single-base-URL
DX (resolver on mempool/user, proxied to storage) is a scheduled follow-up.

**Resolve path:** `genesis_hash` is the create-transaction's hash. The storage node
reads the stored create transaction by key from its blockchain DB
(`get_stored_value_from_db`, the existing blockchain-entry lookup; the stored tx is
decoded with `bincode` on `item.data`), extracts the create output, and returns the
fields below. Read-only; the node does not cache (the SDK caches).

**Response body (200):**
```json
{
  "genesis_hash": "g...",
  "metadata": "…" | null,
  "total_amount": 1000,
  "created": { "block_num": 42, "tx_hash": "g..." },
  "creator_address": "…"
}
```
- `total_amount` — the amount minted at genesis (the item class's total supply).
- `created.block_num` / `created.tx_hash` — where/when the class was minted
  (`tx_hash == genesis_hash`).
- `creator_address` — the `script_public_key` of the genesis create output.

**Errors:**
- Unknown `genesis_hash` (no such create transaction) → `404`.
- Item exists but has no metadata → `200` with `metadata: null`.
- Storage unavailable / lookup failure → `502`/`503` (transient; the SDK degrades
  gracefully, see below).

Out of scope for this endpoint: current circulating supply and current holders
(those are heavier UTXO-wide queries, not answerable from the genesis tx).

## Component 2 — SDK auto-enrichment (all six SDKs)

Applies to `sdk-js`, `sdk-python`, `sdk-go`, `sdk-rust`, `sdk-php`, `sdk-laravel`.

**Default behaviour:** auto-enrich item listings. Any SDK call that returns items
(wallet balances / holdings) attaches a `metadata` field to each item object.

**Mechanism per listing:**
1. Collect distinct `genesis_hash`es from the result.
2. Check the in-instance cache; resolve the misses in parallel via the resolver.
3. Cache each result keyed by `genesis_hash` (no expiry — metadata is immutable).
4. Attach `metadata` to each item.

**`getItemInfo(genesisHash)`** — a helper returning the full genesis-facts object
(the resolver response), for callers that want supply/provenance, not just
metadata. Shares the same cache.

**Opt-out:** enrichment is on by default; callers can disable it per call
(e.g. an `enrich: false` option) and/or per client instance. Disabling skips all
resolver calls and returns items with `metadata` absent/unset.

**Graceful degradation:** if a resolve fails (resolver error, 404, or the item
has no metadata), the item's `metadata` is `null` and the listing call still
succeeds. No separate error flag — `null` is the single "no metadata available"
signal. A core listing must never fail because of an optional enrichment.

**Cache:** per SDK-instance, keyed by `genesis_hash`, no TTL (immutable). Not
shared across instances (no persistence in scope).

**php / laravel:** enrichment-only. They call the `/v1` resolver against their
configured node URL and attach `metadata`; they are **not** migrated to `/v1`
otherwise (that remains a separate, deferred project).

## Testing

**Node:**
- Create an item with metadata, transfer it, then resolve `metadata` by
  `genesis_hash` — returns the genesis metadata (and `total_amount`, `created`,
  `creator_address`).
- Unknown `genesis_hash` → `404`.
- Item created without metadata → `200`, `metadata: null`.

**SDKs (each):**
- A listing of transferred items is enriched with `metadata`.
- Exactly one resolver call per distinct `genesis_hash` (dedup + cache verified);
  repeat listings issue no further resolver calls.
- Resolver error during enrichment → items returned with `metadata: null`, call
  succeeds (graceful degrade).
- `enrich: false` issues no resolver calls.
- `getItemInfo` returns the full genesis-facts object.

## Global constraints

- **No consensus change / no migration** — read-side only; `tx_is_valid` and the
  on-chain item model are untouched.
- **Metadata is immutable** — safe to cache indefinitely by `genesis_hash`.
- **Resolver base URL** — for now the resolver is on the **storage** node, so SDKs
  call the storage URL for enrichment (the mempool/user proxy that would restore a
  single base URL is a deferred follow-up).
- **Hand-written style** — match each repo's existing conventions; no AI-authorship
  signals in code, comments, or commit messages.

## Out of scope (deliberate)

- Batch resolver (single-only for now; add if wallets prove item-class-diverse).
- Node-side response enrichment (chose SDK-side).
- Circulating-supply / current-holder queries.
- `php`/`laravel` `/v1` migration.
