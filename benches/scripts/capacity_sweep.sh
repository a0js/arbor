#!/usr/bin/env bash
# Sweeps entity-count scales through the `capacity` binary, one fresh process
# per scale, and records peak RSS + check()/list_entities() latency for each.
#
# Each scale runs as its own process (not looped in-process) so that a scale
# large enough to OOM only kills that one run — the shell script observes the
# non-zero/killed exit status and stops, which is how we find the practical
# entity-count ceiling on the current machine.
#
# Usage: benches/scripts/capacity_sweep.sh [output.csv]

set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT="${1:-$REPO_ROOT/benches/capacity_results.csv}"

cargo build --release --bin capacity -p arbor-bench --manifest-path "$REPO_ROOT/benches/Cargo.toml"
BIN="$REPO_ROOT/target/release/capacity"

SCALES=(10000 30000 100000 300000 1000000 3000000 10000000 30000000 100000000 300000000)

echo "n_entities,node_count,build_ms,rss_bytes,rss_mb,check_permitted_ns,check_denied_ns,list_resources_ns,list_principals_ns" | tee "$OUT"

for n in "${SCALES[@]}"; do
    echo "=== scale $n ===" >&2
    out=$("$BIN" "$n" 2>&1)
    status=$?

    if [ $status -ne 0 ]; then
        echo "scale $n FAILED (exit $status, likely OOM or crash): $out" >&2
        echo "$n,FAILED,,,,,,,," | tee -a "$OUT"
        echo "Capacity ceiling reached at $n entities on this machine." >&2
        break
    fi

    # out looks like: n_entities=X node_count=Y build_ms=Z rss_bytes=A rss_mb=B ...
    row="$n"
    for field in node_count build_ms rss_bytes rss_mb check_permitted_ns check_denied_ns list_resources_ns list_principals_ns; do
        value=$(echo "$out" | grep -oE "${field}=[0-9.]+" | cut -d= -f2)
        row="$row,$value"
    done
    echo "$row" | tee -a "$OUT"
done

echo "Results written to $OUT" >&2
