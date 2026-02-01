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

