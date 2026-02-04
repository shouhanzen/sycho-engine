use std::{fs, path::PathBuf};

#[test]
fn go_sh_help_mentions_editor_and_game_targets() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let go_sh_path = manifest_dir.join("..").join("go.sh");

    let go_sh = fs::read_to_string(&go_sh_path).expect("read go.sh");

    assert!(
        go_sh.contains("--game"),
        "go.sh should document a --game target flag in its help text"
    );
    assert!(
        go_sh.contains("--editor"),
        "go.sh should document an --editor target flag in its help text"
    );
}

#[test]
fn go_sh_start_is_foreground_by_default_and_detach_is_explicit() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let go_sh_path = manifest_dir.join("..").join("go.sh");

    let go_sh = fs::read_to_string(&go_sh_path).expect("read go.sh");

    assert!(
        go_sh.contains("--detach"),
        "go.sh should document a --detach flag (background mode) in its help text"
    );
    assert!(
        go_sh.contains("Default is foreground"),
        "go.sh should document that --start runs in the foreground by default"
    );
    assert!(
        !go_sh.contains("Default is background"),
        "go.sh should not claim that --start runs in the background by default"
    );
}

#[test]
fn go_sh_help_mentions_build_cache_knobs_for_worktrees() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let go_sh_path = manifest_dir.join("..").join("go.sh");

    let go_sh = fs::read_to_string(&go_sh_path).expect("read go.sh");

    assert!(
        go_sh.contains("sccache"),
        "go.sh help should mention sccache for faster builds across worktrees"
    );
    assert!(
        go_sh.contains("CARGO_TARGET_DIR"),
        "go.sh help should mention CARGO_TARGET_DIR / shared target dir for multi-worktree builds"
    );
}
