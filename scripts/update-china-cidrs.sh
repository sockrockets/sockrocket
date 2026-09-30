#!/usr/bin/env bash
# Regenerate the built-in China IPv4 CIDR list used by geoip:CN and Merlin ipset.
# Source: 17mon/china_ip_list (APNIC-oriented aggregate). Keeps Merlin package lean
# vs shipping a MaxMind MMDB.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/crates/sockrocket-core/data/china_ipv4_cidrs.inc.rs"
URL="${CHINA_IP_LIST_URL:-https://raw.githubusercontent.com/17mon/china_ip_list/master/china_ip_list.txt}"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

echo "Fetching $URL ..."
curl -fsSL --max-time 120 "$URL" -o "$TMP"

python3 - "$TMP" "$OUT" <<'PY'
import ipaddress, sys
from pathlib import Path
src, dest = Path(sys.argv[1]), Path(sys.argv[2])
cidrs = []
for line in src.read_text().splitlines():
    line = line.strip()
    if not line or line.startswith("#"):
        continue
    try:
        net = ipaddress.ip_network(line, strict=False)
    except ValueError as e:
        raise SystemExit(f"invalid CIDR {line!r}: {e}") from e
    if net.version != 4:
        continue
    cidrs.append(str(net))
if len(cidrs) < 1000:
    raise SystemExit(f"too few CIDRs ({len(cidrs)}); aborting")
dest.parent.mkdir(parents=True, exist_ok=True)
body = ["[\n"]
body.extend(f'    "{c}",\n' for c in cidrs)
body.append("]\n")
dest.write_text("".join(body))
print(f"Wrote {len(cidrs)} CIDRs → {dest}")
PY

echo "Done. Rebuild sockrocket-core to pick up the new list."
