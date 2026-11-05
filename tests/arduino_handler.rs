//! End-to-end check of the arduino handler against the bundled arduino-cli
//! v1.5.1 binary. Offline-safe: `Init` only reads the local data dir. Requires
//! the per-platform binary under `arduino-cli-binaries/` to be present.

use thingblock_link::handler::arduino::{ArduinoHandler, default_config_dir};

#[tokio::test]
async fn starts_daemon_and_completes_handshake() {
    let arduino = ArduinoHandler::start(None)
        .await
        .expect("daemon should spawn and complete Create/Init handshake");

    assert_ne!(
        arduino.instance().id,
        0,
        "Init should yield a non-zero instance id"
    );
    // Dropping `daemon` kills the child via kill_on_drop.
}

#[tokio::test]
async fn starts_daemon_with_config_dir_and_reinits() {
    // The in-repo config dir: `arduino-cli.yaml` + the `data/` bundle.
    let arduino = ArduinoHandler::start_with(None, Some(default_config_dir()))
        .await
        .expect("daemon should start against the shipped arduino-cli.yaml");

    assert_ne!(arduino.instance().id, 0);
    arduino
        .reinit()
        .await
        .expect("reinit should re-run Init on the existing instance");
}

#[tokio::test]
async fn missing_config_file_fails_fast() {
    let bogus = std::env::temp_dir().join("thingblock-link-no-such-config-dir");
    let err = ArduinoHandler::start_with(None, Some(bogus.clone()))
        .await
        .err()
        .expect("a missing arduino-cli.yaml should fail startup");

    assert!(
        err.to_string().contains("arduino-cli.yaml"),
        "error should name the missing config file: {err}"
    );
}
