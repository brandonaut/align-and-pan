#!/usr/bin/env bash
#
# Batch-align "part predominant" / "part muted" learning-track pairs and
# emit an MP3 for each pair.
#
# For every audio file in the input directory whose name contains
# "predominant" or "muted", the base name (with that word removed) is used
# as a pairing key. A key that has both a predominant and a muted file is
# run through align_and_pan -- predominant to the left channel, muted to
# the right -- with align_and_pan writing the MP3 directly.
#
# Usage:
#   scripts/batch-pair-mix.sh [-i INPUT_DIR] [-o OUTPUT_DIR] [-b BINARY] [-t TRIM]
#
#   -i INPUT_DIR   directory to scan (non-recursive)   [default: .]
#   -o OUTPUT_DIR  where MP3s are written              [default: ./mixed]
#   -b BINARY      align_and_pan binary                [default: target/release/align_and_pan, else PATH]
#   -t TRIM        pass --trim TRIM to align_and_pan   [default: none, correlate whole files]

set -euo pipefail

input_dir="."
output_dir="./mixed"
binary=""
trim=""

while getopts ":i:o:b:t:h" opt; do
  case "$opt" in
    i) input_dir="$OPTARG" ;;
    o) output_dir="$OPTARG" ;;
    b) binary="$OPTARG" ;;
    t) trim="$OPTARG" ;;
    h) sed -n '2,20p' "$0"; exit 0 ;;
    :) echo "option -$OPTARG needs a value" >&2; exit 2 ;;
    \?) echo "unknown option -$OPTARG" >&2; exit 2 ;;
  esac
done

if [[ -z "$binary" ]]; then
  if [[ -x "target/release/align_and_pan" ]]; then
    binary="target/release/align_and_pan"
  elif command -v align_and_pan >/dev/null 2>&1; then
    binary="align_and_pan"
  else
    echo "align_and_pan not found (build it, or pass -b PATH)" >&2
    exit 1
  fi
fi

[[ -d "$input_dir" ]] || { echo "input dir not found: $input_dir" >&2; exit 1; }

mkdir -p "$output_dir"

# key -> file path, for each role
declare -A pred_file muted_file

# derive a pairing key: lowercase, drop the role word, non-alphanumerics to "-"
make_key() {
  local name="$1"
  name="${name,,}"
  name="${name//predominant/}"
  name="${name//muted/}"
  name="$(printf '%s' "$name" | sed -E 's/[^a-z0-9]+/-/g; s/^-+//; s/-+$//')"
  printf '%s' "$name"
}

shopt -s nullglob nocaseglob
files=()
for ext in mp3 flac wav m4a aac ogg opus; do
  for f in "$input_dir"/*."$ext"; do
    files+=("$f")
  done
done
shopt -u nullglob nocaseglob

skipped=()
for f in "${files[@]}"; do
  base="$(basename "$f")"
  stem="${base%.*}"
  lc="${stem,,}"
  role=""
  [[ "$lc" == *predominant* ]] && role="pred"
  [[ "$lc" == *muted* ]] && role="muted"
  if [[ -z "$role" ]]; then
    skipped+=("$base")
    continue
  fi
  key="$(make_key "$stem")"
  if [[ "$role" == "pred" ]]; then
    if [[ -n "${pred_file[$key]:-}" ]]; then
      echo "warn: multiple predominant files for '$key', keeping first: ${pred_file[$key]##*/}" >&2
    else
      pred_file[$key]="$f"
    fi
  else
    if [[ -n "${muted_file[$key]:-}" ]]; then
      echo "warn: multiple muted files for '$key', keeping first: ${muted_file[$key]##*/}" >&2
    else
      muted_file[$key]="$f"
    fi
  fi
done

mixed=0
for key in "${!pred_file[@]}"; do
  p="${pred_file[$key]}"
  m="${muted_file[$key]:-}"
  if [[ -z "$m" ]]; then
    continue
  fi
  mp3="$output_dir/$key.mp3"
  echo "== $key"
  echo "   L (predominant): $(basename "$p")"
  echo "   R (muted):       $(basename "$m")"
  align_args=("$p" "$m" -o "$mp3")
  [[ -n "$trim" ]] && align_args+=(--trim "$trim")
  "$binary" "${align_args[@]}"
  echo "   -> $mp3"
  mixed=$((mixed + 1))
done

echo
echo "mixed $mixed pair(s) into $output_dir"

unpaired_pred=()
for key in "${!pred_file[@]}"; do
  [[ -z "${muted_file[$key]:-}" ]] && unpaired_pred+=("${pred_file[$key]##*/}")
done
unpaired_muted=()
for key in "${!muted_file[@]}"; do
  [[ -z "${pred_file[$key]:-}" ]] && unpaired_muted+=("${muted_file[$key]##*/}")
done

if ((${#unpaired_pred[@]})); then
  printf 'unpaired predominant: %s\n' "${unpaired_pred[@]}"
fi
if ((${#unpaired_muted[@]})); then
  printf 'unpaired muted: %s\n' "${unpaired_muted[@]}"
fi
if ((${#skipped[@]})); then
  printf 'skipped (no role in name): %s\n' "${skipped[@]}"
fi
