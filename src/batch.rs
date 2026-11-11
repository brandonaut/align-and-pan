//! Batch mode: find "predominant" / "muted" track pairs in a directory and
//! mix each pair with `align_pair` (predominant -> left, muted -> right).

use std::collections::BTreeMap;
use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use anyhow::{Context, Result};
use regex::Regex;

use crate::{align_pair, AlignOptions, OutputFormat};

const AUDIO_EXTS: &[&str] = &["mp3", "flac", "wav", "m4a", "aac", "ogg", "opus"];
const DEFAULT_PREDOMINANT: &[&str] = &["predom"];
const DEFAULT_MUTED: &[&str] = &["mute", "missing"];
/// Always removed from a name when forming the pairing key ("left" is the
/// optional predominant suffix).
const EXTRA_STRIP: &[&str] = &["predominant-left", "predom-left", "predominant", "muted", "left"];

pub struct BatchConfig {
    pub input_dir: PathBuf,
    pub output_dir: PathBuf,
    pub format: OutputFormat,
    pub recursive: bool,
    pub yes: bool,
    pub dry_run: bool,
    pub jobs: usize,
    pub overwrite: bool,
    pub predominant_markers: Vec<String>,
    pub muted_markers: Vec<String>,
    pub opts: AlignOptions,
}

struct Matcher {
    pred: Vec<String>,
    muted: Vec<String>,
    strip: Vec<String>,
    rename: Regex,
    dup_left: Regex,
}

impl Matcher {
    fn new(pred: &[String], muted: &[String]) -> Result<Self> {
        let lower = |v: &[String], d: &[&str]| -> Vec<String> {
            if v.is_empty() {
                d.iter().map(|s| s.to_string()).collect()
            } else {
                v.iter().map(|s| s.to_lowercase()).collect()
            }
        };
        let pred = lower(pred, DEFAULT_PREDOMINANT);
        let muted = lower(muted, DEFAULT_MUTED);

        let mut strip: Vec<String> = pred
            .iter()
            .chain(muted.iter())
            .cloned()
            .chain(EXTRA_STRIP.iter().map(|s| s.to_string()))
            .collect();
        // Longest first so a short marker never eats part of a longer one.
        strip.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        strip.dedup();

        let mut alts: Vec<String> = pred.clone();
        alts.push("predominant".to_string());
        alts.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
        alts.dedup();
        let alts: Vec<String> = alts.iter().map(|a| regex::escape(a)).collect();
        let rename = Regex::new(&format!("(?i)(?:{})(?:[-_ ]*left)?", alts.join("|")))?;
        let dup_left = Regex::new("(?i)left[-_ ]+left")?;

        Ok(Self {
            pred,
            muted,
            strip,
            rename,
            dup_left,
        })
    }

    /// Lowercase, drop role markers, non-alphanumerics to '-'.
    fn key(&self, stem: &str) -> String {
        let mut name = stem.to_lowercase();
        for token in &self.strip {
            name = name.replace(token.as_str(), "");
        }
        let mut out = String::new();
        let mut pending_dash = false;
        for c in name.chars() {
            if c.is_ascii_alphanumeric() {
                if pending_dash && !out.is_empty() {
                    out.push('-');
                }
                pending_dash = false;
                out.push(c);
            } else {
                pending_dash = true;
            }
        }
        out
    }

    /// The predominant file's name with its marker (and any -left) rewritten
    /// to a single 'Left': 'Foo - Tenor-Predominant-Left' -> 'Foo - Tenor-Left'.
    fn output_stem(&self, pred_stem: &str) -> String {
        let s = self.rename.replace_all(pred_stem, "Left");
        self.dup_left.replace_all(&s, "Left").into_owned()
    }
}

enum Role {
    Pred,
    Muted,
}

struct Pair {
    key: String,
    left: PathBuf,
    right: PathBuf,
    out: PathBuf,
}

#[derive(Default)]
struct Scan {
    pairs: Vec<Pair>,
    unpaired: Vec<String>,
    skipped: Vec<String>,
    warnings: Vec<String>,
}

fn collect_dirs(root: &Path, recursive: bool, exclude: &Path) -> Result<Vec<PathBuf>> {
    let mut dirs = vec![root.to_path_buf()];
    if recursive {
        let mut i = 0;
        while i < dirs.len() {
            let mut subs = Vec::new();
            for entry in fs::read_dir(&dirs[i]).with_context(|| format!("reading {:?}", dirs[i]))? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    let path = entry.path();
                    // Don't descend into our own output directory.
                    if std::path::absolute(&path).map(|p| p == exclude).unwrap_or(false) {
                        continue;
                    }
                    subs.push(path);
                }
            }
            subs.sort();
            dirs.extend(subs);
            i += 1;
        }
        dirs[1..].sort();
    }
    Ok(dirs)
}

