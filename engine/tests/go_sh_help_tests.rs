use std::{fs, path::PathBuf};

#[test]
fn go_sh_help_mentions_game_launch_modes() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let go_sh_path = manifest_dir.join("..").join("go.sh");

    let go_sh = fs::read_to_string(&go_sh_path).expect("read go.sh");

    assert!(
        go_sh.contains("./go.sh --start "),
        "go.sh should document windowed game launches"
    );
    assert!(
        go_sh.contains("./go.sh --e2e "),
        "go.sh should document headless playtests"
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

#[test]
fn go_sh_help_mentions_workspace_and_light_test_modes() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let go_sh_path = manifest_dir.join("..").join("go.sh");

    let go_sh = fs::read_to_string(&go_sh_path).expect("read go.sh");

    assert!(
        go_sh.contains("./go.sh --test "),
        "go.sh help should mention --test for the engine and game workspace"
    );
    assert!(
        go_sh.contains("--test-light"),
        "go.sh help should mention --test-light for reduced local CPU pressure"
    );
}
