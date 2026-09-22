//! The only module allowed to name `btleplug` types; everything it exposes is plain data or opaque.

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

pub struct Ble {
    adapter: Adapter,
    /// `PeripheralId` can't be rebuilt from the string sent to the browser, so `connect` looks up here.
    discovered: Mutex<HashMap<String, Peripheral>>,
    /// Scanning is adapter-global; `release_scan` only stops the adapter if its generation is current.
    scan_generation: AsyncMutex<u64>,
}

#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
    pub rssi: Option<i16>,
}

#[derive(Clone)]
pub struct Conn {
    peripheral: Peripheral,
}

pub struct Notification {
    pub characteristic: String,
    pub data: Vec<u8>,
}

impl Ble {
    /// `Ok(None)` means no BLE radio, a normal condition rather than an error.
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

    /// Stops any running scan first, so a restart never hits an adapter-level "already scanning" error.
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
                    // BlueZ replays cached devices on `events()` regardless of the scan filter.
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

    pub async fn release_scan(&self, generation: u64) -> Result<()> {
        let current = self.scan_generation.lock().await;
        if *current == generation {
            self.adapter.stop_scan().await?;
        }
        Ok(())
    }

    /// `events()` is broadcast-backed, so this runs alongside a concurrent scan without missing events.
    pub async fn disconnect_events(&self) -> Result<impl Stream<Item = String> + Send + 'static> {
        let events = self.adapter.events().await?;
        Ok(events.filter_map(|event| async move {
            match event {
                CentralEvent::DeviceDisconnected(id) => Some(id.to_string()),
                _ => None,
            }
        }))
    }

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

    pub async fn subscribe(&self, service: Uuid, char: Uuid) -> Result<()> {
        let characteristic = self.find_characteristic(service, char)?;
        self.peripheral.subscribe(&characteristic).await?;
        Ok(())
    }

    pub async fn unsubscribe(&self, service: Uuid, char: Uuid) -> Result<()> {
        let characteristic = self.find_characteristic(service, char)?;
        self.peripheral.unsubscribe(&characteristic).await?;
        Ok(())
    }

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

    /// Relies on [`Ble::connect`] having run `discover_services`.
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

/// Mirrors [`ScanFilter`] semantics: an empty filter matches everything.
pub fn matches_services(advertised: &[Uuid], filter: &[Uuid]) -> bool {
    filter.is_empty() || filter.iter().any(|uuid| advertised.contains(uuid))
}
