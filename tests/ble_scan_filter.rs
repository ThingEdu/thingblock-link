//! Covers the scan service-UUID filter without needing a real adapter:
//! `Ble::scan` re-checks each discovered device's advertised services against
//! the requested filter rather than trusting the adapter-level `ScanFilter`
//! alone, since `btleplug`'s BlueZ backend replays already-cached devices
//! (previously paired or seen) into the event stream regardless of the
//! active filter.

use thingblock_link::service::ble::transport::matches_services;
use uuid::Uuid;

const THINGBOT_SERVICE: Uuid = Uuid::from_u128(0xaa700001_8f6a_4e2c_b369_4060e0bb33aa);
const UNRELATED_SERVICE: Uuid = Uuid::from_u128(0x0000_2a00_0000_1000_8000_0080_5f9b_34fb);

#[test]
fn empty_filter_matches_any_device() {
    assert!(matches_services(&[], &[]));
    assert!(matches_services(&[THINGBOT_SERVICE], &[]));
}

#[test]
fn matching_service_passes_the_filter() {
    assert!(matches_services(
        &[UNRELATED_SERVICE, THINGBOT_SERVICE],
        &[THINGBOT_SERVICE]
    ));
}

/// Reproduces the bug: a device that doesn't advertise the requested service
/// (e.g. a previously-paired peripheral BlueZ replays into the event stream
/// regardless of the active scan filter) must not pass a non-empty filter.
#[test]
fn device_without_matching_service_is_rejected() {
    assert!(!matches_services(&[UNRELATED_SERVICE], &[THINGBOT_SERVICE]));
}
