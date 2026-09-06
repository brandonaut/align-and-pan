//! align_and_pan
//!
//! Auto-aligns two learning tracks using FFT-based cross-correlation, then
//! pans them into a single stereo WAV file: left_track -> left channel,
//! right_track -> right channel.
//!
//! Pure Rust: symphonia decodes audio (mp3/flac/wav/aac/ogg), no ffmpeg
//! required. rustfft does the cross-correlation. hound writes the output WAV.
//!
//! NOTE: this correlates whitened waveforms (GCC-PHAT, not MFCCs), so it's
//! less robust to heavy noise/compression artifacts than tools like
//! audio-offset-finder -- but for same-source learning tracks that should be
//! plenty. It also assumes a single constant offset: two different
//! performances that drift apart in tempo cannot be aligned by one shift.

use std::fs::File;
use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use clap::Parser;
use hound::{SampleFormat, WavSpec, WavWriter};
use rustfft::{num_complex::Complex, FftPlanner};
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

#[derive(Parser)]
#[command(about = "Auto-align two audio tracks and pan them into a stereo L/R mix")]
struct Args {
    /// Audio file to place in the left channel
    left_track: PathBuf,

    /// Audio file to place in the right channel
    right_track: PathBuf,

    /// Output WAV file path
    #[arg(short, long, default_value = "combined.wav")]
    output: PathBuf,

    /// Only analyze the first N seconds of each file when finding the offset
    /// (speeds up analysis on long files, and limits the effect of two
    /// recordings drifting apart in tempo over their length)
    #[arg(long)]
    trim: Option<f64>,

    /// Warn if confidence is below this (rough, unitless score)
    #[arg(long, default_value_t = 20.0)]
    min_confidence: f32,
}

struct Decoded {
    samples: Vec<f32>, // mono
    sample_rate: u32,
}

fn decode_audio_file(path: &PathBuf) -> Result<Decoded> {
    let file = File::open(path).with_context(|| format!("opening {:?}", path))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let meta_opts: MetadataOptions = Default::default();
    let fmt_opts: FormatOptions = Default::default();

    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &fmt_opts, &meta_opts)
        .with_context(|| format!("probing format for {:?}", path))?;

    let mut format = probed.format;

    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or_else(|| anyhow!("no supported audio track found in {:?}", path))?
        .clone();

    let track_id = track.id;
    let sample_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| anyhow!("unknown sample rate in {:?}", path))?;

    let dec_opts: DecoderOptions = Default::default();
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &dec_opts)
        .with_context(|| format!("creating decoder for {:?}", path))?;

    let mut mono_samples: Vec<f32> = Vec::new();

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(SymphoniaError::IoError(ref e))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break;
            }
            Err(e) => return Err(e.into()),
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded_buf) => {
                append_as_mono(&decoded_buf, &mut mono_samples);
            }
            // Recoverable decode errors: skip this packet and keep going.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        }
    }

    Ok(Decoded {
        samples: mono_samples,
        sample_rate,
    })
}

/// Downmix a decoded audio buffer (any sample format) to mono f32 and append.
fn append_as_mono(buf: &AudioBufferRef, out: &mut Vec<f32>) {
    macro_rules! mixdown {
        ($buf:expr) => {{
            let spec = $buf.spec();
            let channels = spec.channels.count().max(1);
            let frames = $buf.frames();
            for i in 0..frames {
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    sum += IntoSample::<f32>::into_sample($buf.chan(ch)[i]);
                }
                out.push(sum / channels as f32);
            }
        }};
    }

    use symphonia::core::conv::IntoSample;

    match buf {
        AudioBufferRef::U8(b) => mixdown!(b),
        AudioBufferRef::U16(b) => mixdown!(b),
        AudioBufferRef::U24(b) => mixdown!(b),
        AudioBufferRef::U32(b) => mixdown!(b),
        AudioBufferRef::S8(b) => mixdown!(b),
        AudioBufferRef::S16(b) => mixdown!(b),
        AudioBufferRef::S24(b) => mixdown!(b),
        AudioBufferRef::S32(b) => mixdown!(b),
        AudioBufferRef::F32(b) => mixdown!(b),
        AudioBufferRef::F64(b) => mixdown!(b),
    }
}

