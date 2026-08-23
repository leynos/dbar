//! Coverage for the cache writer's exclusive creation of its temp file.
//!
//! Split from the sibling `tests` module to keep both within the module size
//! cap; the concern here is the write path rather than retention.

use super::*;
use camino::Utf8PathBuf;
use rstest::{fixture, rstest};
use tempfile::TempDir;

/// A temporary directory retained for the lifetime of a test.
type Workspace = Result<(TempDir, Utf8PathBuf), CacheError>;

#[fixture]
fn workspace() -> Workspace {
    let temp_dir = TempDir::new().map_err(CacheError::Io)?;
    let dir = Utf8PathBuf::from_path_buf(temp_dir.path().to_path_buf())
        .map_err(|_| CacheError::InvalidUtf8)?;
    Ok((temp_dir, dir))
}

#[rstest]
fn the_writer_refuses_a_name_it_did_not_create(workspace: Workspace) {
    let (_temp_dir, dir) = workspace.expect("workspace");
    let handle = Dir::open_ambient_dir(&dir, ambient_authority()).expect("open cache directory");
    handle
        .write("sibling.json", "ORIGINAL")
        .expect("seed sibling");
    handle
        .write("plain.tmp", "ORIGINAL")
        .expect("seed plain file");
    // `cap_std` already refuses to traverse a symlink out of the directory the
    // capability was opened on, so the redirection left to close is one
    // pointing at a sibling inside it.
    handle
        .symlink("sibling.json", "link.tmp")
        .expect("plant a symlink at a temp name");

    for name in ["plain.tmp", "link.tmp"] {
        let error =
            create_and_fill(&handle, name, "PWNED").expect_err("an existing name must be refused");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::AlreadyExists,
            "writing through {name} must fail rather than redirect, got {error:?}"
        );
    }

    let sibling = dir.join("sibling.json");
    assert_eq!(
        read_bounded(&handle, "sibling.json", &sibling).expect("read sibling"),
        "ORIGINAL",
        "the symlink target must not have been written through"
    );
    let plain = dir.join("plain.tmp");
    assert_eq!(
        read_bounded(&handle, "plain.tmp", &plain).expect("read plain file"),
        "ORIGINAL",
        "an existing file must not have been truncated"
    );
}
