#!/usr/bin/env bash
# Regenerate the built-in China IPv4 CIDR list used by geoip:CN and Merlin ipset.
#
# 17mon/china_ip_list is largely stale and misses many mainland prefixes
# (e.g. 1.1.8.0/24), which caused CN IPs to fall through to the proxy.
# We merge BGP-based chnroutes2 with Loyalsoldier's CN text list and
# collapse overlaps so coverage stays current without shipping an MMDB.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="$ROOT/crates/sockrocket-core/data/china_ipv4_cidrs.inc.rs"

CHNROUTES_URL="${CHNROUTES_URL:-https://raw.githubusercontent.com/misakaio/chnroutes2/master/chnroutes.txt}"
LOYAL_URL="${LOYAL_CN_URL:-https://raw.githubusercontent.com/Loyalsoldier/geoip/release/text/cn.txt}"
OPERATOR_URL="${OPERATOR_CN_URL:-https://raw.githubusercontent.com/gaoyifan/china-operator-ip/ip-lists/china.txt}"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

fetch() {
    local url="$1" dest="$2"
    echo "Fetching $url ..."
    curl -fsSL --max-time 120 "$url" -o "$dest"
}

fetch "$CHNROUTES_URL" "$TMPDIR/chnroutes2.txt"
fetch "$LOYAL_URL" "$TMPDIR/loyal-cn.txt"
fetch "$OPERATOR_URL" "$TMPDIR/operator-cn.txt"

python3 - "$TMPDIR" "$OUT" <<'PY'
import ipaddress
import sys
from pathlib import Path

tmpdir, dest = Path(sys.argv[1]), Path(sys.argv[2])

def parse_cidrs(path: Path) -> list[ipaddress.IPv4Network]:
    nets: list[ipaddress.IPv4Network] = []
    for raw in path.read_text(errors="replace").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        # Loyalsoldier / clash: "IP-CIDR,1.2.3.0/24,no-resolve" or bare CIDR
        if "," in line:
            parts = [p.strip() for p in line.split(",")]
            line = parts[1] if parts[0].upper().startswith("IP-CIDR") and len(parts) > 1 else parts[0]
            line = line.removeprefix("IP-CIDR,").removeprefix("IP-CIDR6,")
        try:
            net = ipaddress.ip_network(line, strict=False)
        except ValueError:
            continue
        if net.version != 4:
            continue
        nets.append(net)  # type: ignore[arg-type]
    return nets

sources = {
    "chnroutes2": parse_cidrs(tmpdir / "chnroutes2.txt"),
    "loyalsoldier": parse_cidrs(tmpdir / "loyal-cn.txt"),
    "china-operator-ip": parse_cidrs(tmpdir / "operator-cn.txt"),
}
for name, nets in sources.items():
    print(f"  {name}: {len(nets)} IPv4 prefixes")

merged = []
for nets in sources.values():
    merged.extend(nets)
collapsed = list(ipaddress.collapse_addresses(sorted(merged)))
if len(collapsed) < 1000:
    raise SystemExit(f"too few CIDRs after merge ({len(collapsed)}); aborting")

dest.parent.mkdir(parents=True, exist_ok=True)
body = ["[\n"]
body.extend(f'    "{c}",\n' for c in collapsed)
body.append("]\n")
dest.write_text("".join(body))
print(f"Wrote {len(collapsed)} collapsed CIDRs → {dest}")
PY

echo "Done. Rebuild sockrocket-core / sockrocket-cli to pick up the new list."