/// Simple linear resampler. Fine for this use case; not audiophile-grade.
fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = to_rate as f64 / from_rate as f64;
    let new_len = ((samples.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(new_len);
    for i in 0..new_len {
        let src_pos = i as f64 / ratio;
        let idx = src_pos.floor() as usize;
        let frac = (src_pos - idx as f64) as f32;
        let s0 = *samples.get(idx).unwrap_or(&0.0);
        let s1 = *samples.get(idx + 1).unwrap_or(&s0);
        out.push(s0 + (s1 - s0) * frac);
    }
    out
}

/// FFT-based cross-correlation. Returns (offset_seconds, confidence).
///
/// offset_seconds > 0 means `b` starts later than `a` (delay `b` by that
/// amount to align them). offset_seconds < 0 means the reverse (delay `a`).
fn find_offset(a: &[f32], b: &[f32], sample_rate: u32) -> (f64, f32) {
    let n = (a.len() + b.len()).next_power_of_two();

    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let ifft = planner.plan_fft_inverse(n);

    let mut fa: Vec<Complex<f32>> = a.iter().map(|&x| Complex::new(x, 0.0)).collect();
    fa.resize(n, Complex::new(0.0, 0.0));
    let mut fb: Vec<Complex<f32>> = b.iter().map(|&x| Complex::new(x, 0.0)).collect();
    fb.resize(n, Complex::new(0.0, 0.0));

    fft.process(&mut fa);
    fft.process(&mut fb);

    // Cross-power spectrum with a partial phase transform (GCC-PHAT-beta):
    // divide each bin by its magnitude raised to `beta`. beta = 1 is pure
    // PHAT, which whitens the spectrum so the correlation peak stays sharp and
    // ignores the level/EQ differences between two mixes of the same take;
    // beta = 0 is plain cross-correlation, which locks onto spurious peaks on
    // repetitive, level-mismatched material like choral learning tracks but is
    // steadier on short or low-SNR windows. 0.75 keeps most of PHAT's peak
    // sharpness without fully amplifying near-silent bins.
    let beta = 0.75f32;
    let mut cross: Vec<Complex<f32>> = fa
        .iter()
        .zip(fb.iter())
        .map(|(x, y)| {
            let c = x * y.conj();
            let mag = c.norm();
            if mag > 1e-6 {
                c / mag.powf(beta)
            } else {
                Complex::new(0.0, 0.0)
            }
        })
        .collect();

    ifft.process(&mut cross);

    let norm = 1.0 / n as f32;
    let mag: Vec<f32> = cross.iter().map(|c| c.re * norm).collect();

    let mut best_idx = 0usize;
    let mut best_val = f32::MIN;
    for (i, &v) in mag.iter().enumerate() {
        if v > best_val {
            best_val = v;
            best_idx = i;
        }
    }

    // Convert circular index to a signed lag.
    let lag: i64 = if best_idx as i64 <= n as i64 / 2 {
        best_idx as i64
    } else {
        best_idx as i64 - n as i64
    };

    let offset_seconds = lag as f64 / sample_rate as f64;

    // Rough confidence: how far the peak stands above the correlation's RMS.
    let rms = (mag.iter().map(|v| v * v).sum::<f32>() / mag.len() as f32).sqrt();
    let confidence = if rms > 0.0 { best_val / rms } else { 0.0 };

    (offset_seconds, confidence)
}

fn f32_to_i16(x: f32) -> i16 {
    (x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16
}

fn write_stereo_wav(path: &PathBuf, left: &[f32], right: &[f32], sample_rate: u32) -> Result<()> {
    let spec = WavSpec {
        channels: 2,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec)
        .with_context(|| format!("creating output file {:?}", path))?;

    let len = left.len().max(right.len());
    for i in 0..len {
        let l = *left.get(i).unwrap_or(&0.0);
        let r = *right.get(i).unwrap_or(&0.0);
        writer.write_sample(f32_to_i16(l))?;
        writer.write_sample(f32_to_i16(r))?;
    }
    writer.finalize()?;
    Ok(())
}

fn main() -> Result<()> {
    let args = Args::parse();

    println!("Decoding {:?}...", args.left_track);
    let left = decode_audio_file(&args.left_track)?;
    println!("Decoding {:?}...", args.right_track);
    let right = decode_audio_file(&args.right_track)?;

    let target_rate = left.sample_rate.max(right.sample_rate);
    let mut left_samples = resample_linear(&left.samples, left.sample_rate, target_rate);
    let mut right_samples = resample_linear(&right.samples, right.sample_rate, target_rate);

    println!("Finding offset...");
    let (offset_seconds, confidence) = {
        let (corr_left, corr_right): (&[f32], &[f32]) = match args.trim {
            Some(trim_secs) => {
                let n = (trim_secs * target_rate as f64) as usize;
                (
                    &left_samples[..left_samples.len().min(n)],
                    &right_samples[..right_samples.len().min(n)],
                )
            }
            None => (&left_samples[..], &right_samples[..]),
        };
        find_offset(corr_left, corr_right, target_rate)
    };

    println!("Detected offset: {:.3}s (right relative to left)", offset_seconds);
    println!("Confidence: {:.2}", confidence);
    if confidence < args.min_confidence {
        eprintln!(
            "Warning: confidence is below {:.2} -- the detected offset may be unreliable. \
             Consider checking alignment manually, or try --trim to skip a messy intro.",
            args.min_confidence
        );
    }

    let delay_right_samples = (offset_seconds.max(0.0) * target_rate as f64).round() as usize;
    let delay_left_samples = ((-offset_seconds).max(0.0) * target_rate as f64).round() as usize;

    if delay_left_samples > 0 {
        let mut padded = vec![0.0f32; delay_left_samples];
        padded.extend_from_slice(&left_samples);
        left_samples = padded;
    }
    if delay_right_samples > 0 {
        let mut padded = vec![0.0f32; delay_right_samples];
        padded.extend_from_slice(&right_samples);
        right_samples = padded;
    }

    println!("Writing {:?}...", args.output);
    write_stereo_wav(&args.output, &left_samples, &right_samples, target_rate)?;
    println!("Done.");

    Ok(())
}
