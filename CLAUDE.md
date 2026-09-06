# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`align_and_pan` is a single-binary Rust CLI.
It takes two audio files, auto-aligns them via FFT cross-correlation, and writes one stereo file with the first file on the left channel and the second on the right.
Output is WAV or MP3, selected from the `-o` extension.
No ffmpeg dependency; MP3 output links LAME, which `mp3lame-sys` builds from bundled C source (so the build host needs a C compiler plus autotools/make).

## Commands

```bash
cargo build --release        # binary at target/release/align_and_pan
cargo run -- left.mp3 right.mp3 -o combined.wav
cargo check                  # fast type-check during iteration
cargo clippy                 # lints
cargo fmt                    # format
```

There is no test suite yet.
`cargo test` runs nothing meaningful.

The crate dependency graph has not been built in the environment that generated this code (no network to fetch crates), so the first `cargo build` may surface compiler errors to fix from the error text.

Cross-compilation notes (Windows/Linux from Linux, macOS needs a mac or CI) live in `README.md`.

## Architecture

Everything is in `src/main.rs`, structured as a linear pipeline in `main()`:

1. `decode_audio_file` — symphonia probes the container, picks the first audio track, decodes every packet, and downmixes to mono `f32` via `append_as_mono`.
   Recoverable `DecodeError` packets are skipped; `UnexpectedEof` ends the loop.
2. `resample_linear` — if the two files differ in sample rate, both are linearly resampled up to the higher rate.
   Alignment math below assumes both streams share one `target_rate`.
3. `find_offset` — zero-pads both signals to the next power of two, forward FFT both, form the cross-power spectrum (one times the conjugate of the other), then whiten it by dividing each bin by its magnitude to the power `beta` (GCC-PHAT-beta, `beta = 0.75`), inverse FFT, take the real part.
   Whitening is what makes the peak survive the level/EQ difference between two mixes; plain cross-correlation locked onto a spurious peak on the sample choral tracks.
   The peak index maps to a signed lag (indices past `n/2` wrap to negative), then to `offset_seconds`.
   Confidence is peak value over the correlation RMS — a rough unitless score, not a percentage; `--trim` limits how much audio feeds this step.
   Only one constant offset is produced: two recordings that drift apart in tempo cannot be aligned across the whole track by a single shift.
4. Delay padding — the later track is prefixed with `offset_seconds * rate` zero samples so both start together.
   Sign convention: positive `offset_seconds` delays the right track, negative delays the left.
5. Output — `output_format` maps the `-o` extension to `Wav` or `Mp3` (any other extension is a hard error, checked right after arg parsing so it fails before the long decode).
   `write_stereo_wav` interleaves left/right, converts `f32` to `i16` with clamping, writes 16-bit PCM via hound.
   `write_stereo_mp3` does the same `f32`-to-`i16` interleave, then encodes one shot via `mp3lame-encoder` at 192 kbps CBR, quality Best; `set_sample_rate` rejects non-MPEG rates so a >48 kHz `target_rate` errors here.
   Either way the shorter channel is zero-padded to match the longer.

The offset sign convention in `find_offset`'s doc comment is load-bearing; changing which argument is `a` vs `b` flips it.
