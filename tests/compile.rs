//! Coverage for the hardware-free parts of `compile`: locating the build
//! artifact in a build directory, and tolerantly parsing the `options` payload.
//! The streamed gRPC translation needs a live daemon, so it is exercised by
//! manual end-to-end runs rather than here (the boundary `board_list` also draws).

use std::fs;

use thingblock_link::server::protocol::CompileOptions;
use thingblock_link::service::arduino::grpc::compile::find_artifact;
use thingblock_link::utils::tempdir::TempDir;

/// A build dir under a self-cleaning temp dir, plus a helper to drop files in it.
fn build_dir() -> TempDir {
    TempDir::new("thingblock-link-test").expect("create temp build dir")
}

fn touch(dir: &TempDir, name: &str) {
    fs::write(dir.path().join(name), b"").expect("write fixture file");
}

#[test]
fn finds_hex_artifact() {
    let dir = build_dir();
    touch(&dir, "sketch.ino.hex");

    let artifact = find_artifact(dir.path(), &[]).expect("hex artifact found");
    assert_eq!(artifact.format, "hex");
    assert!(artifact.path.ends_with("sketch.ino.hex"));
}

#[test]
fn prefers_hex_over_bin() {
    let dir = build_dir();
    touch(&dir, "sketch.ino.bin");
    touch(&dir, "sketch.ino.hex");

    let artifact = find_artifact(dir.path(), &[]).expect("artifact found");
    assert_eq!(artifact.format, "hex", "AVR hex wins over ESP bin");
}

#[test]
fn falls_back_to_bin() {
    let dir = build_dir();
    touch(&dir, "sketch.ino.bin");
    touch(&dir, "sketch.ino.elf"); // not flashable; ignored

    let artifact = find_artifact(dir.path(), &[]).expect("bin artifact found");
    assert_eq!(artifact.format, "bin");
    assert!(artifact.path.ends_with("sketch.ino.bin"));
}

#[test]
fn skips_bootloader_merged_variant() {
    let dir = build_dir();
    touch(&dir, "sketch.ino.with_bootloader.hex");
    touch(&dir, "sketch.ino.hex");

    let artifact = find_artifact(dir.path(), &[]).expect("artifact found");
    assert!(
        artifact.path.ends_with("sketch.ino.hex"),
        "the plain `.ino.hex` is chosen, not the bootloader-merged one"
    );
}

#[test]
fn no_flashable_binary_is_none() {
    let dir = build_dir();
    touch(&dir, "sketch.ino.elf");
    // A bootloader-merged hex alone is not matched by the `.ino.hex` suffix.
    touch(&dir, "sketch.ino.with_bootloader.hex");

    assert!(find_artifact(dir.path(), &[]).is_none());
}

#[test]
fn empty_build_dir_is_none() {
    let dir = build_dir();
    assert!(find_artifact(dir.path(), &[]).is_none());
}

#[test]
fn missing_build_dir_is_none() {
    let dir = build_dir();
    let absent = dir.path().join("does-not-exist");
    assert!(find_artifact(&absent, &[]).is_none());
}

#[test]
fn options_default_when_empty() {
    let opts: CompileOptions = serde_json::from_value(serde_json::json!({})).unwrap();
    assert!(!opts.verbose);
    assert!(opts.warnings.is_none());
    assert!(opts.libraries.is_empty());
    assert!(opts.build_properties.is_empty());
}

#[test]
fn options_ignore_unknown_keys() {
    let opts: CompileOptions = serde_json::from_value(serde_json::json!({
        "verbose": true,
        "warnings": "all",
        "libraries": ["/libs/Servo"],
        "buildProperties": ["build.extra_flags=-DFOO"],
        "somethingTheEditorAdded": 42,
    }))
    .expect("unknown keys are ignored");

    assert!(opts.verbose);
    assert_eq!(opts.warnings.as_deref(), Some("all"));
    assert_eq!(opts.libraries, ["/libs/Servo"]);
    assert_eq!(opts.build_properties, ["build.extra_flags=-DFOO"]);
}

/// The build properties an ESP compile reports, with the C3's bootloader offset.
fn esp_props() -> Vec<String> {
    vec!["build.bootloader_addr=0x0".to_string()]
}

/// Lay down the four images an ESP build produces.
fn esp_build(dir: &TempDir) {
    touch(dir, "sketch.ino.bin");
    touch(dir, "sketch.ino.bootloader.bin");
    touch(dir, "sketch.ino.partitions.bin");
    touch(dir, "boot_app0.bin");
}

#[test]
fn esp_build_yields_four_parts_in_flash_order() {
    let dir = build_dir();
    esp_build(&dir);

    let artifact = find_artifact(dir.path(), &esp_props()).expect("bin artifact found");
    let offsets: Vec<u32> = artifact.parts.iter().map(|p| p.offset).collect();
    assert_eq!(offsets, [0x0, 0x8000, 0xe000, 0x10000]);
    assert!(
        artifact.parts[0]
            .path
            .ends_with("sketch.ino.bootloader.bin")
    );
    assert!(artifact.parts[3].path.ends_with("sketch.ino.bin"));
}

#[test]
fn bootloader_offset_comes_from_build_properties() {
    let dir = build_dir();
    esp_build(&dir);

    // The classic ESP32 puts its bootloader at 0x1000, not 0x0.
    let props = vec!["build.bootloader_addr=0x1000".to_string()];
    let artifact = find_artifact(dir.path(), &props).expect("artifact found");
    assert_eq!(artifact.parts[0].offset, 0x1000);
}

#[test]
fn incomplete_esp_build_yields_no_parts() {
    let dir = build_dir();
    touch(&dir, "sketch.ino.bin");
    touch(&dir, "sketch.ino.bootloader.bin"); // partitions + boot_app0 missing

    let artifact = find_artifact(dir.path(), &esp_props()).expect("artifact found");
    assert!(
        artifact.parts.is_empty(),
        "a partial image set must not be flashed"
    );
    assert!(artifact.path.ends_with("sketch.ino.bin"));
}

#[test]
fn missing_bootloader_addr_yields_no_parts() {
    let dir = build_dir();
    esp_build(&dir);

    let artifact = find_artifact(dir.path(), &[]).expect("artifact found");
    assert!(artifact.parts.is_empty(), "no offset, no parts");
}

#[test]
fn avr_hex_has_no_parts() {
    let dir = build_dir();
    touch(&dir, "sketch.ino.hex");

    let artifact = find_artifact(dir.path(), &esp_props()).expect("artifact found");
    assert_eq!(artifact.format, "hex");
    assert!(
        artifact.parts.is_empty(),
        "Intel HEX carries its own addresses"
    );
}
