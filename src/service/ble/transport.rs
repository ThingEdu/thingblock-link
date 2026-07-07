//! BLE transport, contained behind plain-data types.
//!
//! This is the only module allowed to name a `btleplug` type — the same role
//! [`crate::service::arduino::bridge`] plays for gRPC. Everything it exposes
//! ([`Device`], [`Conn`], [`Notification`]) is plain data or an opaque
//! wrapper, so [`super::session`] never writes `use btleplug::...`.
//!
//! `btleplug` already gives synchronous-feeling async methods for connect,
//! read, write, and friends; this module only adds what it doesn't: a
//! round-trip from string id back to a `Peripheral` handle (`btleplug`'s
//! `PeripheralId` can't be rebuilt from a string), containment of the
//! `btleplug` types themselves, reshaping its single multiplexed notification
//! stream into a per-connection one, and mapping its errors onto
//! [`crate::error::Error`].

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use btleplug::api::{
    Central, CentralEvent, Characteristic, Manager as _, Peripheral as _, ScanFilter, WriteType,
};
use btleplug::platform::{Adapter, Manager, Peripheral};
use futures::{Stream, StreamExt};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::error::{Error, Result};

/// One BLE radio, app-wide. A later step stores this in `AppState` behind an
/// `Arc` so every session can scan/connect through the same adapter.
///
/// `discovered` caches every `Peripheral` handle seen by [`Ble::scan`], keyed
/// by the string form of its `btleplug` id. `connect` looks a peripheral up
/// here rather than re-scanning, since `PeripheralId` cannot be reconstructed
/// from the string we hand to the browser.
///
/// Scanning is adapter-global, so `scan_generation` arbitrates ownership:
/// each [`Ble::scan`] bumps it under the lock, and [`Ble::release_scan`] only
/// stops the adapter if the caller's generation is still current. That way a
/// cancelled scan's cleanup can never kill the scan that replaced it, and the
/// lock serializes start/stop calls so the adapter never sees a start while
/// an older scan is still being torn down.
pub struct Ble {
    adapter: Adapter,
    discovered: Mutex<HashMap<String, Peripheral>>,
    scan_generation: AsyncMutex<u64>,
}

/// A scan hit, handed to the browser. Carries the same string id `connect`
/// expects back.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
}

/// A live connection to one peripheral. A session owns one per connected
/// device for its lifetime, and hands clones to its notification pump task —
/// cheap, since `Peripheral` is itself a handle onto shared `btleplug` state.
#[derive(Clone)]
pub struct Conn {
    peripheral: Peripheral,
}

/// One notification, reshaped from `btleplug`'s single per-peripheral stream.
/// `characteristic` is the emitting characteristic's UUID in string form.
pub struct Notification {
    pub characteristic: String,
    pub data: Vec<u8>,
}

impl Ble {
    /// Open the first available Bluetooth adapter. `Ok(None)` means the
    /// machine has no BLE radio, which is a normal condition (e.g. a desktop
    /// with no Bluetooth card) — not an error. Any failure talking to the
    /// adapter subsystem itself is `Err`.
    pub async fn discover() -> Result<Option<Ble>> {
        let manager = Manager::new().await?;
        let mut adapters = manager.adapters().await?;
        if adapters.is_empty() {
            return Ok(None);
        }
        let adapter = adapters.remove(0);
        Ok(Some(Ble {
            adapter,
            discovered: Mutex::new(HashMap::new()),
            scan_generation: AsyncMutex::new(0),
        }))
    }

    /// Start scanning and return the scan's generation plus a stream of
    /// matching [`Device`]s. Takes `Arc<Self>` and returns a `'static` stream
    /// so a caller can spawn a task that owns the stream independently of
    /// whoever called `scan`. Pass the generation to [`Ble::release_scan`]
    /// when done with the stream.
    ///
    /// Any scan already running is stopped first (its owner's later
    /// `release_scan` becomes a no-op once the generation moves on), so
    /// restarting a scan never races the previous scan's teardown into an
    /// adapter-level "already scanning" error.
    ///
    /// Every peripheral the adapter reports (discovered or updated) is
    /// resolved to its properties, filtered by `services` and `name_prefix`
    /// if given, cached in `discovered` under its string id, and yielded as a
    /// `Device`. A peripheral that briefly fails to resolve (e.g. a stale
    /// advertisement) is skipped rather than failing the whole stream.
    ///
    /// The `services` filter is re-checked here against each device's own
    /// advertised services rather than trusted to `start_scan`'s adapter-level
    /// filter alone: on Linux, `btleplug`'s BlueZ backend replays a
    /// `DeviceDiscovered` event for every device the adapter already has
    /// cached (previously paired or seen) when [`Adapter::events`] opens,
    /// regardless of the active scan filter, so an unrelated cached device
    /// can otherwise leak through as a false hit.
    pub async fn scan(
        self: Arc<Self>,
        services: Vec<Uuid>,
        name_prefix: Option<String>,
    ) -> Result<(u64, impl Stream<Item = Device> + 'static)> {
        let filter_services = services.clone();
        let generation = {
            let mut current = self.scan_generation.lock().await;
            *current += 1;
            let _ = self.adapter.stop_scan().await;
            self.adapter.start_scan(ScanFilter { services }).await?;
            *current
        };
        let events = self.adapter.events().await?;

        Ok((
            generation,
            events.filter_map(move |event| {
                let this = self.clone();
                let name_prefix = name_prefix.clone();
                let filter_services = filter_services.clone();
                async move {
                    let id = match event {
                        CentralEvent::DeviceDiscovered(id) | CentralEvent::DeviceUpdated(id) => id,
                        _ => return None,
                    };
                    let peripheral = this.adapter.peripheral(&id).await.ok()?;
                    let props = peripheral.properties().await.ok()??;
                    if !matches_services(&props.services, &filter_services) {
                        return None;
                    }
                    if let Some(prefix) = &name_prefix {
                        let matches = props
                            .local_name
                            .as_deref()
                            .is_some_and(|name| name.starts_with(prefix.as_str()));
                        if !matches {
                            return None;
                        }
                    }

                    let key = id.to_string();
                    this.discovered
                        .lock()
                        .expect("discovered mutex")
                        .insert(key.clone(), peripheral);
                    Some(Device {
                        id: key,
                        name: props.local_name,
                        rssi: props.rssi,
                    })
                }
            }),
        ))
    }

