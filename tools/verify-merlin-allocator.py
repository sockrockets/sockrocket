#!/usr/bin/env python3
"""Fail closed unless an unstripped ARM64 ELF uses the expected musl allocator.

Checks the actual symbol table, not strings from debug info or a target triple.
Run before stripping; deployment must verify the resulting artifact's digest.
"""
import argparse
import hashlib
import json
from pathlib import Path
import struct


def inspect(path: Path) -> dict:
    data = path.read_bytes()
    if data[:6] != b"\x7fELF\x02\x01":
        raise ValueError("expected a little-endian ELF64 file")
    header = struct.unpack_from("<16sHHIQQQIHHHHHH", data)
    if header[2] != 183:
        raise ValueError("expected AArch64 machine type")
    sections = [
        struct.unpack_from("<IIQQQQIIQQ", data, header[6] + i * header[11])
        for i in range(header[12])
    ]
    symbols = set()
    for section in sections:
        if section[1] != 2:  # SHT_SYMTAB, required before strip
            continue
        strings = sections[section[6]]
        table = data[strings[4] : strings[4] + strings[5]]
        for offset in range(section[4], section[4] + section[5], section[9]):
            name, _, _, index, _, _ = struct.unpack_from("<IBBHQQ", data, offset)
            if index == 0:  # undefined references do not prove the implementation
                continue
            end = table.find(b"\0", name)
            symbols.add(table[name:end].decode("utf-8", errors="replace"))
    if not symbols:
        raise ValueError("symbol table unavailable; verify the unstripped artifact")
    if any("SmpAllocator" in name for name in symbols):
        raise ValueError("Zig SmpAllocator symbols are present")
    required = {"malloc", "free", "realloc", "__malloc_context", "__libc_malloc_impl"}
    missing = required - symbols
    if missing:
        raise ValueError("expected musl allocator symbols missing: " + ", ".join(sorted(missing)))
    return {"result": "passed", "allocator": "musl", "architecture": "aarch64",
            "sha256": hashlib.sha256(data).hexdigest(), "zig_smp_symbols": 0}


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps(inspect(args.binary)))
    except (ValueError, IndexError, struct.error) as error:
        parser.exit(1, f"allocator verification failed: {error}\n")
