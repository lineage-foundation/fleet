#!/usr/bin/env bash
set -euo pipefail

# ops/deploy-testnet.sh — guarded canary deploy of a signed fleet release onto the live
# testnet (Railway project "fleet", environment "testnet": 3 mempool + 3 storage + 3 miner
# services). This touches a LIVE chain. Read this whole header before changing anything.
#
# Flow:
#   1. Pre-flight: resolve the testnet service map from the Railway API (no IDs are
#      hardcoded — everything is looked up by name at runtime, see "Runtime resolution"
#      below). cosign-verify the target images are signed by fleet's own release workflow.
#      Confirm the chain is healthy *before* touching anything (never deploy onto a chain
#      that's already broken).
#   2. Canary: deploy to miner-2 alone (miners are not a RAFT group, so this is the lowest
#      blast-radius single node), wait for it to warm up, then verify chain health.
#   3. Roll the remaining 8 services ONE AT A TIME in a fixed order (remaining miners, then
#      storage one-by-one, then mempool one-by-one), verifying chain health after every
#      single node. Storage and mempool are each a 3-node RAFT group (quorum = 2/3) — this
#      script never has more than one node of a given RAFT tier mid-redeploy at once.
#   4. On any failed verification: roll back every node this run has changed (in reverse
#      order) to the image it was running before this run touched it, then exit non-zero.
#      Nothing is left half-rolled silently.
#
# Health check (from the SRE runbook — the "identical hash" part is load-bearing, matching
# heights alone is a false positive): a sample of storage-0/1/2's public
# `/v1/blocks/latest` is "healthy" only if all three are reachable, report the SAME block
# height AND the SAME consensus hash (`nonce_and_mining_tx_hash[1]`) at that height, and a
# second sample ~30-90s later shows the height has strictly advanced (with the same
# all-three-identical check repeated). Unreachable, flat, or diverged => unhealthy.
#
# Runtime resolution (PUBLIC REPO — no project/environment/service UUIDs or tokens are ever
# hardcoded here): every ID is looked up from the Railway API by name ("fleet" project,
# "testnet" environment, the 9 public service names) at the start of every run. The only
# hardcoded identifiers are the public *.lineage.to RPC hostnames and the public
# mempool/storage/miner service-name scheme, both already public.
#
# Mainnet guard (structural, not a flag): this script looks up exactly one environment, by
# the literal name "testnet", and only ever mutates services found inside it. There is no
# parameter, env var, or code path anywhere below that accepts a different environment name
# — if a "mainnet" (or any other) environment exists in the project, it is never queried for
# mutation targets and is structurally unreachable from this script.
#
# Image tag note: the release/workflow-dispatch version is conventionally written vX.Y.Z,
# but images.yml tags GHCR pushes via `docker/metadata-action`'s
# `type=semver,pattern={{version}}`, which parses the tag through the `semver` package and
# emits bare `major.minor.patch` — the leading "v" is NOT part of the pushed image tag
# (verified: `semver.parse('v1.2.3').version === '1.2.3'`). We accept vX.Y.Z (or X.Y.Z) as
# input and strip a leading "v" before building any `ghcr.io/...:<tag>` reference, so the
# image reference we deploy actually exists.
#
# Usage:
#   RAILWAY_API_TOKEN=... ops/deploy-testnet.sh vX.Y.Z
#   VERSION=vX.Y.Z RAILWAY_API_TOKEN=... ops/deploy-testnet.sh
#   DRY_RUN=1 RAILWAY_API_TOKEN=... ops/deploy-testnet.sh vX.Y.Z   # resolve + print the
#                                                                   # plan, zero mutations.
#
# Required env:
#   RAILWAY_API_TOKEN  Railway API token for the fleet project (Actions secret). Presence is
#                       checked before any network call is made; the script refuses to run
#                       without it, dry run or not.
# Optional env:
#   DRY_RUN                    "1" to resolve + print the plan only (default "0").
#   HEALTH_SAMPLE_GAP_SECONDS  seconds between the two health samples (default 45; runbook
#                               window is 30-90s).
#   WARMUP_SECONDS             seconds to wait after a redeploy before probing health
#                               (default 20).
#   CURL_MAX_TIME               per-request curl timeout in seconds (default 10).

