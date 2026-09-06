#!/usr/bin/env python3
"""Batch-align "part predominant" / "part muted" learning-track pairs.

For every audio file in the input directory whose name contains "predominant"
or "muted", the base name (with that word removed) is used as a pairing key.
A key that has both a predominant and a muted file is run through
align_and_pan -- predominant to the left channel, muted to the right -- with
align_and_pan writing the MP3 directly (-o *.mp3).

Stdlib only; no third-party packages and no shell, so it runs the same on
Windows and Linux.
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
from pathlib import Path

AUDIO_EXTS = {"mp3", "flac", "wav", "m4a", "aac", "ogg", "opus"}


def find_binary(explicit: str | None) -> str:
    if explicit:
        return explicit
    for name in ("align_and_pan", "align_and_pan.exe"):
        local = Path("target/release") / name
        if local.is_file():
            return str(local)
    found = shutil.which("align_and_pan")
    if found:
        return found
    sys.exit("align_and_pan not found (build it, or pass -b PATH)")


def make_key(stem: str) -> str:
    """lowercase, drop the role word, non-alphanumerics to '-'."""
    name = stem.lower().replace("predominant", "").replace("muted", "")
    return re.sub(r"[^a-z0-9]+", "-", name).strip("-")


def main() -> None:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "-i",
        "--input-dir",
        default=".",
        help="directory to scan (non-recursive) [default: %(default)s]",
    )
    parser.add_argument(
        "-o",
        "--output-dir",
        default="./mixed",
        help="where MP3s are written [default: %(default)s]",
    )
    parser.add_argument(
        "-b",
        "--binary",
        default=None,
        help="align_and_pan binary [default: target/release/align_and_pan, else PATH]",
    )
    parser.add_argument(
        "-t",
        "--trim",
        default=None,
        help="pass --trim TRIM to align_and_pan [default: none, correlate whole files]",
    )
    args = parser.parse_args()

    binary = find_binary(args.binary)

    input_dir = Path(args.input_dir)
    if not input_dir.is_dir():
        sys.exit(f"input dir not found: {input_dir}")

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    pred_file: dict[str, Path] = {}
    muted_file: dict[str, Path] = {}
    skipped: list[str] = []

    for entry in sorted(input_dir.iterdir()):
        if not entry.is_file() or entry.suffix.lower().lstrip(".") not in AUDIO_EXTS:
            continue
        lc = entry.stem.lower()
        if "predominant" in lc:
            role = pred_file
        elif "muted" in lc:
            role = muted_file
        else:
            skipped.append(entry.name)
            continue
        key = make_key(entry.stem)
        if key in role:
            kind = "predominant" if role is pred_file else "muted"
            print(
                f"warn: multiple {kind} files for '{key}', keeping first: {role[key].name}",
                file=sys.stderr,
            )
        else:
            role[key] = entry

    mixed = 0
    for key in sorted(pred_file):
        p = pred_file[key]
        m = muted_file.get(key)
        if m is None:
            continue
        mp3 = output_dir / f"{key}.mp3"
        print(f"== {key}")
        print(f"   L (predominant): {p.name}")
        print(f"   R (muted):       {m.name}")
        cmd = [binary, str(p), str(m), "-o", str(mp3)]
        if args.trim is not None:
            cmd += ["--trim", str(args.trim)]
        subprocess.run(cmd, check=True)
        print(f"   -> {mp3}")
        mixed += 1

    print()
    print(f"mixed {mixed} pair(s) into {output_dir}")

    unpaired_pred = [pred_file[k].name for k in sorted(pred_file) if k not in muted_file]
    unpaired_muted = [muted_file[k].name for k in sorted(muted_file) if k not in pred_file]

    for name in unpaired_pred:
        print(f"unpaired predominant: {name}")
    for name in unpaired_muted:
        print(f"unpaired muted: {name}")
    for name in skipped:
        print(f"skipped (no role in name): {name}")


if __name__ == "__main__":
    main()
