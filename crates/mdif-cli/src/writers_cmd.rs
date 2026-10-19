//! `mdif writers`: which processes write to disk.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use mdif_core::text::printable;
use mdif_core::units::bytes;
use mdif_macos::proc;

pub fn writers(secs: u64, top: usize) {
    let before: HashMap<i32, proc::ProcIo> = proc::own_processes()
        .into_iter()
        .map(|p| (p.pid, p))
        .collect();
    std::thread::sleep(Duration::from_secs(secs));
    let after = proc::own_processes();

    let mut now: Vec<(u64, &proc::ProcIo)> = after
        .iter()
        .filter_map(|p| {
            let b = before.get(&p.pid).filter(|b| b.name == p.name)?;
            Some((p.bytes_written.saturating_sub(b.bytes_written), p))
        })
        .collect();
    now.sort_by_key(|&(d, _)| Reverse(d));
    println!("Writing now ({secs} s sample, your processes):");
    for (d, p) in now.iter().take(top).filter(|(d, _)| *d > 0) {
        println!(
            "  {:>9}/s  {:>7}  {}",
            bytes(d / secs.max(1)),
            p.pid,
            label(p)
        );
    }

    let mut total: Vec<&proc::ProcIo> = after.iter().collect();
    total.sort_by_key(|p| Reverse(p.bytes_written));
    println!("\nMost written since they started:");
    for p in total.iter().take(top) {
        println!(
            "  {:>9}  {:>7}  {}",
            bytes(p.bytes_written),
            p.pid,
            label(p)
        );
    }
}

/// The process name, and for `diskimages-helper` the disk images it is
/// writing: it writes whatever its clients write into a mounted image, so
/// its bytes belong to them (and the image), not to it.
fn label(p: &proc::ProcIo) -> String {
    let name = printable(&p.name).into_owned();
    if p.name != "diskimages-helper" {
        return name;
    }
    let mut images: Vec<String> = proc::files_open_for_writing(p.pid)
        .iter()
        .filter_map(|f| image_of(f))
        .collect();
    images.dedup();
    if images.is_empty() {
        name
    } else {
        format!("{name} (writing {})", printable(&images.join(", ")))
    }
}

/// A sparse bundle is written band by band: name the bundle, not the band.
fn image_of(file: &Path) -> Option<String> {
    let image = file
        .ancestors()
        .find(|a| a.extension().is_some_and(|e| e == "sparsebundle"))
        .unwrap_or(file);
    Some(image.file_name()?.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn images_are_named_by_their_file_or_bundle() {
        assert_eq!(
            image_of(Path::new("/x/Backup.sparsebundle/bands/1a")).as_deref(),
            Some("Backup.sparsebundle")
        );
        assert_eq!(
            image_of(Path::new("/x/scratch.sparseimage")).as_deref(),
            Some("scratch.sparseimage")
        );
    }
}
