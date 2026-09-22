//! BLE transport: the only module allowed to name `btleplug` types, so the session only sees
//! plain data or opaque wrappers (the role `arduino::bridge` plays for gRPC).

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

/// The app-wide BLE radio, shared by every session so all scan/connect through one adapter.
pub struct Ble {
    adapter: Adapter,
    /// Every peripheral seen by `scan`, keyed by string id: `PeripheralId` can't be rebuilt
    /// from the string sent to the browser, so `connect` looks it up here.
    discovered: Mutex<HashMap<String, Peripheral>>,
    /// Scanning is adapter-global; `release_scan` only stops the adapter if its generation is
    /// current, so a stale scan's cleanup can't kill its replacement.
    scan_generation: AsyncMutex<u64>,
}

/// A scan hit sent to the browser; `id` is the string `connect` expects back.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
}

/// A live connection to one peripheral. Cheap to clone (`Peripheral` is a handle onto shared
/// state), so the session hands clones to its notification pump.
#[derive(Clone)]
pub struct Conn {
    peripheral: Peripheral,
}

/// One notification; `characteristic` is the emitting characteristic's UUID as a string.
pub struct Notification {
    pub characteristic: String,
    pub data: Vec<u8>,
}

impl Ble {
    /// Opens the first Bluetooth adapter. `Ok(None)` means no BLE radio, a normal condition
    /// (e.g. a desktop without Bluetooth); only adapter-subsystem failures are `Err`.
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

    /// Starts a scan, returning its generation (for `release_scan`) and an owned stream of
    /// matching devices, each cached for `connect`, so a spawned task can own the stream.
    pub async fn scan(
        self: Arc<Self>,
        services: Vec<Uuid>,
        name_prefix: Option<String>,
    ) -> Result<(u64, impl Stream<Item = Device> + 'static)> {
        let filter_services = services.clone();
        // Stop any running scan first under the lock, so a restart never races the old scan's
        // teardown into an adapter-level "already scanning" error.
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
                    // A peripheral that briefly fails to resolve (stale advertisement) is
                    // skipped rather than failing the whole stream.
                    let peripheral = this.adapter.peripheral(&id).await.ok()?;
                    let props = peripheral.properties().await.ok()??;
                    // Re-check the filter: BlueZ replays cached devices on `events()`
                    // regardless of the scan filter, which would leak unrelated devices.
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

    /// Stops the scan started under `generation` if it is still current; a stale generation is
    /// a no-op, so releasing a finished scan never stops its replacement.
    pub async fn release_scan(&self, generation: u64) -> Result<()> {
        let current = self.scan_generation.lock().await;
        if *current == generation {
            self.adapter.stop_scan().await?;
        }
        Ok(())
    }

    /// Streams ids of peripherals that disconnect unexpectedly. `events()` is broadcast-backed,
    /// so this runs alongside a concurrent scan without either stream missing events.
    pub async fn disconnect_events(&self) -> Result<impl Stream<Item = String> + Send + 'static> {
        let events = self.adapter.events().await?;
        Ok(events.filter_map(|event| async move {
            match event {
                CentralEvent::DeviceDisconnected(id) => Some(id.to_string()),
                _ => None,
            }
        }))
    }

    /// Connects to a peripheral from a prior scan and discovers its services, so `Conn`'s
    /// characteristic lookups have something to search.
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
    /// Writes `data` to a characteristic identified by service + characteristic UUID, so
    /// callers never hold a `btleplug` handle.
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

    pub async fn read(&self, service: Uuid, char: Uuid) -> Result<Vec<u8>> {
        let characteristic = self.find_characteristic(service, char)?;
        Ok(self.peripheral.read(&characteristic).await?)
    }

    /// Enables notify/indicate for a characteristic.
    pub async fn subscribe(&self, service: Uuid, char: Uuid) -> Result<()> {
        let characteristic = self.find_characteristic(service, char)?;
        self.peripheral.subscribe(&characteristic).await?;
        Ok(())
    }

    /// Disables notify/indicate for a characteristic.
    pub async fn unsubscribe(&self, service: Uuid, char: Uuid) -> Result<()> {
        let characteristic = self.find_characteristic(service, char)?;
        self.peripheral.unsubscribe(&characteristic).await?;
        Ok(())
    }

    /// One owned stream of every notification on this connection, mirroring `btleplug`'s single
    /// multiplexed stream; `'static` so a spawned pump task can own it.
    pub async fn notifications(&self) -> Result<impl Stream<Item = Notification> + Send + 'static> {
        let stream = self.peripheral.notifications().await?;
        Ok(stream.map(|notification| Notification {
            characteristic: notification.uuid.to_string(),
            data: notification.value,
        }))
    }

    pub async fn disconnect(&self) -> Result<()> {
        self.peripheral.disconnect().await?;
        Ok(())
    }

    /// Finds the `btleplug` characteristic for a `(service, char)` pair. Relies on
    /// `Ble::connect` having run `discover_services`.
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

/// Whether a device's advertised `services` satisfy a scan filter, mirroring `ScanFilter`
/// semantics: an empty filter matches everything, else at least one UUID must overlap.
pub fn matches_services(advertised: &[Uuid], filter: &[Uuid]) -> bool {
    filter.is_empty() || filter.iter().any(|uuid| advertised.contains(uuid))
}
