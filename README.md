# align_and_pan

Auto-aligns two audio tracks (via FFT cross-correlation) and pans them into a
single stereo file: left_track -> left channel, right_track -> right channel.

Output is WAV or MP3, chosen from the `-o` extension.

No ffmpeg required.
Decoding is via `symphonia` (mp3/flac/wav/aac/ogg), FFT via `rustfft`, WAV output via `hound`, MP3 output via `mp3lame-encoder` (LAME).

## Build (native, for your own machine)

```bash
cargo build --release
# binary at target/release/align_and_pan (or .exe on Windows)
```

MP3 output links LAME, which `mp3lame-sys` builds from bundled C source with autotools.
The build host therefore needs a C compiler plus `autoconf`, `automake`, `libtool`, and `make`.

- Debian/Ubuntu: `sudo apt install build-essential autoconf automake libtool`
- Fedora: `sudo dnf install gcc autoconf automake libtool make`
- Windows (native): install the MSYS2 toolchain (`pacman -S base-devel mingw-w64-x86_64-toolchain`), or build under WSL.

## Usage

```bash
./align_and_pan left_track.mp3 right_track.mp3 -o combined.wav
./align_and_pan left_track.mp3 right_track.mp3 -o combined.mp3
./align_and_pan left_track.mp3 right_track.mp3 -o combined.wav --trim 30
```

- `-o FILE` — output path; `.wav` writes 16-bit PCM, `.mp3` writes a 192 kbps MP3.
  Any other extension is an error.
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

Because MP3 output builds LAME from C source, each cross target also needs a
matching C cross-compiler, not just the Rust target:

- `x86_64-pc-windows-gnu`: `sudo apt install gcc-mingw-w64-x86-64` (plus
  `autoconf automake libtool` on the build host).
- `x86_64-unknown-linux-gnu` from an x86-64 Linux host: the native `gcc` is
  enough; other-arch Linux targets need that arch's `gcc` cross package.

The `cross` tool bundles these toolchains in its Docker images and is the
least painful route if the host toolchain fights you.

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
- MP3 output is fixed at 192 kbps and only supports the standard MPEG sample
  rates (8/11.025/12/16/22.05/24/32/44.1/48 kHz). If both inputs are at a
  higher rate (e.g. 96 kHz) the encoder errors out; write WAV instead, or
  downsample the inputs first.
- This has not been compiled/tested in the environment that generated it
  (no network access to fetch crates). Run `cargo build --release` and report
  any compiler errors — Rust's messages are usually specific enough to fix
  quickly from the error text alone.