fn scan_dir(dir: &Path, cfg: &BatchConfig, m: &Matcher, scan: &mut Scan) -> Result<()> {
    let rel = dir.strip_prefix(&cfg.input_dir).unwrap_or(Path::new(""));
    let shown = |name: &str| rel.join(name).display().to_string();

    let mut entries: Vec<_> = fs::read_dir(dir)
        .with_context(|| format!("reading {:?}", dir))?
        .collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());

    let mut pred: BTreeMap<String, PathBuf> = BTreeMap::new();
    let mut muted: BTreeMap<String, PathBuf> = BTreeMap::new();

    for entry in entries {
        let path = entry.path();
        let is_audio = path.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
                .unwrap_or(false);
        if !is_audio {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let lc = stem.to_lowercase();
        let role = if m.pred.iter().any(|x| lc.contains(x.as_str())) {
            Role::Pred
        } else if m.muted.iter().any(|x| lc.contains(x.as_str())) {
            Role::Muted
        } else {
            scan.skipped.push(shown(&name));
            continue;
        };
        let (map, kind) = match role {
            Role::Pred => (&mut pred, "predominant"),
            Role::Muted => (&mut muted, "muted"),
        };
        let key = m.key(&stem);
        if let Some(first) = map.get(&key) {
            scan.warnings.push(format!(
                "multiple {kind} files for '{key}', keeping first: {}",
                shown(&first.file_name().unwrap_or_default().to_string_lossy())
            ));
        } else {
            map.insert(key, path);
        }
    }

    let out_dir = cfg.output_dir.join(rel);
    for (key, left) in &pred {
        match muted.get(key) {
            Some(right) => {
                let stem = m.output_stem(&left.file_stem().unwrap_or_default().to_string_lossy());
                scan.pairs.push(Pair {
                    key: if rel.as_os_str().is_empty() {
                        key.clone()
                    } else {
                        format!("{}/{}", rel.display(), key)
                    },
                    left: left.clone(),
                    right: right.clone(),
                    out: out_dir.join(format!("{stem}.{}", cfg.format.extension())),
                });
            }
            None => scan.unpaired.push(format!(
                "unpaired predominant: {}",
                shown(&left.file_name().unwrap_or_default().to_string_lossy())
            )),
        }
    }
    for (key, right) in &muted {
        if !pred.contains_key(key) {
            scan.unpaired.push(format!(
                "unpaired muted: {}",
                shown(&right.file_name().unwrap_or_default().to_string_lossy())
            ));
        }
    }
    Ok(())
}

fn run_pair(p: &Pair, cfg: &BatchConfig) -> Result<crate::Outcome> {
    if let Some(parent) = p.out.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {:?}", parent))?;
    }
    let res = align_pair(&p.left, &p.right, &p.out, cfg.format, &cfg.opts, false);
    if res.is_err() {
        // Don't leave a truncated file that --skip-existing would trust later.
        let _ = fs::remove_file(&p.out);
    }
    res
}

/// Returns Ok(true) if every pair succeeded (or there was nothing to do).
pub fn run(cfg: &BatchConfig) -> Result<bool> {
    let matcher = Matcher::new(&cfg.predominant_markers, &cfg.muted_markers)?;
    let out_abs = std::path::absolute(&cfg.output_dir)?;

    let mut scan = Scan::default();
    for dir in collect_dirs(&cfg.input_dir, cfg.recursive, &out_abs)? {
        scan_dir(&dir, cfg, &matcher, &mut scan)?;
    }

    for w in &scan.warnings {
        eprintln!("warn: {w}");
    }
    let mut todo: Vec<&Pair> = Vec::new();
    for p in &scan.pairs {
        println!("== {}", p.key);
        println!("   L (predominant): {}", p.left.display());
        println!("   R (muted):       {}", p.right.display());
        if !cfg.overwrite && p.out.exists() {
            println!("   -> {} (exists, skipping)", p.out.display());
        } else {
            println!("   -> {}", p.out.display());
            todo.push(p);
        }
    }
    for u in &scan.unpaired {
        println!("{u}");
    }
    for s in &scan.skipped {
        println!("skipped (no role in name): {s}");
    }

    if todo.is_empty() {
        println!("\nno pairs to process");
        return Ok(true);
    }
    println!(
        "\n{} pair(s) to mix into {}",
        todo.len(),
        cfg.output_dir.display()
    );
    if cfg.dry_run {
        return Ok(true);
    }
    if !cfg.yes && std::io::stdin().is_terminal() {
        print!("Proceed? [y/N] ");
        std::io::stdout().flush()?;
        let mut reply = String::new();
        std::io::stdin().read_line(&mut reply)?;
        if !matches!(reply.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("aborted");
            return Ok(true);
        }
    }

    let total = todo.len();
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let failures: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

    std::thread::scope(|s| {
        for _ in 0..cfg.jobs.min(total) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= total {
                    break;
                }
                let p = todo[i];
                let res = run_pair(p, cfg);
                let n = done.fetch_add(1, Ordering::SeqCst) + 1;
                match res {
                    Ok(o) => {
                        let warn = if o.confidence < cfg.opts.min_confidence {
                            "  [LOW CONFIDENCE]"
                        } else {
                            ""
                        };
                        println!(
                            "[{n}/{total}] ok   {} (offset {:.3}s, confidence {:.2}){warn}",
                            p.key, o.offset_seconds, o.confidence
                        );
                    }
                    Err(e) => {
                        let msg = format!("{e:#}");
                        println!("[{n}/{total}] FAIL {}: {msg}", p.key);
                        failures.lock().unwrap().push((p.key.clone(), msg));
                    }
                }
            });
        }
    });

    let failures = failures.into_inner().unwrap();
    println!(
        "\nmixed {} of {} pair(s) into {}",
        total - failures.len(),
        total,
        cfg.output_dir.display()
    );
    if !failures.is_empty() {
        eprintln!("{} pair(s) failed:", failures.len());
        for (key, msg) in &failures {
            eprintln!("  {key}: {msg}");
        }
    }
    Ok(failures.is_empty())
}
