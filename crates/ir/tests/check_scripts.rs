//! The repository guards must reject their seeded defects, including a stale-HEAD push.
#[test]
fn repository_guards_reject_seeded_defects() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let status = std::process::Command::new("python3")
        .arg(root.join("scripts/test_checks.py"))
        .current_dir(&root)
        .status()
        .expect("Python 3 runs repository check witnesses");
    assert!(status.success(), "repository check witnesses failed");
}
