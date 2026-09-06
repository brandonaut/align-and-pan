#!/usr/bin/env python3
"""Batch-align "part predominant" / "part muted" learning-track pairs.

For every audio file in the input directory whose name carries a predominant
marker (predominant, predom, either with an optional -left) or a muted marker
(muted, mute, missing), the base name with that marker removed is used as a
pairing key. A key that has both a predominant and a muted file is run
through align_and_pan -- predominant to the left channel, muted to the
right -- with align_and_pan writing the MP3 directly (-o *.mp3).

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

# Substring markers, checked case-insensitively against the file stem.
PREDOMINANT_MARKERS = ("predom",)
MUTED_MARKERS = ("mute", "missing")

# Removed from a stem to form the pairing key. Longest variants first so a
# shorter marker never eats part of a longer one ("predominant" before
# "predom", "muted" before "mute"). "left" is the optional predominant suffix.
KEY_STRIP = (
    "predominant-left",
    "predom-left",
    "predominant",
    "predom",
    "muted",
    "mute",
    "missing",
    "left",
)


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
    """lowercase, drop role markers, non-alphanumerics to '-'."""
    name = stem.lower()
    for token in KEY_STRIP:
        name = name.replace(token, "")
    return re.sub(r"[^a-z0-9]+", "-", name).strip("-")


def output_stem(pred_stem: str) -> str:
    """The predominant file's name with its marker (and any -left) rewritten
    to a single 'Left', so the output keeps the source naming scheme:
    'Foo - Tenor-Predominant-Left' -> 'Foo - Tenor-Left'."""
    s = re.sub(r"(?i)predom(?:inant)?(?:[-_ ]*left)?", "Left", pred_stem)
    return re.sub(r"(?i)left[-_ ]+left", "Left", s)


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
    parser.add_argument(
        "-y",
        "--yes",
        action="store_true",
        help="skip the confirmation prompt and process every pair",
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
        if any(m in lc for m in PREDOMINANT_MARKERS):
            role = pred_file
        elif any(m in lc for m in MUTED_MARKERS):
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

    pairs = [
        (key, pred_file[key], muted_file[key], output_dir / f"{output_stem(pred_file[key].stem)}.mp3")
        for key in sorted(pred_file)
        if key in muted_file
    ]

    unpaired_pred = [pred_file[k].name for k in sorted(pred_file) if k not in muted_file]
    unpaired_muted = [muted_file[k].name for k in sorted(muted_file) if k not in pred_file]

    for key, p, m, mp3 in pairs:
        print(f"== {key}")
        print(f"   L (predominant): {p.name}")
        print(f"   R (muted):       {m.name}")
        print(f"   -> {mp3}")
    for name in unpaired_pred:
        print(f"unpaired predominant: {name}")
    for name in unpaired_muted:
        print(f"unpaired muted: {name}")
    for name in skipped:
        print(f"skipped (no role in name): {name}")

    if not pairs:
        print("\nno pairs to process")
        return

    print(f"\n{len(pairs)} pair(s) to mix into {output_dir}")
    if not args.yes:
        try:
            reply = input("Proceed? [y/N] ").strip().lower()
        except EOFError:
            reply = ""
        if reply not in ("y", "yes"):
            print("aborted")
            return

    for key, p, m, mp3 in pairs:
        print(f"== {key}")
        cmd = [binary, str(p), str(m), "-o", str(mp3)]
        if args.trim is not None:
            cmd += ["--trim", str(args.trim)]
        subprocess.run(cmd, check=True)
        print(f"   -> {mp3}")

    print(f"\nmixed {len(pairs)} pair(s) into {output_dir}")


if __name__ == "__main__":
    main()
