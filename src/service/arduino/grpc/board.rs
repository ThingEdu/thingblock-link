//! `BoardList` translation backing WS `listBoards`. JS `pnpid`s are Windows PNP ids
//! (`USB\VID_2341&PID_0043`), so they're rebuilt from arduino-cli's `vid`/`pid` port properties.

use tracing::warn;

use crate::error::Result;
use crate::server::protocol::ConnectionTarget;
use crate::service::arduino::grpc::{Client, cli};

/// Discovery window for a one-shot board enumeration.
const BOARD_LIST_TIMEOUT_MS: i64 = 1000;

impl Client {
    /// Lists connected boards whose USB pnpid is in `pnpid`, as `ConnectionTarget`s.
    /// Backs WS `listBoards`, so the editor only sees boards its device supports.
    pub async fn board_list(&mut self, pnpid: &[String]) -> Result<Vec<ConnectionTarget>> {
        let instance = *self.instance();
        let resp = self
            .inner()
            .board_list(cli::BoardListRequest {
                instance: Some(instance),
                timeout: BOARD_LIST_TIMEOUT_MS,
                fqbn: String::new(),
                // Offline-safe: never reach out to the cloud to identify a board.
                skip_cloud_api_for_board_detection: true,
            })
            .await?
            .into_inner();

        for warning in &resp.warnings {
            warn!(warning = %warning, "board list discovery warning");
        }

        Ok(detected_ports_to_targets(&resp.ports, pnpid))
    }

    /// Counts connected USB boards for the status window's "boards connected" line.
    /// Also serves as a daemon liveness probe: `Err` means the daemon is unresponsive.
    pub async fn connected_board_count(&mut self) -> Result<usize> {
        let instance = *self.instance();
        let resp = self
            .inner()
            .board_list(cli::BoardListRequest {
                instance: Some(instance),
                timeout: BOARD_LIST_TIMEOUT_MS,
                fqbn: String::new(),
                skip_cloud_api_for_board_detection: true,
            })
            .await?
            .into_inner();

        Ok(count_board_ports(&resp.ports))
    }
}

/// Counts detected ports with a USB vid/pid, so generic serial devices aren't counted.
/// Pure (no I/O) so it is unit-testable without hardware.
pub fn count_board_ports(ports: &[cli::DetectedPort]) -> usize {
    ports
        .iter()
        .filter(|detected| detected.port.as_ref().and_then(port_pnp_id).is_some())
        .count()
}

/// Maps detected ports to `ConnectionTarget`s, keeping only those whose pnpid is in `pnpid`.
/// Pure (no I/O) so it is unit-testable without hardware.
pub fn detected_ports_to_targets(
    ports: &[cli::DetectedPort],
    pnpid: &[String],
) -> Vec<ConnectionTarget> {
    ports
        .iter()
        .filter_map(|detected| {
            let port = detected.port.as_ref()?;
            let id = port_pnp_id(port)?;
            if !pnpid.iter().any(|wanted| wanted.eq_ignore_ascii_case(&id)) {
                return None;
            }
            Some(ConnectionTarget {
                port: port.address.clone(),
                label: port_label(detected, port),
            })
        })
        .collect()
}

/// Rebuilds the Windows PNP id (`USB\VID_xxxx&PID_xxxx`) from a port's `vid`/`pid`.
/// `None` if either is absent, e.g. a network port.
fn port_pnp_id(port: &cli::Port) -> Option<String> {
    let vid = normalize_hex_id(port.properties.get("vid")?);
    let pid = normalize_hex_id(port.properties.get("pid")?);
    Some(format!("USB\\VID_{vid}&PID_{pid}"))
}

/// Strips a leading `0x`/`0X` and uppercases (`0x2341` -> `2341`) to match the PNP id form.
fn normalize_hex_id(raw: &str) -> String {
    raw.strip_prefix("0x")
        .or_else(|| raw.strip_prefix("0X"))
        .unwrap_or(raw)
        .to_uppercase()
}

/// Human label for a target: the first matching board's name, else the port's label,
/// else its address.
fn port_label(detected: &cli::DetectedPort, port: &cli::Port) -> String {
    if let Some(board) = detected.matching_boards.first()
        && !board.name.is_empty()
    {
        return board.name.clone();
    }
    if !port.label.is_empty() {
        return port.label.clone();
    }
    port.address.clone()
}
