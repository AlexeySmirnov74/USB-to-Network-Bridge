#[cfg(windows)]
pub mod agent;
pub mod backend;
pub mod budget;
#[cfg(windows)]
pub mod camera;
pub mod catalog;
pub mod client;
#[cfg(windows)]
pub mod control;
pub mod credentials;
#[cfg(windows)]
pub mod desktop;
pub mod device_history;
#[cfg(windows)]
pub(crate) mod device_icons;
pub mod device_kind;
pub mod diagnostics;
pub mod discovery;
pub mod health;
pub mod host_status;
pub mod i18n;
#[cfg(windows)]
pub mod instance;
pub mod inventory;
#[cfg(windows)]
pub mod launcher;
pub mod management;
#[cfg(windows)]
mod native_capture;
#[cfg(windows)]
mod ordering;
#[cfg(windows)]
pub mod pnp_diagnostics;
pub mod proxy;
#[cfg(windows)]
pub mod proxy_relay;
pub mod publication;
pub mod recovery;
pub mod server;
pub mod settings;
#[cfg(windows)]
pub mod supervisor;
#[cfg(windows)]
pub mod usb_snapshot;
pub const APP_NAME: &str = "USB to Network Bridge";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const REPOSITORY: &str = "https://github.com/AlexeySmirnov74/USB-to-Network-Bridge";
#[cfg(windows)]
pub mod tray;
#[cfg(windows)]
pub mod usbdk;
#[cfg(windows)]
pub mod vbox;
#[cfg(windows)]
pub mod vhci;
pub mod wire;

use std::{io, net::TcpStream, time::Duration};

pub fn tune_socket(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let socket = socket2::SockRef::from(stream);
    socket.set_tcp_keepalive(
        &socket2::TcpKeepalive::new()
            .with_time(Duration::from_secs(60))
            .with_interval(Duration::from_secs(15)),
    )?;
    Ok(())
}
