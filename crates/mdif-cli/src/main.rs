//! `mdif`: the MyDiskIsFull command line.

mod agent_cmd;
mod alerts_cmd;
mod clean_cmd;
mod keep_cmd;
mod map_cmd;
mod owners_cmd;
mod space_cmd;
mod status_cmd;
mod tidy_cmd;
mod tweaks_cmd;
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
    /// Free space, the forecast and what needs attention, at a glance
    Status {
        /// Print the result as JSON (what the menu bar app reads)
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        history: History,
    },
    /// When the disk will be full, from the watcher's history
    Forecast {
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
        /// With --json, include the free-space history
        #[arg(long, requires = "json")]
        series: bool,
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
        /// Append errors to ~/Library/Logs/MyDiskIsFull/watch.log
        #[arg(long)]
        log: bool,
        /// Exit when standard input closes: the app that started it quit
        #[arg(long)]
        attached: bool,
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
    /// What to do about what fills the disk, so it stays fixed
    Suggest {
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        history: History,
    },
    /// Do the clean-ups a suggestion offers: remove finished worktrees,
    /// delete build output, move what an app left behind to the Trash,
    /// delete local Time Machine snapshots. Each is checked again first
    Clean {
        /// What a suggestion offers to clean (its action's paths; a local
        /// snapshot by its name, as `tmutil listlocalsnapshots /` gives it)
        #[arg(required = true)]
        paths: Vec<PathBuf>,
        /// Show what would be done, and do nothing
        #[arg(long)]
        dry_run: bool,
        /// Print what was done as JSON
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        history: History,
    },
    /// What is never suggested for deleting, cleaning or moving
    Keep {
        #[command(subcommand)]
        action: Option<KeepAction>,
        /// Print the list as JSON
        #[arg(long, global = true)]
        json: bool,
        /// History database (the list is kept next to it)
        #[arg(long, default_value_os_t = watch_cmd::default_db(), global = true)]
        db: PathBuf,
    },
    /// Clean-ups the watcher does once a day, each once you turn it on: old
    /// simulators, old Rust build caches, caches apps stopped using
    Tidy {
        #[command(subcommand)]
        action: Option<TidyAction>,
        /// Print the list, or what a run did, as JSON
        #[arg(long, global = true)]
        json: bool,
        /// History database (where the switches and the last runs are kept)
        #[arg(long, default_value_os_t = watch_cmd::default_db(), global = true)]
        db: PathBuf,
    },
    /// What raises an alert: how little free space, how soon full, how much a
    /// folder grows or a process writes; and which of them notify
    Alerts {
        #[command(subcommand)]
        action: Option<AlertsAction>,
        /// Print the settings as JSON
        #[arg(long, global = true)]
        json: bool,
        /// History database (where the settings are kept)
        #[arg(long, default_value_os_t = watch_cmd::default_db(), global = true)]
        db: PathBuf,
        /// Volume (the automatic free-space floor depends on its size)
        #[arg(long, default_value = map::DATA_ROOT, global = true)]
        volume: PathBuf,
    },
    /// Settings that stop something from filling the disk again
    Tweaks {
        #[command(subcommand)]
        action: Option<TweakAction>,
        /// Print the list, or what apply and revert did, as JSON
        #[arg(long, global = true)]
        json: bool,
        /// History database (where applied tweaks are recorded)
        #[arg(long, default_value_os_t = watch_cmd::default_db(), global = true)]
        db: PathBuf,
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
    /// Where the space goes: macOS, apps, build output, caches, your files...
    /// biggest first, and what of it is waste
    Space {
        /// Print the result as JSON (what the app reads)
        #[arg(long)]
        json: bool,
        /// Show the last measurement instead of scanning, if it's younger
        /// than this many seconds
        #[arg(long)]
        max_age: Option<u64>,
        /// Scan even if the last measurement is young enough
        #[arg(long)]
        refresh: bool,
        /// History database (the measurement is kept next to it)
        #[arg(long, default_value_os_t = watch_cmd::default_db())]
        db: PathBuf,
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
enum TweakAction {
    /// Apply a tweak, recording what it replaces
    Apply {
        id: String,
        /// Show what it would change, and change nothing
        #[arg(long)]
        dry_run: bool,
    },
    /// Undo a tweak applied with MyDiskIsFull
    Revert { id: String },
}

#[derive(Subcommand)]
enum TidyAction {
    /// Turn a tidy-up on: the watcher does it once a day
    On { id: String },
    /// Turn a tidy-up off
    Off { id: String },
    /// Do them now: those named, or all that are on
    Run {
        ids: Vec<String>,
        /// Show what would be done, and do nothing
        #[arg(long)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
enum AlertsAction {
    /// Set one: low_free (bytes, or auto), full_within_days (1, 3 or 14),
    /// growth_min and churn_per_hour (bytes, like 5GB or 500MB),
    /// notify_forecast, notify_growth, notify_churn (true or false)
    Set { key: String, value: String },
    /// Back to the defaults: those named, or all
    Reset { keys: Vec<String> },
}

#[derive(Subcommand)]
enum KeepAction {
    /// Keep files or folders (`~/` is your home; `*` and `?` match within a name)
    Add {
        #[arg(required = true)]
        paths: Vec<String>,
    },
    /// Stop keeping them
    Remove {
        #[arg(required = true)]
        paths: Vec<String>,
    },
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
    watch_cmd::mark_started();
    match Cli::parse().cmd {
        Cmd::Status { json, history } => status_cmd::status(&history.db, &history.volume, json)?,
        Cmd::Forecast {
            json,
            series,
            history,
        } => watch_cmd::forecast(&history.db, &history.volume, json, series)?,
        Cmd::Watch {
            interval,
            once,
            quiet,
            log,
            attached,
            owners_every,
            history,
        } => watch_cmd::watch(
            &history.db,
            &history.volume,
            watch_cmd::WatchOptions {
                interval: Duration::from_secs(interval),
                owners_every,
                once,
                quiet,
                log,
                attached,
            },
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
        Cmd::Suggest { json, history } => tweaks_cmd::suggest(&history.db, &history.volume, json)?,
        Cmd::Clean {
            paths,
            dry_run,
            json,
            history,
        } => clean_cmd::clean(&history.db, &history.volume, &paths, dry_run, json)?,
        Cmd::Keep { action, json, db } => match action {
            None => keep_cmd::list(&db, json)?,
            Some(KeepAction::Add { paths }) => keep_cmd::add(&db, &paths, json)?,
            Some(KeepAction::Remove { paths }) => keep_cmd::remove(&db, &paths, json)?,
        },
        Cmd::Tidy { action, json, db } => match action {
            None => tidy_cmd::list(&db, json)?,
            Some(TidyAction::On { id }) => tidy_cmd::set(&db, &id, true, json)?,
            Some(TidyAction::Off { id }) => tidy_cmd::set(&db, &id, false, json)?,
            Some(TidyAction::Run { ids, dry_run }) => tidy_cmd::run(&db, &ids, dry_run, json)?,
        },
        Cmd::Alerts {
            action,
            json,
            db,
            volume,
        } => match action {
            None => alerts_cmd::list(&db, &volume, json)?,
            Some(AlertsAction::Set { key, value }) => {
                alerts_cmd::set(&db, &volume, &key, &value, json)?
            }
            Some(AlertsAction::Reset { keys }) => alerts_cmd::reset(&db, &volume, &keys, json)?,
        },
        Cmd::Tweaks { action, json, db } => match action {
            None => tweaks_cmd::list(&db, json)?,
            Some(TweakAction::Apply { id, dry_run }) => tweaks_cmd::apply(&db, &id, dry_run, json)?,
            Some(TweakAction::Revert { id }) => tweaks_cmd::revert(&db, &id, json)?,
        },
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
                big_files: None,
            };
            let m = map::build(&root, &opts)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&m)?);
            } else {
                map_cmd::print_map(&m, depth);
            }
        }
        Cmd::Space {
            json,
            max_age,
            refresh,
            db,
        } => space_cmd::space(&db, json, max_age, refresh)?,
        Cmd::Writers { secs, top } => writers_cmd::writers(secs, top),
    }
    Ok(())
}
