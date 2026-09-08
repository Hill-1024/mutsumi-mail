#!/usr/bin/env python3
"""Check Mutsumi Mail's name-resolved JNI downcalls in final APK/AAB artifacts.

Compilation cannot detect a Kotlin external method whose Rust implementation
was not linked. Check the actual DEX after R8 against each packaged ABI. This
app uses exported Java_* symbols, not RegisterNatives-based registration.
"""

import argparse
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import zipfile


def jni_escape(value):
    # JNI names encode UTF-16 code units, including Kotlin's '$' companion names.
    result = []
    encoded = value.encode("utf-16-be")
    for index in range(0, len(encoded), 2):
        unit = int.from_bytes(encoded[index:index + 2], "big")
        char = chr(unit)
        if char.isascii() and char.isalnum():
            result.append(char)
        else:
            result.append({"/": "_", "_": "_1", ";": "_2", "[": "_3"}.get(
                char, f"_0{unit:04x}"
            ))
    return "".join(result)


def native_methods(dump):
    owner = name = signature = None
    for line in dump.splitlines():
        match = re.search(r"Class descriptor\s*:\s*'L([^;]+);'", line)
        if match:
            owner = match[1]
        match = re.search(r"name\s*:\s*'([^']+)'", line)
        if match:
            name = match[1]
        match = re.search(r"type\s*:\s*'([^']+)'", line)
        if match:
            signature = match[1]
        if re.search(r"access\s*:.*\bNATIVE\b", line):
            if not owner or not name or not signature or not signature.startswith("("):
                raise ValueError("Cannot parse native method from dexdump")
            yield owner, name, signature


def verify(package, dexdump, nm):
    methods = set()
    symbols_by_abi = {}
    with tempfile.TemporaryDirectory(prefix="mutsumi-jni-") as temporary:
        directory = Path(temporary)
        with zipfile.ZipFile(package) as archive:
            for entry in archive.namelist():
                if re.fullmatch(r"(?:base/dex/)?classes\d*\.dex", entry):
                    dex = directory / Path(entry).name
                    dex.write_bytes(archive.read(entry))
                    # String constants in dexdump can contain modified UTF-8 NULs.
                    dump = subprocess.check_output(
                        [dexdump, str(dex)], text=True, errors="replace"
                    )
                    methods.update(native_methods(dump))
                match = re.fullmatch(r"(?:base/)?lib/([^/]+)/([^/]+\.so)", entry)
                if match:
                    abi, library = match.groups()
                    target = directory / abi / library
                    target.parent.mkdir(exist_ok=True)
                    target.write_bytes(archive.read(entry))
                    exported = subprocess.check_output(
                        [nm, "--dynamic", "--defined-only", "--format=posix", str(target)],
                        text=True,
                    )
                    symbols_by_abi.setdefault(abi, set()).update(
                        line.split()[0] for line in exported.splitlines() if line.strip()
                    )

    if not methods or not symbols_by_abi:
        raise ValueError(f"{package}: expected DEX native methods and packaged native libraries")

    missing = []
    for abi, symbols in sorted(symbols_by_abi.items()):
        for owner, name, signature in sorted(methods):
            short = f"Java_{jni_escape(owner)}_{jni_escape(name)}"
            arguments = signature[1:signature.index(")")]
            long = f"{short}__{jni_escape(arguments)}"
            if short not in symbols and long not in symbols:
                missing.append(f"  {abi}: {owner}.{name}{signature}\n    missing {short}")
    if missing:
        raise ValueError(f"{package}: unresolved JNI methods:\n" + "\n".join(missing))
    print(f"{package}: verified {len(methods)} JNI methods across "
          f"{', '.join(sorted(symbols_by_abi))}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dexdump", required=True, help="Android SDK build-tools dexdump")
    parser.add_argument("--nm", required=True, help="Android NDK llvm-nm")
    parser.add_argument("packages", nargs="+", type=Path, help="Final APK or AAB files")
    args = parser.parse_args()
    try:
        for package in args.packages:
            verify(package, args.dexdump, args.nm)
    except (OSError, ValueError, zipfile.BadZipFile, subprocess.CalledProcessError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
