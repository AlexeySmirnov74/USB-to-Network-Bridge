//! Regression: cached connection reports must never claim a live device indefinitely.
use usbbridge::recovery::{ClientReport, ClientStatus, ConnectionStatus, DeviceStatus};
fn status(updated: u64) -> ClientStatus {
    ClientStatus {
        updated_unix_ms: updated,
        server: "192.0.2.1:3240".into(),
        phase: "connected".into(),
        error: None,
        devices: vec![DeviceStatus {
            identity: "USB\\VID_1234&PID_5678\\SERIAL".into(),
            busid: "2-4".into(),
            port: Some(1),
            phase: "connected".into(),
            error: None,
            retry_in_seconds: 0,
        }],
    }
}
#[test]
fn only_stale_connections_lose_their_connected_claim_and_keep_identity() {
    let now = 1_000_000;
    let mut report = ClientReport {
        selected: status(1),
        servers: vec![
            ConnectionStatus {
                key: "old".into(),
                name: "Old".into(),
                status: status(1),
            },
            ConnectionStatus {
                key: "live".into(),
                name: "Live".into(),
                status: status(now),
            },
        ],
    };
    report.invalidate_stale(now);
    assert_eq!(report.selected.phase, "agent_unavailable");
    assert_eq!(report.servers[0].status.devices[0].port, None);
    assert_eq!(
        report.servers[0].status.devices[0].identity,
        "USB\\VID_1234&PID_5678\\SERIAL"
    );
    assert_eq!(report.servers[1].status.devices[0].port, Some(1));
    assert_eq!(report.servers[1].status.phase, "connected");
}