# ---------------------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------------------
readonly RAILWAY_API="https://backboard.railway.app/graphql/v2"
# Railway fronts this endpoint with Cloudflare, which 1010s requests with no/blank UA.
readonly RAILWAY_USER_AGENT="Mozilla/5.0 (compatible; lineage-fleet-deploy-testnet/1.0; +https://github.com/lineage-foundation/fleet)"
readonly PROJECT_NAME="fleet"
readonly TESTNET_ENV_NAME="testnet"
readonly GHCR_OWNER="lineage-foundation"
readonly COSIGN_IDENTITY_REGEXP='https://github.com/lineage-foundation/fleet/.*'
readonly COSIGN_OIDC_ISSUER='https://token.actions.githubusercontent.com'

readonly STORAGE_HOSTS=(storage-0.lineage.to storage-1.lineage.to storage-2.lineage.to)

readonly ALL_SERVICES=(miner-0 miner-1 miner-2 storage-0 storage-1 storage-2 mempool-0 mempool-1 mempool-2)
# Canary (miner-2) first, then the rest of the miners, then storage one-by-one, then mempool
# one-by-one. This order guarantees at most one node of any RAFT tier (storage, mempool) is
# ever mid-redeploy at the same time.
readonly DEPLOY_ORDER=(miner-2 miner-0 miner-1 storage-0 storage-1 storage-2 mempool-0 mempool-1 mempool-2)

HEALTH_SAMPLE_GAP_SECONDS="${HEALTH_SAMPLE_GAP_SECONDS:-45}"
WARMUP_SECONDS="${WARMUP_SECONDS:-20}"
CURL_MAX_TIME="${CURL_MAX_TIME:-10}"
DRY_RUN="${DRY_RUN:-0}"

declare -gA SERVICE_ID=()
declare -gA CURRENT_IMAGE=()
declare -gA TARGET_IMAGE=()
TESTNET_ENV_ID=""
# Nodes this run has mutated the Railway-side image field for, in the order it touched them —
# appended by deploy_node() the instant service_set_image succeeds (before redeploy is even
# attempted), so a set-succeeded/redeploy-failed partial mutation is tracked for rollback too,
# not just a fully successful deploy. Rolled back in reverse order on any failure.
declare -ga ROLLED=()

# ---------------------------------------------------------------------------------------
# Small helpers
# ---------------------------------------------------------------------------------------
log() {
  # Always stderr: several functions below are invoked via command substitution to capture
  # a return value on stdout (e.g. `id=$(resolve_project_id)`) — a log line on stdout would
  # silently get swallowed into that captured value instead of reaching the console/CI log.
  printf '[deploy-testnet] %s %s\n' "$(date -u +%H:%M:%S)" "$*" >&2
}

short_id() {
  local id="$1"
  printf '%s…' "${id:0:8}"
}

image_family() {
  case "$1" in
    mempool-*) printf 'mempool' ;;
    storage-*) printf 'storage' ;;
    miner-*) printf 'miner' ;;
    *)
      log "ERROR: unknown service family for '$1'"
      return 1
      ;;
  esac
}

