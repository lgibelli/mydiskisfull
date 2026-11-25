//! Where a file's data physically lives: NTFS has no clones, so nothing
//! shares blocks and nothing asks (`Entry::may_share_blocks` is false).

use std::io;
use std::path::Path;

/// Physical extents of the file at `path`. Not on Windows.
pub fn physical(_path: &Path, _block: u64) -> io::Result<Vec<(u64, u64)>> {
    Err(crate::unsupported("reading a file's extents"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_asks_and_nothing_answers() {
        let e = physical(Path::new(r"C:\"), 4096).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Unsupported);
    }
}
