//! `mdif docker`: what `docker system prune` would remove, item by item, and
//! the images no container uses, which only `-a` removes (see
//! mdif_core::docker). Nothing is removed here: `mdif clean <item>` does it.

use anyhow::Result;
use mdif_core::docker::{self, Item, Report};
use mdif_core::text::printable;
use mdif_core::units::bytes;

use crate::watch_cmd::home;

pub fn show(json_out: bool) -> Result<()> {
    let r = docker::report(&home());
    if json_out {
        println!("{}", serde_json::to_string_pretty(&r)?);
    } else {
        print(&r);
    }
    Ok(())
}

fn print(r: &Report) {
    if let Some(d) = r.disk {
        println!(
            "Docker Desktop's disk image takes {} on this disk.",
            bytes(d)
        );
    }
    if let Some(why) = &r.why {
        println!("{}", printable(why));
        return;
    }
    let pruned: Vec<&Item> = r
        .containers
        .iter()
        .chain(&r.dangling)
        .chain(&r.networks)
        .chain(&r.cache)
        .collect();
    if pruned.is_empty() {
        println!(
            "\n`docker system prune` would remove nothing: no stopped containers, untagged \
             images, unused networks or unused build cache."
        );
    } else {
        println!(
            "\n`docker system prune` would remove ({}):",
            bytes(pruned.iter().map(|i| i.bytes).sum())
        );
        list(&pruned);
        if !r.containers.is_empty() {
            println!("  What a container wrote outside its volumes goes with it.");
        }
    }
    if !r.unused.is_empty() {
        println!(
            "\nImages no container uses, which `docker system prune -a` removes too (about \
             {}; Docker downloads or builds them again when needed):",
            bytes(r.unused.iter().map(|i| i.bytes).sum())
        );
        list(&r.unused.iter().collect::<Vec<_>>());
    }
    if r.cache_shared > 0 {
        println!(
            "\nThe build cache shares {} with the images: that goes only with them.",
            bytes(r.cache_shared)
        );
    }
    if r.volumes > 0 {
        println!(
            "\nLeft alone: {} volume{} no container uses. Volumes hold data.",
            r.volumes,
            if r.volumes == 1 { "" } else { "s" }
        );
    }
    if !pruned.is_empty() || !r.unused.is_empty() {
        println!("\n`mdif clean <item>` removes one (`--dry-run` checks first).");
    }
}

fn list(items: &[&Item]) {
    for i in items {
        println!(
            "  {:>9}  {}  ({})\n             {}",
            bytes(i.bytes),
            printable(&i.title),
            printable(&i.detail),
            printable(&i.name)
        );
    }
}
