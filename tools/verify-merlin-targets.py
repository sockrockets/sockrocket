"""Check firmware ABI and standalone executability before packing Merlin binaries."""
from pathlib import Path
import struct
import sys


def check(path, elf_class, machine, float_abi=None):
    with Path(path).open('rb') as binary:
        header = binary.read(64)
        if (len(header) < 64 or header[:4] != b'\x7fELF'
                or header[4:7] != bytes([elf_class, 1, 1])
                or struct.unpack_from('<H', header, 16)[0] not in (2, 3)
                or struct.unpack_from('<H', header, 18)[0] != machine):
            raise SystemExit(f'{path}: unexpected ELF architecture or executable type')

        if elf_class == 1:
            flags = struct.unpack_from('<I', header, 36)[0]
            if flags >> 24 != 5 or flags & 0x600 != float_abi:
                raise SystemExit(f'{path}: unexpected ARM EABI or float ABI')
            offset = struct.unpack_from('<I', header, 28)[0]
            size, count = struct.unpack_from('<HH', header, 42)
        else:
            offset = struct.unpack_from('<Q', header, 32)[0]
            size, count = struct.unpack_from('<HH', header, 54)

        if size != (32 if elf_class == 1 else 56) or count == 0:
            raise SystemExit(f'{path}: invalid program header table')
        binary.seek(offset)
        table = binary.read(size * count)
        if len(table) != size * count:
            raise SystemExit(f'{path}: truncated program header table')
        types = {struct.unpack_from('<I', table, i)[0] for i in range(0, len(table), size)}
        if 1 not in types or 3 in types:  # PT_LOAD required, PT_INTERP needs an external loader
            raise SystemExit(f'{path}: expected a standalone executable')


if __name__ == '__main__':
    arm64, armhf, armsf = sys.argv[1:]
    check(arm64, 2, 183)         # ELF64, EM_AARCH64
    check(armhf, 1, 40, 0x400)   # ELF32, EM_ARM, EF_ARM_ABI_FLOAT_HARD
    check(armsf, 1, 40, 0x200)   # ELF32, EM_ARM, EF_ARM_ABI_FLOAT_SOFT
