#!/usr/bin/env python3
"""逐帧比对 Windows PE 内的图标和正式 ICO，无第三方依赖。

python scripts/verify-windows-icon.py path/to/swoop.exe path/to/swoop-cli.exe
"""

import argparse
from pathlib import Path
import struct


def unpack(data, fmt, offset):
    return struct.unpack_from(fmt, data, offset)


def checked_slice(data, offset, size):
    if offset < 0 or size < 0 or offset + size > len(data):
        raise ValueError("resource extends beyond the file")
    return data[offset:offset + size]


class PeResources:
    def __init__(self, data):
        self.data = data
        if data[:2] != b"MZ":
            raise ValueError("not an EXE: missing DOS signature")
        pe, = unpack(data, "<I", 0x3C)
        if data[pe:pe + 4] != b"PE\0\0":
            raise ValueError("missing PE signature")
        section_count, = unpack(data, "<H", pe + 6)
        optional_size, = unpack(data, "<H", pe + 20)
        optional = pe + 24
        magic, = unpack(data, "<H", optional)
        directories = {0x10B: 96, 0x20B: 112}.get(magic)
        if directories is None:
            raise ValueError("unsupported PE optional header")
        self.resource_rva, self.resource_size = unpack(data, "<II", optional + directories + 16)
        if not self.resource_rva or not self.resource_size:
            raise ValueError("EXE contains no Windows resources")
        self.sections = []
        for index in range(section_count):
            header = optional + optional_size + index * 40
            _, rva, raw_size, raw_offset = unpack(data, "<IIII", header + 8)
            self.sections.append((rva, raw_size, raw_offset))

    def at_rva(self, rva, size):
        for start, length, offset in self.sections:
            if start <= rva and rva + size <= start + length:
                return checked_slice(self.data, offset + rva - start, size)
        raise ValueError(f"unmapped resource RVA {rva:#x}")

    def relative(self, offset, size):
        if offset < 0 or offset + size > self.resource_size:
            raise ValueError("resource directory entry is out of bounds")
        return self.at_rva(self.resource_rva + offset, size)

    def entries(self, directory):
        named, numbered = unpack(self.relative(directory, 16), "<HH", 12)
        entries = {}
        for index in range(named + numbered):
            name, child = unpack(self.relative(directory + 16 + index * 8, 8), "<II", 0)
            if not name & 0x80000000:
                entries[name] = (child & 0x7FFFFFFF, bool(child & 0x80000000))
        return entries

    def child_directory(self, directory, identifier):
        child, is_directory = self.entries(directory)[identifier]
        if not is_directory:
            raise ValueError(f"expected resource directory for ID {identifier}")
        return child

    def payload(self, directory, language):
        leaf, is_directory = self.entries(directory)[language]
        if is_directory:
            raise ValueError("expected resource data leaf")
        rva, size = unpack(self.relative(leaf, 16), "<II", 0)
        return self.at_rva(rva, size)


def verify(executable, source):
    reserved, kind, count = unpack(source, "<HHH", 0)
    if (reserved, kind) != (0, 1) or count == 0:
        raise ValueError("invalid source ICO")
    frames = []
    for index in range(count):
        entry = checked_slice(source, 6 + index * 16, 16)
        size, offset = unpack(entry, "<II", 8)
        frames.append((entry[:12], checked_slice(source, offset, size)))

    resources = PeResources(executable.read_bytes())
    # RT_GROUP_ICON = 14, group 1 is the application's primary file icon.
    groups = resources.child_directory(0, 14)
    group = resources.child_directory(groups, 1)
    icons = resources.child_directory(0, 3)  # RT_ICON
    languages = resources.entries(group)
    if not languages:
        raise ValueError("primary icon group has no language resources")
    for language in languages:
        content = resources.payload(group, language)
        if unpack(content, "<HHH", 0) != (0, 1, count):
            raise ValueError("embedded icon group/frame count differs from A")
        for index, (metadata, pixels) in enumerate(frames):
            entry = checked_slice(content, 6 + index * 14, 14)
            if entry[:12] != metadata:
                raise ValueError(f"icon frame {index} dimensions/metadata differ from A")
            identifier, = unpack(entry, "<H", 12)
            image = resources.child_directory(icons, identifier)
            if resources.payload(image, language) != pixels:
                raise ValueError(f"icon frame {index} pixels differ from A")
    print(f"{executable}: A 折光, {count} frames, {len(languages)} language(s), all bytes match")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("executables", type=Path, nargs="+")
    parser.add_argument("--icon", type=Path,
                        default=Path(__file__).resolve().parent.parent / "assets/brand/swoop/swoop.ico")
    args = parser.parse_args()
    try:
        source = args.icon.read_bytes()
        for executable in args.executables:
            verify(executable, source)
    except (OSError, ValueError, KeyError, struct.error) as error:
        parser.exit(1, f"Windows icon verification failed: {error}\n")


if __name__ == "__main__":
    main()
