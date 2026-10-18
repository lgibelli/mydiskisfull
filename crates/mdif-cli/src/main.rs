//! `mdif`: the MyDiskIsFull command line.

mod agent_cmd;
mod map_cmd;
mod owners_cmd;
mod watch_cmd;
mod writers_cmd;

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use mdif_core::map;
use mdif_core::scan::Options;

#[derive(Parser)]
#[command(
    name = "mdif",
    version,
    about = "MyDiskIsFull: watches your disk, predicts when it will be full, and finds what fills it"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// When the disk will be full, from the watcher's history
    Forecast {
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        history: History,
    },
    /// Sample free space and disk writers into the history (what the agent runs)
    Watch {
        /// Seconds between samples
        #[arg(long, default_value_t = 300)]
        interval: u64,
        /// Take one sample and exit
        #[arg(long)]
        once: bool,
        /// Print errors only
        #[arg(long)]
        quiet: bool,
        /// Seconds between updates of the growth per folder (0: off)
        #[arg(long, default_value_t = 1800)]
        owners_every: u64,
        #[command(flatten)]
        history: History,
    },
    /// Which folders grew or shrank, and what came back after cleanups
    Owners {
        /// Window, in hours
        #[arg(long, default_value_t = 24)]
        hours: i64,
        /// How many folders to show
        #[arg(long, default_value_t = 15)]
        top: usize,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        history: History,
    },
    /// Runaway disk fillers: folders that keep growing, processes that churn
    Incidents {
        /// How far back to look, in days
        #[arg(long, default_value_t = 7)]
        days: i64,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        history: History,
    },
    /// Run the watcher in the background (a LaunchAgent)
    Agent {
        #[command(subcommand)]
        action: AgentAction,
    },
    /// Where the disk space is now, reconciled with what APFS reports
    Map {
        /// Directory to scan (default: the whole Data volume)
        #[arg(long, default_value = map::DATA_ROOT)]
        root: PathBuf,
        /// Levels of the tree to print
        #[arg(long, default_value_t = 3)]
        depth: usize,
        /// Hide subtrees smaller than this many MB
        #[arg(long, default_value_t = 500)]
        min_mb: u64,
        /// Print the full result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Which of your processes are writing to disk, and how fast
    Writers {
        /// Sampling interval in seconds
        #[arg(long, default_value_t = 10)]
        secs: u64,
        /// How many processes to show
        #[arg(long, default_value_t = 10)]
        top: usize,
    },
}

#[derive(Args)]
struct History {
    /// History database
    #[arg(long, default_value_os_t = watch_cmd::default_db())]
    db: PathBuf,
    /// Volume to watch
    #[arg(long, default_value = map::DATA_ROOT)]
    volume: PathBuf,
}

#[derive(Subcommand)]
enum AgentAction {
    /// Install the watcher as a LaunchAgent and start it
    Install {
        /// Seconds between samples
        #[arg(long, default_value_t = 300)]
        interval: u64,
    },
    /// Stop the watcher and remove the LaunchAgent (the history is kept)
    Uninstall,
    /// Whether the watcher is running, and how big its history is
    Status,
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Forecast { json, history } => watch_cmd::forecast(&history.db, &history.volume, json)?,
        Cmd::Watch {
            interval,
            once,
            quiet,
            owners_every,
            history,
        } => watch_cmd::watch(
            &history.db,
            &history.volume,
            Duration::from_secs(interval),
            owners_every,
            once,
            quiet,
        )?,
        Cmd::Owners {
            hours,
            top,
            json,
            history,
        } => owners_cmd::owners(&history.db, hours.max(1), top, json)?,
        Cmd::Incidents {
            days,
            json,
            history,
        } => owners_cmd::incidents(&history.db, days.max(1), json)?,
        Cmd::Agent { action } => match action {
            AgentAction::Install { interval } => agent_cmd::install(interval)?,
            AgentAction::Uninstall => agent_cmd::uninstall()?,
            AgentAction::Status => agent_cmd::status()?,
        },
        Cmd::Map {
            root,
            depth,
            min_mb,
            json,
        } => {
            let opts = Options {
                max_depth: depth.max(1),
                min_node: min_mb * 1_000_000,
            };
            let m = map::build(&root, &opts)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&m)?);
            } else {
                map_cmd::print_map(&m, depth);
            }
        }
        Cmd::Writers { secs, top } => writers_cmd::writers(secs, top),
    }
    Ok(())
}
