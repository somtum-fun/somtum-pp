#!/usr/bin/env bash
# Compares somtum-pp with the server's pp-sidecar on the given .osu files (named <beatmapId>.osu).
# usage: scripts/parity.sh [sidecar-url] maps/*.osu      (default url http://localhost:9000/pp)
# The sidecar pays only Relax/Autopilot on the server, but it computes every mods combo the same
# way as somtum-pp's Relax/Autopilot routes; vanilla rows are skipped (the server pays vanilla in
# process with ppy's own calculator).
set -euo pipefail
url=http://localhost:9000/pp
[[ ${1:-} == http* ]] && { url=$1; shift; }
cd "$(dirname "$0")/.."
cargo build -q --release --example pp
B=target/release/examples/pp
pass=0; fail=0
for f in "$@"; do
  id=$(basename "$f" .osu); objs=$(awk '/\[HitObjects\]/{h=1;next} h&&NF' "$f" | wc -l)
  for mods in 128 136 192 144 8192 8200 8256 1152; do
    for hits in "$objs 0 0 0" "$((objs-12)) 8 2 2" "$((objs-40)) 30 5 5"; do
      read -r n300 n100 n50 miss <<<"$hits"; combo=$(( n300 > 200 ? 400 : n300 ))
      ours=$($B "$f" 0 $mods $n300 $n100 $n50 $miss $combo)
      theirs=$(curl -fsS -XPOST "$url" -H 'content-type: application/json' -d "{\"beatmapId\":$id,\"mode\":0,\"mods\":$mods,\"n300\":$n300,\"n100\":$n100,\"n50\":$n50,\"nGeki\":0,\"nKatu\":0,\"nMiss\":$miss,\"maxCombo\":$combo,\"passed\":true}")
      a=$(jq .pp <<<"$ours"); b=$(jq .pp <<<"$theirs")
      if python3 -c "import sys;a,b=$a,$b;sys.exit(0 if abs(a-b)<=1e-9*max(1,abs(b)) else 1)"; then
        pass=$((pass+1))
      else
        fail=$((fail+1)); echo "DIFF $id mods=$mods hits=$hits ours=$ours sidecar=$theirs"
      fi
    done
  done
done
echo "parity: $pass equal, $fail different"
[[ $fail == 0 ]]
