# align_and_pan

Auto-aligns two audio tracks (via FFT cross-correlation) and pans them into a
single stereo WAV: left_track -> left channel, right_track -> right channel.

Pure Rust — no ffmpeg required. Decoding is via `symphonia` (mp3/flac/wav/aac/ogg),
FFT via `rustfft`, WAV output via `hound`.

## Build (native, for your own machine)

```bash
cargo build --release
# binary at target/release/align_and_pan (or .exe on Windows)
```

## Usage

```bash
./align_and_pan left_track.mp3 right_track.mp3 -o combined.wav
./align_and_pan left_track.mp3 right_track.mp3 -o combined.wav --trim 30
```

- `--trim N` — only use the first N seconds of each file to compute the offset
  (skip a messy/blown intro, or speed up analysis on long files).
- `--min-confidence N` — warn if the correlation confidence is below N
  (default 5.0; this is a rough, unitless score, not a percentage).

## Cross-compiling for distribution

Install targets once:

```bash
rustup target add x86_64-pc-windows-gnu   # Windows
rustup target add x86_64-apple-darwin     # macOS Intel
rustup target add aarch64-apple-darwin    # macOS Apple Silicon
rustup target add x86_64-unknown-linux-gnu
```

Then build for each:

```bash
cargo build --release --target x86_64-pc-windows-gnu
cargo build --release --target x86_64-unknown-linux-gnu
```

macOS targets typically need to actually be built on macOS (or via the
`cross` tool / a CI runner like GitHub Actions with a macos runner) since
cross-compiling to Apple targets from Linux is unreliable without Apple's
SDK. For Windows and Linux, building from Linux directly usually works fine
once the target and a linker (`mingw-w64` for the Windows target) are
installed.

A GitHub Actions matrix build (one job per OS runner, each just running
`cargo build --release`) is the most reliable way to produce all three
binaries without fighting cross-compilation locally.

## Known limitations

- Cross-correlation runs on whitened waveforms (GCC-PHAT), not MFCCs, so it's
  less robust to heavy noise/compression than tools like `audio-offset-finder`.
  For same-source learning tracks this should be more than sufficient.
- Only a single constant offset is found. If the two files are different
  performances that drift apart in tempo, no one shift aligns them for the
  whole track; use `--trim` to lock onto the start and accept tail drift.
- Resampling (when the two files have different sample rates) is linear
  interpolation — fine for alignment purposes, not audiophile-grade.
- This has not been compiled/tested in the environment that generated it
  (no network access to fetch crates). Run `cargo build --release` and report
  any compiler errors — Rust's messages are usually specific enough to fix
  quickly from the error text alone.
