//! Editing one value of a hand-written settings file: everything the editor does not understand has to survive.

use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dj-config-edit-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("a scratch folder");
    dir.join("settings.toml")
}

#[test]
fn a_neighbouring_key_and_its_comment_survive() {
    let path = scratch("neighbour");
    let original = concat!(
        "# Why this number: 256 frames is what the booth interface likes.\n",
        "[audio]\n",
        "block_frames = 256\n",
        "# A key from a newer build, which this build knows nothing about.\n",
        "future_key = \"keep me\"\n",
        "\n",
        "[display]\n",
        "frame_hz = 20\n",
    );
    std::fs::write(&path, original).expect("the file");

    dj_config::set_value(&path, "audio.block_frames", "512").expect("the edit");
    let after = std::fs::read_to_string(&path).expect("the file back");
    assert!(after.contains("block_frames = 512"), "{after}");
    assert!(after.contains("256 frames is what the booth interface likes"), "the comment is gone: {after}");
    assert!(after.contains("future_key = \"keep me\""), "an unknown key is gone: {after}");
    assert!(after.contains("frame_hz = 20"), "another table is gone: {after}");
}

#[test]
fn a_key_that_is_not_there_yet_is_created_in_its_table() {
    let path = scratch("missing");
    std::fs::write(&path, "[audio]\nblock_frames = 256\n").expect("the file");
    dj_config::set_value(&path, "audio.device", "\"s4\"").expect("the edit");
    let after = std::fs::read_to_string(&path).expect("the file back");
    assert!(after.contains("device = \"s4\""), "{after}");
    assert!(after.contains("block_frames = 256"), "the neighbouring key is gone: {after}");
}

#[test]
fn a_table_that_is_not_there_yet_is_created() {
    let path = scratch("table");
    std::fs::write(&path, "[audio]\nblock_frames = 256\n").expect("the file");
    dj_config::set_value(&path, "diagnostics.log_level", "\"info\"").expect("the edit");
    let after = std::fs::read_to_string(&path).expect("the file back");
    assert!(after.contains("[diagnostics]"), "{after}");
    assert!(after.contains("log_level = \"info\""), "{after}");
    // And the file the engine reads still parses into the typed configuration.
    let (config, _used) = dj_config::Config::load_or_default(Some(&path)).expect("the file still parses");
    assert_eq!(config.audio.block_frames, 256);
    assert_eq!(config.diagnostics.level(), Some("info"));
}

#[test]
fn an_unknown_log_level_is_ignored_rather_than_silencing_the_log() {
    let path = scratch("level-typo");
    std::fs::write(&path, "[diagnostics]\nlog_level = \"loud\"\n").expect("the file");
    let (config, _used) = dj_config::Config::load_or_default(Some(&path)).expect("the file still parses");
    assert_eq!(config.diagnostics.level(), None, "a typo must not pick a level");
}

#[test]
fn a_value_is_read_back_as_toml_text() {
    let path = scratch("read");
    std::fs::write(&path, "[audio]\nblock_frames = 256\n").expect("the file");
    let value = dj_config::get_value(&path, "audio.block_frames").expect("the read");
    assert_eq!(value.as_deref(), Some("256"));
    assert_eq!(dj_config::get_value(&path, "audio.device").expect("the read"), None);
}

#[test]
fn a_value_toml_cannot_parse_is_refused_and_the_file_is_untouched() {
    let path = scratch("refuse");
    std::fs::write(&path, "[audio]\nblock_frames = 256\n").expect("the file");
    assert!(dj_config::set_value(&path, "audio.block_frames", "not a value").is_err());
    let after = std::fs::read_to_string(&path).expect("the file back");
    assert_eq!(after, "[audio]\nblock_frames = 256\n", "a refused edit changed the file");
}

#[test]
fn the_first_change_on_a_fresh_install_creates_the_folder_and_the_file() {
    let dir = std::env::temp_dir().join(format!("dj-config-edit-fresh-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("hauler-dj").join("config.toml");
    dj_config::set_value(&path, "audio.block_frames", "1024").expect("the first change");
    let after = std::fs::read_to_string(&path).expect("the file the change made");
    assert!(after.contains("block_frames = 1024"), "{after}");
    let _ = std::fs::remove_dir_all(&dir);
}
