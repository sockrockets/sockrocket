#!/usr/bin/env python3
"""Merge APNIC CN IPv6 allocations and china-operator-ip routes for GeoIP and Merlin."""
import argparse
import hashlib
import ipaddress
from pathlib import Path
import tempfile
import urllib.request

APNIC = "https://ftp.apnic.net/stats/apnic/delegated-apnic-latest"
OPERATOR = "https://raw.githubusercontent.com/gaoyifan/china-operator-ip/ip-lists/china6.txt"
MAX_PREFIXES = 5000
GLOBAL_UNICAST = ipaddress.IPv6Network("2000::/3")


def read_source(path, url):
    if path:
        return path.read_bytes()
    limit = 16 * 1024 * 1024
    with urllib.request.urlopen(url, timeout=45) as response:
        data = response.read(limit + 1)
    if len(data) > limit:
        raise ValueError(f"Input exceeds download limit: {url}")
    return data


def public_prefix(cidr):
    network = ipaddress.IPv6Network(cidr)
    if network.prefixlen < 16 or not network.subnet_of(GLOBAL_UNICAST):
        raise ValueError(f"Unexpected public IPv6 prefix: {cidr}")
    return network


def apnic_prefixes(data):
    networks = []
    for line in data.decode("utf-8").splitlines():
        row = line.split("|")
        if len(row) >= 7 and row[:3] == ["apnic", "CN", "ipv6"] and row[6] in ("allocated", "assigned"):
            networks.append(public_prefix(row[3] + "/" + row[4]))
    if not 100 <= len(networks) <= 50000:
        raise ValueError("Unexpected APNIC CN IPv6 allocation count")
    return networks


def operator_prefixes(data):
    networks = []
    for line in data.decode("utf-8").splitlines():
        cidr = line.partition("#")[0].strip()
        if cidr:
            networks.append(public_prefix(cidr))
    if not 100 <= len(networks) <= 50000:
        raise ValueError("Unexpected china-operator-ip IPv6 route count")
    return networks


def generate(allocations, apnic_digest, operator_data):
    routes = operator_prefixes(operator_data)
    networks = list(ipaddress.collapse_addresses([*allocations, *routes]))
    if len(networks) > MAX_PREFIXES:
        raise ValueError(f"Merged {len(networks)} IPv6 prefixes, exceeding the {MAX_PREFIXES} limit")
    text = f"// APNIC: {APNIC}\n// APNIC SHA-256: {apnic_digest}\n"
    text += "// APNIC statistics identify original allocations, not guaranteed current location.\n"
    text += "// APNIC makes no accuracy guarantee and is not responsible for loss or damage.\n"
    text += f"// Operator routes: {OPERATOR}\n"
    text += f"// Operator SHA-256: {hashlib.sha256(operator_data).hexdigest()}\n"
    text += "[\n" + "".join(f'    "{network}",\n' for network in networks) + "]\n"
    return text, len(networks)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, help="Use a downloaded APNIC statistics file")
    parser.add_argument("--operator-input", type=Path, help="Use a downloaded china6.txt")
    args = parser.parse_args()
    data = read_source(args.input, APNIC)
    text, count = generate(apnic_prefixes(data), hashlib.sha256(data).hexdigest(),
                           read_source(args.operator_input, OPERATOR))
    output = Path(__file__).resolve().parents[1] / "crates/sockrocket-core/data/china_ipv6_cidrs.inc.rs"
    with tempfile.NamedTemporaryFile(dir=output.parent, delete=False) as temp:
        temp.write(text.encode("utf-8"))
    temporary = Path(temp.name)
    temporary.chmod(output.stat().st_mode & 0o777 if output.exists() else 0o644)
    temporary.replace(output)
    print(f"Generated {count} CN IPv6 prefixes (limit {MAX_PREFIXES})")
