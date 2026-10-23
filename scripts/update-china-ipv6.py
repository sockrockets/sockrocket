#!/usr/bin/env python3
"""Generate CN IPv6 allocation prefixes from APNIC public delegated statistics."""
import argparse
import hashlib
import ipaddress
from pathlib import Path
import urllib.request

SOURCE = "https://ftp.apnic.net/stats/apnic/delegated-apnic-latest"


def generate(data):
    networks = []
    for line in data.decode("utf-8").splitlines():
        row = line.split("|")
        if len(row) >= 7 and row[:3] == ["apnic", "CN", "ipv6"] and row[6] in ("allocated", "assigned"):
            network = ipaddress.ip_network(row[3] + "/" + row[4], strict=True)
            if network.version != 6 or network.prefixlen < 16 or network.prefixlen > 128:
                raise ValueError("Invalid IPv6 allocation prefix")
            networks.append(network)
    if not 100 <= len(networks) <= 50000:
        raise ValueError("Unexpected CN IPv6 allocation count; output was not replaced")
    networks = list(ipaddress.collapse_addresses(networks))
    text = "// Source: " + SOURCE + "\n"
    text += "// Source SHA-256: " + hashlib.sha256(data).hexdigest() + "\n"
    text += "// APNIC statistics identify original allocations, not guaranteed current location.\n"
    text += "// APNIC makes no accuracy guarantee and is not responsible for loss or damage.\n"
    text += "[\n" + "".join('    "' + str(network) + '",\n' for network in networks) + "]\n"
    return text, len(networks)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="Use a previously downloaded APNIC statistics file")
    args = parser.parse_args()
    if args.input:
        data = args.input.read_bytes()
    else:
        with urllib.request.urlopen(SOURCE, timeout=45) as response:
            data = response.read(16 * 1024 * 1024 + 1)
        if len(data) > 16 * 1024 * 1024:
            raise ValueError("APNIC input exceeds the download limit")
    text, count = generate(data)
    output = Path(__file__).resolve().parents[1] / "crates/sockrocket-core/data/china_ipv6_cidrs.inc.rs"
    temporary = output.with_suffix(".tmp")
    temporary.write_text(text, encoding="utf-8")
    temporary.replace(output)
    print("Generated", count, "CN IPv6 allocation prefixes")
