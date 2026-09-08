//! Coverage for `ResourceRoot` — the hardware-free startup validation of the
//! configured resource root (Flow 1) and `resolve_lib_dir`, which turns an
//! untrusted `{pack, lib}` reference into a local library directory (Flow 2).

use std::fs;

use thingblock_link::service::resource::ResourceRoot;
use thingblock_link::utils::tempdir::TempDir;

#[test]
fn new_accepts_an_existing_dir_and_canonicalizes() {
    let dir = TempDir::new("thingblock-link-resource").expect("create temp dir");
    let root = ResourceRoot::new(dir.path()).expect("existing dir is a valid root");

    // The stored path is absolute and still points at the same directory.
    assert!(root.path().is_absolute());
    assert_eq!(root.path(), dir.path().canonicalize().unwrap());
}

#[test]
fn new_rejects_a_missing_path() {
    let dir = TempDir::new("thingblock-link-resource").expect("create temp dir");
    let missing = dir.path().join("nope");

    assert!(ResourceRoot::new(&missing).is_err());
}

#[test]
fn new_rejects_a_file() {
    let dir = TempDir::new("thingblock-link-resource").expect("create temp dir");
    let file = dir.path().join("not-a-dir");
    fs::write(&file, b"x").expect("write file");

    assert!(ResourceRoot::new(&file).is_err());
}

#[test]
fn resolve_lib_dir_resolves_a_vendored_dir() {
    let dir = TempDir::new("thingblock-link-resource").expect("create temp dir");
    let lib = dir.path().join("dht").join("lib").join("DHT");
    fs::create_dir_all(&lib).expect("create lib dir");
    let root = ResourceRoot::new(dir.path()).expect("valid root");

    let resolved = root
        .resolve_lib_dir("dht", "lib/DHT")
        .expect("existing lib dir resolves");

    assert!(resolved.is_absolute());
    assert!(resolved.starts_with(root.path()));
    assert_eq!(resolved, lib.canonicalize().unwrap());
}

#[test]
fn resolve_lib_dir_rejects_a_missing_ref() {
    let dir = TempDir::new("thingblock-link-resource").expect("create temp dir");
    let root = ResourceRoot::new(dir.path()).expect("valid root");

    assert!(root.resolve_lib_dir("dht", "lib/Nope").is_err());
}

#[test]
fn resolve_lib_dir_rejects_a_file() {
    let dir = TempDir::new("thingblock-link-resource").expect("create temp dir");
    let pack = dir.path().join("dht").join("lib");
    fs::create_dir_all(&pack).expect("create pack dir");
    fs::write(pack.join("DHT.h"), b"x").expect("write file");
    let root = ResourceRoot::new(dir.path()).expect("valid root");

    assert!(root.resolve_lib_dir("dht", "lib/DHT.h").is_err());
}

#[test]
fn resolve_lib_dir_rejects_traversal_escaping_the_root() {
    // A sibling dir outside the root that `../` would reach.
    let parent = TempDir::new("thingblock-link-resource").expect("create temp dir");
    let root_dir = parent.path().join("root");
    let outside = parent.path().join("outside");
    fs::create_dir_all(&root_dir).expect("create root");
    fs::create_dir_all(&outside).expect("create outside dir");
    let root = ResourceRoot::new(&root_dir).expect("valid root");

    // The escape target exists and is a directory, so only the root check rejects it.
    assert!(root.resolve_lib_dir("..", "outside").is_err());
}

#[test]
fn resolve_firmware_file_returns_a_file_inside_the_root() {
    let dir = TempDir::new("thingblock-link-fw").expect("temp dir");
    let pack = dir
        .path()
        .join("extensions/devices/thingbot/firmware/telemetrix-ble");
    fs::create_dir_all(&pack).expect("create pack dir");
    fs::write(pack.join("telemetrix-ble.ino.bin"), b"\x00\x01").expect("write image");
    let root = ResourceRoot::new(dir.path()).expect("resource root");

    let resolved = root
        .resolve_firmware_file(
            "extensions/devices/thingbot",
            "firmware/telemetrix-ble/telemetrix-ble.ino.bin",
        )
        .expect("firmware should resolve");

    assert!(resolved.is_file(), "resolves to the image file");
    assert!(resolved.starts_with(root.path()), "stays under the root");
}

#[test]
fn resolve_firmware_file_refuses_a_path_escaping_the_root() {
    // A sibling dir outside the root that `../` would reach, holding a real file so
    // canonicalize succeeds and the containment check (not a missing-path error) is what
    // rejects it — the same setup `resolve_lib_dir_rejects_traversal_escaping_the_root` uses,
    // since a literal `../../../../etc/hosts` depends on the OS temp dir's nesting depth and
    // does not reliably reach an existing path on every machine.
    let parent = TempDir::new("thingblock-link-fw-esc").expect("temp dir");
    let root_dir = parent.path().join("root");
    let outside = parent.path().join("outside");
    fs::create_dir_all(root_dir.join("extensions/devices/thingbot")).expect("create pack dir");
    fs::create_dir_all(&outside).expect("create outside dir");
    fs::write(outside.join("secret.bin"), b"\x00\x01").expect("write outside file");
    let root = ResourceRoot::new(&root_dir).expect("resource root");

    let err = root
        .resolve_firmware_file(
            "extensions/devices/thingbot",
            "../../../../outside/secret.bin",
        )
        .expect_err("a path leaving the root must be refused");

    assert!(
        err.to_string().contains("escapes the resource root"),
        "error should name the containment failure, got: {err}"
    );
}

#[test]
fn resolve_firmware_file_refuses_a_directory() {
    let dir = TempDir::new("thingblock-link-fw-dir").expect("temp dir");
    let pack = dir
        .path()
        .join("extensions/devices/thingbot/firmware/telemetrix-ble");
    fs::create_dir_all(&pack).expect("create pack dir");
    let root = ResourceRoot::new(dir.path()).expect("resource root");

    let err = root
        .resolve_firmware_file("extensions/devices/thingbot", "firmware/telemetrix-ble")
        .expect_err("a directory is not an image");

    assert!(err.to_string().contains("is not a file"), "got: {err}");
}