    /// Stop the scan started under `generation`, if it is still the adapter's
    /// current one. A stale generation (a newer scan has since started) is a
    /// no-op, so releasing a finished scan can never stop its replacement.
    pub async fn release_scan(&self, generation: u64) -> Result<()> {
        let current = self.scan_generation.lock().await;
        if *current == generation {
            self.adapter.stop_scan().await?;
        }
        Ok(())
    }

    /// A stream of device ids for peripherals that disconnect unexpectedly,
    /// filtered from the adapter's shared event stream. `btleplug` backs
    /// `events()` with a broadcast channel, so this can run alongside a
    /// concurrent [`Ble::scan`] on the same adapter without either stream
    /// missing events.
    pub async fn disconnect_events(&self) -> Result<impl Stream<Item = String> + Send + 'static> {
        let events = self.adapter.events().await?;
        Ok(events.filter_map(|event| async move {
            match event {
                CentralEvent::DeviceDisconnected(id) => Some(id.to_string()),
                _ => None,
            }
        }))
    }

    /// Connect to a peripheral previously seen via [`Ble::scan`] and discover
    /// its services, so [`Conn`]'s characteristic lookups have something to
    /// search.
    pub async fn connect(&self, id: &str) -> Result<Conn> {
        let peripheral = {
            let discovered = self.discovered.lock().expect("discovered mutex");
            discovered.get(id).cloned()
        }
        .ok_or_else(|| {
            Error::Ble(format!(
                "device {id} was not discovered via scan; connect requires a live scan result"
            ))
        })?;

        peripheral.connect().await?;
        peripheral.discover_services().await?;
        Ok(Conn { peripheral })
    }
}

impl Conn {
    /// Write `data` to a characteristic, identified by its service + own
    /// UUID rather than a `btleplug` handle.
    pub async fn write(
        &self,
        service: Uuid,
        char: Uuid,
        data: &[u8],
        with_response: bool,
    ) -> Result<()> {
        let characteristic = self.find_characteristic(service, char)?;
        let write_type = if with_response {
            WriteType::WithResponse
        } else {
            WriteType::WithoutResponse
        };
        self.peripheral
            .write(&characteristic, data, write_type)
            .await?;
        Ok(())
    }

    /// Read the current value of a characteristic.
    pub async fn read(&self, service: Uuid, char: Uuid) -> Result<Vec<u8>> {
        let characteristic = self.find_characteristic(service, char)?;
        Ok(self.peripheral.read(&characteristic).await?)
    }

    /// Enable notify/indicate for a characteristic.
    pub async fn subscribe(&self, service: Uuid, char: Uuid) -> Result<()> {
        let characteristic = self.find_characteristic(service, char)?;
        self.peripheral.subscribe(&characteristic).await?;
        Ok(())
    }

    /// Disable notify/indicate for a characteristic.
    pub async fn unsubscribe(&self, service: Uuid, char: Uuid) -> Result<()> {
        let characteristic = self.find_characteristic(service, char)?;
        self.peripheral.unsubscribe(&characteristic).await?;
        Ok(())
    }

    /// A single stream of every notification for this connection. `btleplug`
    /// multiplexes all characteristics of a peripheral onto one stream and
    /// tags each item with only the characteristic's UUID, so this mirrors
    /// that shape rather than faking per-characteristic streams the
    /// underlying API doesn't provide.
    ///
    /// Returns an owned, `'static` stream (the underlying `btleplug` stream
    /// is already a boxed `dyn Stream + Send` with no borrow on the
    /// peripheral handle) so a caller can move it into a spawned pump task
    /// that outlives this call.
    pub async fn notifications(&self) -> Result<impl Stream<Item = Notification> + Send + 'static> {
        let stream = self.peripheral.notifications().await?;
        Ok(stream.map(|notification| Notification {
            characteristic: notification.uuid.to_string(),
            data: notification.value,
        }))
    }

    /// Terminate the connection.
    pub async fn disconnect(&self) -> Result<()> {
        self.peripheral.disconnect().await?;
        Ok(())
    }

    /// Locate the `btleplug` characteristic for a `(service, char)` pair.
    /// Requires `discover_services` to have already populated
    /// `self.peripheral`'s characteristic set, which [`Ble::connect`]
    /// guarantees.
    fn find_characteristic(&self, service: Uuid, char: Uuid) -> Result<Characteristic> {
        self.peripheral
            .characteristics()
            .into_iter()
            .find(|c| c.service_uuid == service && c.uuid == char)
            .ok_or_else(|| {
                Error::Ble(format!(
                    "characteristic {char} on service {service} not found on this peripheral"
                ))
            })
    }
}

/// Whether a device's advertised `services` satisfy a scan's `filter`,
/// mirroring [`ScanFilter`]'s documented semantics: an empty filter matches
/// everything, a non-empty one requires at least one overlapping UUID.
pub fn matches_services(advertised: &[Uuid], filter: &[Uuid]) -> bool {
    filter.is_empty() || filter.iter().any(|uuid| advertised.contains(uuid))
}