normalize_version() {
  local v="$1"
  if [[ ! "$v" =~ ^v?[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    log "ERROR: version '$v' is not a valid vX.Y.Z / X.Y.Z release version"
    return 1
  fi
  printf '%s' "${v#v}"
}

# ---------------------------------------------------------------------------------------
# Railway GraphQL
# ---------------------------------------------------------------------------------------
gql() {
  # $1 = query string, $2 = variables json (optional, default {})
  # NB: do NOT write `vars="${2:-{}}"` — bash parses that as `${2:-{}` + a literal
  # `}`, so when $2 is set the value gets a spurious trailing `}` (invalid JSON that
  # jq --argjson rejects); when $2 is unset it happens to yield `{}`, masking the bug.
  local query="$1" vars="{}" body resp
  [[ $# -ge 2 && -n "$2" ]] && vars="$2"
  body=$(jq -n --arg q "$query" --argjson v "$vars" '{query: $q, variables: $v}')
  if ! resp=$(curl -sS --max-time "$CURL_MAX_TIME" \
      -H "Authorization: Bearer ${RAILWAY_API_TOKEN}" \
      -H "Content-Type: application/json" \
      -H "User-Agent: ${RAILWAY_USER_AGENT}" \
      -d "$body" \
      "$RAILWAY_API"); then
    log "ERROR: Railway API request failed (network)"
    return 1
  fi
  if [[ -z "$resp" ]]; then
    log "ERROR: empty response from Railway API"
    return 1
  fi
  if ! jq -e . >/dev/null 2>&1 <<<"$resp"; then
    log "ERROR: non-JSON response from Railway API: ${resp:0:200}"
    return 1
  fi
  if jq -e 'has("errors")' >/dev/null 2>&1 <<<"$resp"; then
    log "ERROR: Railway API returned errors:"
    jq -r '.errors[]? | "  - " + (.message // tostring)' <<<"$resp" >&2
    return 1
  fi
  printf '%s' "$resp"
}

resolve_project_id() {
  local resp pid
  # Token-type robust: a *workspace* token (the CI/least-privilege choice) lists
  # projects at the top level, and its `me` query returns "Not Authorized" (a
  # workspace token is not a user). An *account/personal* token lists projects
  # under `me.workspaces` and returns nothing at the top level. Try the workspace
  # path first, then fall back to the personal path, so either token type works.
  resp=$(gql 'query { projects { edges { node { id name } } } }') || resp=""
  pid=$(jq -r --arg name "$PROJECT_NAME" '
    [.data.projects.edges[]?.node | select(.name == $name)] | .[0].id // empty
  ' <<<"$resp" 2>/dev/null)

  if [[ -z "$pid" ]]; then
    resp=$(gql 'query { me { workspaces { projects { edges { node { id name } } } } } }') || resp=""
    pid=$(jq -r --arg name "$PROJECT_NAME" '
      [.data.me.workspaces[]?.projects.edges[]?.node | select(.name == $name)] | .[0].id // empty
    ' <<<"$resp" 2>/dev/null)
  fi

  if [[ -z "$pid" ]]; then
    log "ERROR: no Railway project named '$PROJECT_NAME' visible to this token (tried workspace + account queries)"
    return 1
  fi
  printf '%s' "$pid"
}

resolve_project() {
  local pid="$1"
  # shellcheck disable=SC2016  # single quotes intentional: this is a GraphQL variable ($id/$env/...), not shell expansion
  gql 'query($id: String!) {
    project(id: $id) {
      environments { edges { node { id name } } }
      services {
        edges {
          node {
            id
            name
            serviceInstances { edges { node { environmentId source { image } } } }
          }
        }
      }
    }
  }' "$(jq -n --arg id "$pid" '{id: $id}')"
}

service_set_image() {
  local svc_id="$1" image="$2"
  # shellcheck disable=SC2016  # single quotes intentional: this is a GraphQL variable ($id/$env/...), not shell expansion
  gql 'mutation($env: String!, $svc: String!, $image: String!) {
    serviceInstanceUpdate(environmentId: $env, serviceId: $svc, input: { source: { image: $image } })
  }' "$(jq -n --arg env "$TESTNET_ENV_ID" --arg svc "$svc_id" --arg image "$image" '{env: $env, svc: $svc, image: $image}')"
}

# Deploy the service's CURRENT config (i.e. the image just set by service_set_image).
# NB: use serviceInstanceDeployV2, NOT serviceInstanceRedeploy — the latter re-runs
# the service's PREVIOUS deployment (its old image tag), so after changing source.image
# it would redeploy the OLD tag and silently leave the node on the wrong version.
# serviceInstanceDeployV2 (commitSha optional) creates a fresh deployment from the
# current config, which is what actually rolls the new image tag.
service_deploy() {
  local svc_id="$1"
  # shellcheck disable=SC2016  # single quotes intentional: this is a GraphQL variable ($id/$env/...), not shell expansion
  gql 'mutation($env: String!, $svc: String!) {
    serviceInstanceDeployV2(environmentId: $env, serviceId: $svc)
  }' "$(jq -n --arg env "$TESTNET_ENV_ID" --arg svc "$svc_id" '{env: $env, svc: $svc}')"
}

# Poll a service's latest deployment until it reaches SUCCESS running the EXPECTED
# image, or fail. This is the guard against "config says X but the running deployment
# is still Y": we confirm the node actually deployed the target tag before trusting it
# and before the chain-health check. Returns 0 only when the newest deployment is
# SUCCESS and its image == $expected_image.
verify_node_image() {
  local svc_id="$1" expected_image="$2" waited=0 timeout="${DEPLOY_VERIFY_TIMEOUT:-300}"
  while [ "$waited" -lt "$timeout" ]; do
    local resp status image
    # shellcheck disable=SC2016
    resp=$(gql 'query($env: String!, $svc: String!) {
      deployments(first: 1, input: { environmentId: $env, serviceId: $svc }) {
        edges { node { status meta } }
      }
    }' "$(jq -n --arg env "$TESTNET_ENV_ID" --arg svc "$svc_id" '{env: $env, svc: $svc}')") || { sleep 10; waited=$((waited + 10)); continue; }
    status=$(jq -r '.data.deployments.edges[0].node.status // empty' <<<"$resp" 2>/dev/null)
    image=$(jq -r '.data.deployments.edges[0].node.meta.image // empty' <<<"$resp" 2>/dev/null)
    case "$status" in
      SUCCESS)
        if [ "$image" = "$expected_image" ]; then return 0; fi
        log "  deployment SUCCESS but image is '$image', expected '$expected_image'"
        return 1
        ;;
      FAILED|CRASHED|REMOVED)
        log "  deployment status '$status' (image '$image')"
        return 1
        ;;
    esac
    sleep 10
    waited=$((waited + 10))
  done
  log "  timed out after ${timeout}s waiting for deployment to reach SUCCESS on $expected_image"
  return 1
}

# ---------------------------------------------------------------------------------------
# Chain health
# ---------------------------------------------------------------------------------------
fetch_block() {
  # $1 = hostname; prints "height<TAB>hash" on success
  local host="$1" resp
  resp=$(curl -sS --max-time "$CURL_MAX_TIME" "https://${host}/v1/blocks/latest") || return 1
  jq -e -r '[.block.block.header.b_num, .block.block.header.nonce_and_mining_tx_hash[1]] | @tsv' <<<"$resp" 2>/dev/null
}

chain_sample() {
  # Populates the caller's HEIGHTS[]/HASHES[] arrays (bash dynamic scoping). Returns 1 if
  # any storage host is unreachable or returns an unparsable body.
  local i=0 host line h has
  for host in "${STORAGE_HOSTS[@]}"; do
    if line=$(fetch_block "$host"); then
      IFS=$'\t' read -r h has <<<"$line"
      HEIGHTS[i]="$h"
      HASHES[i]="$has"
    else
      log "  $host: unreachable"
      return 1
    fi
    i=$((i + 1))
  done
  return 0
}

sample_consistent() {
  [[ -n "${HEIGHTS[0]:-}" && -n "${HEIGHTS[1]:-}" && -n "${HEIGHTS[2]:-}" ]] || return 1
  [[ "${HEIGHTS[0]}" == "${HEIGHTS[1]}" && "${HEIGHTS[1]}" == "${HEIGHTS[2]}" ]] || return 1
  [[ -n "${HASHES[0]:-}" && "${HASHES[0]}" == "${HASHES[1]:-}" && "${HASHES[1]}" == "${HASHES[2]:-}" ]] || return 1
  return 0
}

verify_chain_health() {
  local label="$1"
  local -a HEIGHTS=() HASHES=()

  log "health[$label]: sampling storage-0/1/2 (t0)"
  if ! chain_sample; then
    log "UNHEALTHY[$label]: one or more storage RPCs unreachable at t0"
    return 1
  fi
  if ! sample_consistent; then
    log "UNHEALTHY[$label]: heights/hashes diverge at t0 (heights=${HEIGHTS[*]:-})"
    return 1
  fi
  local height_t0="${HEIGHTS[0]}"
  log "health[$label]: t0 height=$height_t0 hash=${HASHES[0]:0:12}... — waiting ${HEALTH_SAMPLE_GAP_SECONDS}s to confirm it advances"
  sleep "$HEALTH_SAMPLE_GAP_SECONDS"

  log "health[$label]: sampling storage-0/1/2 (t1)"
  if ! chain_sample; then
    log "UNHEALTHY[$label]: one or more storage RPCs unreachable at t1"
    return 1
  fi
  if ! sample_consistent; then
    log "UNHEALTHY[$label]: heights/hashes diverge at t1 (heights=${HEIGHTS[*]:-})"
    return 1
  fi
  local height_t1="${HEIGHTS[0]}"
  if ! (( height_t1 > height_t0 )); then
    log "UNHEALTHY[$label]: height not advancing (t0=$height_t0 t1=$height_t1)"
    return 1
  fi

  log "HEALTHY[$label]: height advanced $height_t0 -> $height_t1, consensus hash identical across storage-0/1/2 at both samples"
  return 0
}

# ---------------------------------------------------------------------------------------
# Signing
# ---------------------------------------------------------------------------------------
cosign_verify_image() {
  local image_ref="$1"
  if ! command -v cosign >/dev/null 2>&1; then
    if [[ "$DRY_RUN" == "1" ]]; then
      log "WARN: cosign not installed; skipping signature check for $image_ref (DRY_RUN only — a real run REQUIRES cosign)"
      return 0
    fi
    log "ERROR: cosign not installed; refusing to deploy an unverified image ($image_ref)"
    return 1
  fi
  log "cosign verify: $image_ref"
  if ! cosign verify \
      --certificate-identity-regexp "$COSIGN_IDENTITY_REGEXP" \
      --certificate-oidc-issuer "$COSIGN_OIDC_ISSUER" \
      "$image_ref" >/dev/null 2>&1; then
    log "ERROR: cosign signature verification FAILED for $image_ref"
    return 1
  fi
  log "OK: $image_ref is signed by fleet's release workflow"
  return 0
}

# ---------------------------------------------------------------------------------------
# Deploy / rollback
# ---------------------------------------------------------------------------------------
deploy_node() {
  local svc="$1" image="$2"
  log "-> deploying $svc to $image"
  service_set_image "${SERVICE_ID[$svc]}" "$image" >/dev/null || return 1
  # The image field is now changed on Railway even if the deploy trigger below fails —
  # record that immediately so the caller's rollback covers this node either way.
  ROLLED+=("$svc")
  service_deploy "${SERVICE_ID[$svc]}" >/dev/null || return 1
  # Confirm the node actually deployed the target image (not a redeploy of the old
  # tag) and reached SUCCESS before we trust it / check chain health.
  if ! verify_node_image "${SERVICE_ID[$svc]}" "$image"; then
    log "ERROR: $svc did not come up running $image"
    return 1
  fi
  log "   $svc deployment SUCCESS on $image"
  return 0
}

rollback_node() {
  local svc="$1"
  local prior="${CURRENT_IMAGE[$svc]:-}"
  if [[ -z "$prior" ]]; then
    log "ERROR: no captured prior image for $svc; CANNOT roll back automatically — manual fix required"
    return 1
  fi
  log "<- rolling back $svc to prior image $prior"
  if ! service_set_image "${SERVICE_ID[$svc]}" "$prior" >/dev/null; then
    log "ERROR: rollback image-set failed for $svc"
    return 1
  fi
  if ! service_deploy "${SERVICE_ID[$svc]}" >/dev/null; then
    log "ERROR: rollback deploy failed for $svc"
    return 1
  fi
  return 0
}

print_plan() {
  log "--- DRY RUN PLAN (zero mutations made) ---"
  log "project: $PROJECT_NAME   environment: $TESTNET_ENV_NAME"
  log "target images:"
  local fam
  for fam in mempool storage miner; do
    log "  $fam -> ${TARGET_IMAGE[$fam]}"
  done
  log "deploy order (1 = canary, then one node at a time; never 2/3 of a RAFT tier down):"
  local n=1 svc tag
  for svc in "${DEPLOY_ORDER[@]}"; do
    tag=""
    [[ "$svc" == "${DEPLOY_ORDER[0]}" ]] && tag=" (CANARY)"
    log "  $n. $svc : ${CURRENT_IMAGE[$svc]:-<unknown>} -> ${TARGET_IMAGE[$(image_family "$svc")]}$tag"
    n=$((n + 1))
  done
  log "verify between every step: storage-0/1/2 /v1/blocks/latest must be reachable, report the"
  log "  SAME height AND the SAME consensus hash (nonce_and_mining_tx_hash[1]) across all three,"
  log "  at two samples ${HEALTH_SAMPLE_GAP_SECONDS}s apart, with height strictly advancing between them."
  log "on any failed verify: roll back every node changed so far (reverse order) to its captured"
  log "  prior image, redeploy, re-verify, then exit non-zero."
}

# ---------------------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------------------
main() {
  local raw_version="${1:-${VERSION:-}}"

  if [[ -z "$raw_version" ]]; then
    log "ERROR: version required (arg 1 or \$VERSION), e.g. v1.4.2"
    exit 1
  fi

  # Fails safe before any network call: no token, no run — dry run or not.
  if [[ -z "${RAILWAY_API_TOKEN:-}" ]]; then
    log "ERROR: RAILWAY_API_TOKEN is not set. Refusing to run (no network calls made, zero mutations)."
    exit 1
  fi

  local bin
  for bin in curl jq; do
    command -v "$bin" >/dev/null 2>&1 || { log "ERROR: required tool '$bin' not found"; exit 1; }
  done

  local image_tag
  image_tag=$(normalize_version "$raw_version") || exit 1

  log "=== fleet testnet canary deploy ==="
  log "mode: $([[ "$DRY_RUN" == "1" ]] && echo DRY_RUN || echo LIVE)"
  log "release version: $raw_version  ->  GHCR image tag: $image_tag"

  log "resolving Railway project/environment/service map (runtime lookup, nothing hardcoded)..."
  local project_id
  project_id=$(resolve_project_id) || exit 1
  log "project '$PROJECT_NAME' resolved: $(short_id "$project_id")"

  local resp
  resp=$(resolve_project "$project_id") || exit 1

  TESTNET_ENV_ID=$(jq -r --arg n "$TESTNET_ENV_NAME" '
    [.data.project.environments.edges[]?.node | select(.name == $n)] | .[0].id // empty
  ' <<<"$resp")
  if [[ -z "$TESTNET_ENV_ID" ]]; then
    log "ERROR: no environment named '$TESTNET_ENV_NAME' in project '$PROJECT_NAME'"
    exit 1
  fi
  # Structural mainnet guard: the only environment ever looked up or referenced anywhere in
  # this script is the one whose name literally equals "testnet" (above). Any other
  # environment present in the project (e.g. a future "mainnet") is never queried as a
  # mutation target by any code path here.
  log "environment '$TESTNET_ENV_NAME' resolved: $(short_id "$TESTNET_ENV_ID")"

  local svc_name svc_id svc_image
  while IFS=$'\t' read -r svc_name svc_id svc_image; do
    [[ -n "$svc_name" ]] || continue
    SERVICE_ID["$svc_name"]="$svc_id"
    CURRENT_IMAGE["$svc_name"]="$svc_image"
  done < <(jq -r --arg env "$TESTNET_ENV_ID" '
    .data.project.services.edges[]?.node
    | . as $s
    | ($s.serviceInstances.edges[]? | select(.node.environmentId == $env) | .node) as $inst
    | select($inst != null)
    | [$s.name, $s.id, ($inst.source.image // "")] | @tsv
  ' <<<"$resp")

  local missing=() svc
  for svc in "${ALL_SERVICES[@]}"; do
    [[ -n "${SERVICE_ID[$svc]:-}" ]] || missing+=("$svc")
  done
  if (( ${#missing[@]} > 0 )); then
    log "ERROR: could not resolve service(s) in '$TESTNET_ENV_NAME': ${missing[*]}"
    log "ABORT: incomplete topology — refusing to deploy partially. No mutations made."
    exit 1
  fi
  log "resolved all ${#ALL_SERVICES[@]} testnet services"

  local fam
  for fam in mempool storage miner; do
    TARGET_IMAGE["$fam"]="ghcr.io/${GHCR_OWNER}/${fam}:${image_tag}"
  done

  log "--- pre-flight: cosign signature verification ---"
  for fam in mempool storage miner; do
    cosign_verify_image "${TARGET_IMAGE[$fam]}" || {
      log "ABORT: signature verification failed; no mutations made"
      exit 1
    }
  done

  if [[ "$DRY_RUN" == "1" ]]; then
    print_plan
    log "DRY_RUN=1: plan resolved, zero mutations made. Exiting 0."
    exit 0
  fi

  log "--- pre-flight: baseline chain health ---"
  if ! verify_chain_health "baseline"; then
    log "ABORT: chain is already unhealthy — refusing to deploy onto a broken chain. No mutations made."
    exit 1
  fi

  ROLLED=()
  local failed=0 failed_node=""
  for svc in "${DEPLOY_ORDER[@]}"; do
    fam=$(image_family "$svc") || { failed=1; failed_node="$svc"; break; }
    local img="${TARGET_IMAGE[$fam]}"
    log "=== node: $svc  current: ${CURRENT_IMAGE[$svc]:-<unknown>}  target: $img ==="
    if ! deploy_node "$svc" "$img"; then
      log "ERROR: deploy mutation failed for $svc"
      failed=1
      failed_node="$svc"
      break
    fi
    log "warmup ${WARMUP_SECONDS}s for $svc to restart and rejoin its peers..."
    sleep "$WARMUP_SECONDS"
    if ! verify_chain_health "after:$svc"; then
      failed=1
      failed_node="$svc"
      break
    fi
    log "$svc verified healthy on $image_tag"
  done

  if (( failed == 1 )); then
    log "!!! DEPLOY REGRESSED at node '$failed_node' — rolling back ${#ROLLED[@]} changed node(s): ${ROLLED[*]:-none}"
    local rb_ok=1 i
    for (( i = ${#ROLLED[@]} - 1; i >= 0; i-- )); do
      rollback_node "${ROLLED[$i]}" || rb_ok=0
    done
    if (( rb_ok == 1 )); then
      log "rollback mutations issued for: ${ROLLED[*]:-none}"
      log "warmup ${WARMUP_SECONDS}s before re-checking health post-rollback..."
      sleep "$WARMUP_SECONDS"
      if verify_chain_health "post-rollback"; then
        log "chain confirmed healthy after rollback."
      else
        log "WARNING: chain not confirmed healthy after rollback — manual investigation required NOW."
      fi
    else
      log "CRITICAL: one or more rollback mutations FAILED — the chain may be left in a MIXED-VERSION state. Manual intervention required immediately."
    fi
    log "=== DEPLOY FAILED: $raw_version was NOT fully rolled out (failed at $failed_node). See log above for rollback status. ==="
    exit 1
  fi

  log "=== DEPLOY SUCCEEDED: all ${#ALL_SERVICES[@]} testnet services now on $raw_version ($image_tag) ==="
}

# Guarded so this file can be `source`d (e.g. by a test harness exercising the helper
# functions in isolation) without kicking off main() as a side effect.
if [[ "${BASH_SOURCE[0]}" == "${0}" ]]; then
  main "$@"
fi
