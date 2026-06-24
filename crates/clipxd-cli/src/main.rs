//! `clipxd` — the command line.
//!
//! `clipxd import <url|file>` turns a video into an agent-queryable clip index;
//! `clipxd query <clip> "<question>"` answers from that index **without the video**.
//! That round trip is the headline demo (docs/overview.md §6).

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use clipxd_index::{query, Index};
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(name = "clipxd", about = "Record once. Humans watch it. Agents read it.")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Import a video (URL or file) into an agent-queryable clip index.
    Import {
        /// A video URL (yt-dlp) or a local file path.
        input: String,
        /// Directory to write the clip into.
        #[arg(long, default_value = "clips")]
        out: PathBuf,
        /// Frames per second to sample from the source.
        #[arg(long, default_value_t = 4.0)]
        fps: f32,
        /// Override the codec salience floor (lower = denser emission; "degrade mode").
        #[arg(long)]
        salience_min: Option<f32>,
    },
    /// Ask a question about a clip; answered from the index, no video needed.
    Query {
        /// A clip directory or an index.json path.
        clip: PathBuf,
        /// The natural-language question.
        question: String,
    },
    /// Full-text search across a clip's transcript + on-screen text + captions.
    Search {
        clip: PathBuf,
        query: String,
    },
    /// Show a clip's metadata and stream sizes.
    Info { clip: PathBuf },
}

fn load_index(clip: &Path) -> Result<Index> {
    let path = if clip.is_dir() {
        clip.join("index.json")
    } else {
        clip.to_path_buf()
    };
    let txt = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&txt).with_context(|| format!("parsing {}", path.display()))
}

fn main() -> Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("clipxd=info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .without_time()
        .init();

    match Cli::parse().cmd {
        Cmd::Import {
            input,
            out,
            fps,
            salience_min,
        } => {
            let opts = clipxd_import::ImportOptions {
                sample_fps: fps,
                salience_min,
            };
            let r = clipxd_import::import(&input, &out, &opts)?;
            let (tb, ob, cb) = &r.backends;
            println!("✓ imported → {}", r.clip_dir.display());
            println!("  backends: transcriber={tb}  ocr={ob}  captioner={cb}");
            println!(
                "  streams:  transcript={}  on_screen_text={}  visual_timeline={}  events={}",
                r.index.transcript.len(),
                r.index.on_screen_text.len(),
                r.index.visual_timeline.len(),
                r.index.event_track.len()
            );
            println!("  index:    {}", r.clip_dir.join("index.json").display());
        }
        Cmd::Query { clip, question } => {
            let idx = load_index(&clip)?;
            let ans = query::query_clip(&idx, &question);
            println!("Q: {question}");
            println!("A: {}", ans.text);
            if !ans.citations.is_empty() {
                let cites: Vec<String> = ans.citations.iter().map(|t| format!("{t:.1}s")).collect();
                println!("   cited: {}", cites.join(", "));
            }
        }
        Cmd::Search { clip, query: q } => {
            let idx = load_index(&clip)?;
            let hits = query::search_text(&idx, &q);
            if hits.is_empty() {
                println!("no matches for: {q}");
            }
            for h in hits.iter().take(10) {
                println!("[{:>6.1}s] ({}) {}", h.t, h.stream, h.text);
            }
        }
        Cmd::Info { clip } => {
            let idx = load_index(&clip)?;
            println!(
                "{}  \"{}\"  {}x{}  {:.1}s",
                idx.id,
                idx.metadata.title,
                idx.metadata.resolution[0],
                idx.metadata.resolution[1],
                idx.metadata.duration
            );
            println!("source={:?}  status={:?}", idx.source, idx.status);
            println!(
                "transcript={}  on_screen_text={}  visual_timeline={}  events={}",
                idx.transcript.len(),
                idx.on_screen_text.len(),
                idx.visual_timeline.len(),
                idx.event_track.len()
            );
            println!("summary: {}", idx.summary.tldr);
        }
    }
    Ok(())
}
