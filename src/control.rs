//! GUI-equivalent administration using the same settings locks and elevated helpers.
use crate::{
    agent,
    desktop::Role,
    i18n,
    settings::{self, ClientSettings, HostSettings, SavedServer},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    io::{self, BufRead},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

pub const HELP: &str = concat!(
    "USB to Network Bridge v.",
    env!("CARGO_PKG_VERSION"),
    r#" — command line control
usbbridge [--config FILE] [--language en|ru|de|fr|zh] server|client COMMAND

Both roles:
  gui                                  Open the interface
  hide | close-ui                      Hide window / close tray interface
  language [en|ru|de|fr|zh]              Read/save interface language (default en)
  autostart status|on|off                Background recovery and tray at Windows startup
  health                                Check supervisor and controller heartbeats
  logs                                  Print log locations
  video settings|status|on|off           Manage USB camera receiver/server or video sender/client
  video configure FILE                  Apply video settings JSON (key_file contains no inline secret)
  local-camera settings|status|on|off    Client-local USB camera and Unity Capture Client sender
  local-camera configure FILE           Apply local receiver settings and keep its sender synchronized
  about                                 Print version and repository
  cameras                               List cameras available in local Windows
  camera-modes CAMERA                   List video modes for an exact camera path/identity
  preview [USB_ID]                       Open camera preview with live FPS
  camera-test CAMERA [--mode N] [--seconds N] [--low-light on|off]  Count real camera frames without saving images

Server:
  status                                Running state, ports and devices
  devices | refresh                     Current complete device list
  start | stop                          Start/stop the selected backend
  settings                              Read current settings
  settings [--name NAME] [--tcp-port N] [--discovery-port N]
           [--backend Native|Usbipd] [--allow IP ...]
  publish ID on|off                     Share/unshare one identity or USB bus ID
  local ID on|off                       Local use while idle (the Enabled checkbox)
  forget ID                             Forget an unplugged device (connected devices are refused)
  settings --auto-remove-unplugged on|off --unplugged-days N

Client:
  discover | list HOST[:PORT]|auto        Discover servers/list shared devices
  devices                               All devices, including unshared ones
  status | ports                        Recovery report/virtual USB ports
  servers                               Saved servers and current selection
  add HOST[:PORT] [--name NAME] [--id UUID]
  select SERVER                         Save the selected server
  pause SERVER | resume SERVER          Disconnect/reconnect this server; preserve device choices
  remove SERVER                         Disconnect its devices and forget server
  address SERVER HOST[:PORT]             Change a saved server's address/port
  settings [--discovery on|off] [--discovery-port N]
           [--poll-seconds N] [--retry-max-seconds N]
  proxy SERVER direct
  proxy SERVER http|https|socks5 HOST:PORT [--auth-stdin]
  connect [SERVER|HOST[:PORT]|auto] BUSID  Connect now, remember and recover
  enable SERVER ID                       Re-enable a remembered USB identity
  disconnect SERVER ID                   Disconnect and disable recovery for identity
  detach PORT                           Disconnect a virtual USB port
  refresh                               Discover and show the complete catalog

SERVER is a saved UUID, address or unique name. ID accepts a USB identity or bus ID.
Ports: 1..65535; USB data and discovery/list ports must differ.
--allow replaces the server allowlist. Other settings remain unchanged.
--auth-stdin reads username then password as two UTF-8 lines; credentials are DPAPI protected.
Omitting --auth-stdin removes proxy authentication. No passwords on the command line.
Read commands never change USB ownership. Backend/port changes restart the server;
publication, forgetting unplugged devices, cleanup policy and interface language do not.
Output is JSON except help and delegated transport commands. Errors return a nonzero exit code.
"#
);
fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}
fn default_config(runtime: &Path, role: Role) -> PathBuf {
    let local = runtime.join(if role == Role::Server {
        "host.local.json"
    } else {
        "client.local.json"
    });
    if role == Role::Client && !local.exists() {
        settings::default_client_path()
    } else {
        local
    }
}
fn help_language(args: &[String]) -> io::Result<i18n::Language> {
    if let Some(option) = args.windows(2).find(|pair| pair[0] == "--language") {
        return i18n::Language::parse(&option[1]);
    }
    if let Some(option) = args.windows(2).find(|pair| pair[0] == "--config") {
        return Ok(i18n::preferred(Path::new(&option[1])));
    }
    if let Some(role) = args.iter().find_map(|arg| match arg.as_str() {
        "server" => Some(Role::Server),
        "client" => Some(Role::Client),
        _ => None,
    }) {
        let exe = std::env::current_exe()?;
        return Ok(i18n::preferred(&default_config(
            exe.parent().unwrap(),
            role,
        )));
    }
    Ok(i18n::Language::En)
}
fn help(language: i18n::Language) -> String {
    HELP.lines()
        .map(|line| {
            if line.starts_with("USB to Network Bridge v.") {
                return format!(
                    "{} v.{} — {}",
                    crate::APP_NAME,
                    crate::APP_VERSION,
                    i18n::text_in(language, "command line control")
                );
            }
            if line.starts_with("  ")
                && let Some((_, description)) = line.trim_start().split_once("  ")
            {
                let description = description.trim_start();
                let prefix = &line[..line.len() - description.len()];
                return format!("{prefix}{}", i18n::text_in(language, description));
            }
            i18n::text_in(language, line).to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}
fn print(value: &impl Serialize) -> io::Result<()> {
    println!(
        "{}",
        serde_json::to_string_pretty(value).map_err(io::Error::other)?
    );
    Ok(())
}
fn device_json(row: &crate::host_status::DeviceRow) -> Value {
    let mut value = json!(row);
    value["state"] = json!(match row.state.as_str() {
        "Не подключено к USB" => "unplugged",
        "Не опубликовано" => "not_shared",
        "Сервер выключен" => "server_stopped",
        "Используется клиентом" => "in_use",
        "Доступно" => "available",
        _ => "unknown",
    });
    value["status_text"] = json!(i18n::t(&row.state));
    value
}
fn on_off(value: &str) -> io::Result<bool> {
    match value {
        "on" => Ok(true),
        "off" => Ok(false),
        _ => Err(invalid("expected on or off")),
    }
}
fn port(value: &str) -> io::Result<u16> {
    value
        .parse::<u16>()
        .ok()
        .filter(|p| *p > 0)
        .ok_or_else(|| invalid("port must be in 1..65535"))
}
fn take_options(args: &[String], allowed: &[&str]) -> io::Result<Vec<(String, String)>> {
    if !args.len().is_multiple_of(2) {
        return Err(invalid("each option requires a value"));
    }
    let mut result = Vec::new();
    for pair in args.as_chunks::<2>().0 {
        if !allowed.contains(&pair[0].as_str()) {
            return Err(invalid(format!("unknown option: {}", pair[0])));
        }
        if pair[0] != "--allow" && result.iter().any(|(key, _)| key == &pair[0]) {
            return Err(invalid(format!("duplicate option: {}", pair[0])));
        }
        result.push((pair[0].clone(), pair[1].clone()));
    }
    Ok(result)
}
fn client_config(path: &Path) -> io::Result<ClientSettings> {
    match settings::read::<ClientSettings>(path) {
        Ok(config) => {
            config.validate()?;
            Ok(config)
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(ClientSettings::default()),
        Err(e) => Err(e),
    }
}
fn saved(config: &ClientSettings, query: &str) -> io::Result<SavedServer> {
    let matches: Vec<_> = config
        .connections()
        .into_iter()
        .filter(|s| {
            s.key().eq_ignore_ascii_case(query)
                || s.server.eq_ignore_ascii_case(query)
                || s.name.eq_ignore_ascii_case(query)
        })
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(invalid("saved server not found")),
        _ => Err(invalid("ambiguous server name; use its UUID or address")),
    }
}
fn target(server: &SavedServer, query: &str) -> io::Result<String> {
    let matches: Vec<_> = server
        .devices
        .iter()
        .filter(|d| d.identity.eq_ignore_ascii_case(query) || d.busid == query)
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.identity.clone()),
        [] => Err(invalid("remembered device not found")),
        _ => Err(invalid("ambiguous USB bus ID; use the full identity")),
    }
}
struct RequestFiles {
    request: PathBuf,
    reply: PathBuf,
}
impl Drop for RequestFiles {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.request);
        let _ = std::fs::remove_file(&self.reply);
    }
}
/// All server mutations go through the existing transactional GUI helpers and backend mutex.
pub fn host_request(config: &Path, script: &str, request: &Value) -> io::Result<Value> {
    if ![
        "Publish-Device.ps1",
        "Configure-Server.ps1",
        "Set-ServerState.ps1",
    ]
    .contains(&script)
    {
        return Err(invalid("unknown server action"));
    }
    let host: HostSettings = settings::read(config)?;
    host.validate()?;
    let path = std::env::temp_dir().join(format!(
        "usbbridge-{}.local.request.json",
        uuid::Uuid::new_v4()
    ));
    let files = RequestFiles {
        reply: PathBuf::from(format!("{}.result.json", path.display())),
        request: path,
    };
    settings::save(&files.request, request)?;
    let status = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(host.runtime.join(script))
        .arg("-Config")
        .arg(config)
        .arg("-RequestFile")
        .arg(&files.request)
        .arg("-Elevate")
        .creation_flags(0x08000000)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    let reply: Value = settings::read(&files.reply)
        .map_err(|e| io::Error::other(format!("server helper did not return a result: {e}")))?;
    if !status.success() || reply["Success"].as_bool() != Some(true) {
        return Err(io::Error::other(
            reply["Error"].as_str().unwrap_or("server action failed"),
        ));
    }
    Ok(reply)
}
pub fn run(mut args: Vec<String>) -> io::Result<()> {
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{}", help(help_language(&args)?));
        return Ok(());
    }
    let mut config = None;
    let mut language = None;
    while args.first().is_some_and(|a| a.starts_with("--")) {
        let key = args.remove(0);
        if key == "--json" {
            continue;
        }
        if args.is_empty() {
            return Err(invalid(format!("missing value for {key}")));
        }
        let value = args.remove(0);
        match key.as_str() {
            "--config" if config.is_none() => config = Some(PathBuf::from(value)),
            "--language" if language.is_none() => language = Some(i18n::Language::parse(&value)?),
            _ => return Err(invalid(format!("unknown or duplicate option: {key}"))),
        }
    }
    let role = match args.first().map(String::as_str) {
        Some("server") => Role::Server,
        Some("client") => Role::Client,
        _ => return Err(invalid("expected server or client; see --help")),
    };
    args.remove(0);
    let command = args
        .first()
        .ok_or_else(|| invalid("missing command; see --help"))?
        .clone();
    args.remove(0);
    let runtime = std::env::current_exe()?
        .parent()
        .ok_or_else(|| invalid("executable directory missing"))?
        .to_path_buf();
    let config = config.unwrap_or_else(|| default_config(&runtime, role));
    let config = std::path::absolute(config)?;
    i18n::set_language(language.unwrap_or_else(|| i18n::preferred(&config)));
    crate::diagnostics::init(
        &config,
        if role == Role::Server {
            "server-cli"
        } else {
            "client-cli"
        },
    );
    // Arguments may contain endpoint names, but credential input is never logged.
    crate::diagnostics::event(
        "cli.action",
        json!({"role":if role==Role::Server {"server"}else{"client"},"command":command,"arguments":args}),
    );
    match command.as_str() {
        "local-camera" if role == Role::Client && !args.is_empty() => {
            let file = crate::video::receiver_path(&config);
            match args[0].as_str() {
                "settings" if args.len() == 1 => print(&settings::read::<Value>(&file)?),
                "status" if args.len() == 1 => print(&json!({
                    "receiver":crate::video::status(&file),
                    "sender":crate::video::status(&crate::video::local_sender_path(&config))
                })),
                "configure" if args.len() == 2 => {
                    let value: Value = settings::read(Path::new(&args[1]))?;
                    crate::desktop::local_camera_apply(&config, value)?;
                    print(&json!({"success":true}))
                }
                "on" | "off" if args.len() == 1 => {
                    let mut value: Value = settings::read(&file)?;
                    value["enabled"] = json!(args[0] == "on");
                    crate::desktop::local_camera_apply(&config, value)?;
                    print(&json!({"success":true}))
                }
                _ => Err(invalid(
                    "local-camera requires settings|status|on|off|configure FILE",
                )),
            }
        }
        "video" if !args.is_empty() => {
            let receive = role == Role::Server;
            let file = if receive {
                crate::video::receiver_path(&config)
            } else {
                crate::video::sender_path(&config)
            };
            match args[0].as_str() {
                "settings" if args.len() == 1 => print(&settings::read::<Value>(&file)?),
                "status" if args.len() == 1 => print(&crate::video::status(&file)),
                "configure" if args.len() == 2 => {
                    let value: Value = settings::read(Path::new(&args[1]))?;
                    if receive {
                        serde_json::from_value::<crate::video::ReceiverSettings>(value.clone())
                            .map_err(io::Error::other)?
                            .validate()?;
                    } else {
                        serde_json::from_value::<crate::video::SenderSettings>(value.clone())
                            .map_err(io::Error::other)?
                            .validate()?;
                    }
                    crate::desktop::video_apply(&config, role, value)?;
                    print(&json!({"success":true}))
                }
                "on" | "off" if args.len() == 1 => {
                    let mut value: Value = settings::read(&file)?;
                    value["enabled"] = json!(args[0] == "on");
                    crate::desktop::video_apply(&config, role, value)?;
                    print(&json!({"success":true}))
                }
                _ => Err(invalid(
                    "video requires settings|status|on|off|configure FILE",
                )),
            }
        }
        "cameras" if args.is_empty() => print(&crate::camera::list()?),
        "preview" if args.len() <= 1 => {
            crate::camera::launch(
                &config,
                role,
                args.first().map(String::as_str),
                !args.is_empty(),
            )?;
            print(&json!({"success":true}))
        }
        "camera-modes" if args.len() == 1 => {
            let cameras: Vec<_> = crate::camera::list()?
                .into_iter()
                .filter(|camera| camera.matches(&args[0]))
                .collect();
            let [camera] = cameras.as_slice() else {
                return Err(invalid(
                    "camera not found or ambiguous; use its exact path from cameras",
                ));
            };
            print(&crate::camera::modes(&camera.path)?)
        }
        "camera-test" if !args.is_empty() => {
            let options = take_options(
                &args[1..],
                &["--mode", "--seconds", "--low-light", "--stop-file"],
            )?;
            let mut mode = None;
            let mut seconds = 10;
            let mut low_light = false;
            let mut stop_file = None;
            for (key, value) in options {
                if key == "--mode" {
                    mode = Some(
                        value
                            .parse()
                            .map_err(|_| invalid("invalid camera mode index"))?,
                    );
                } else if key == "--stop-file" {
                    stop_file = Some(PathBuf::from(value));
                } else if key == "--low-light" {
                    low_light = on_off(&value)?;
                } else {
                    seconds = value
                        .parse()
                        .map_err(|_| invalid("invalid camera test duration"))?;
                }
            }
            crate::camera::benchmark(&args[0], mode, seconds, low_light, stop_file.as_deref())
        }
        "about" if args.is_empty() => print(
            &json!({"name":crate::APP_NAME,"version":crate::APP_VERSION,"repository":crate::REPOSITORY}),
        ),
        "language" if args.len() <= 1 => {
            if let Some(value) = args.first() {
                crate::tray::change_language(&config, role, i18n::Language::parse(value)?)?;
            }
            print(&json!({"language":i18n::preferred(&config).code()}))
        }
        "health" if args.is_empty() => print(&crate::supervisor::status(&config, role)?),
        "logs" if args.is_empty() => print(
            &json!({"directory":config.parent().unwrap_or(Path::new(".")).join("logs"),"controller":config.with_extension("agent.log")}),
        ),
        "gui" if args.is_empty() => {
            Command::new(runtime.join("usbbridge-agent.exe"))
                .args([
                    "--gui",
                    "--role",
                    if role == Role::Server {
                        "server"
                    } else {
                        "client"
                    },
                    "--config",
                ])
                .arg(&config)
                .creation_flags(0x08000000)
                .spawn()?;
            print(&json!({"success":true}))
        }
        "hide" | "close-ui" if args.is_empty() => print(
            &json!({"success":true,"found":crate::tray::control_interface(role,command=="close-ui")?}),
        ),
        "autostart" if args.len() == 1 => autostart(&config, role, &args[0]),
        _ if role == Role::Server => server(&config, &command, &args),
        _ => client(&runtime, &config, &command, &args),
    }
}
fn server(path: &Path, command: &str, args: &[String]) -> io::Result<()> {
    let host: HostSettings = settings::read(path)?;
    host.validate()?;
    match command {
        "status" | "devices" | "refresh" if args.is_empty() => {
            let status = crate::host_status::collect(path)?;
            print(
                &json!({"enabled":status.enabled,"running":status.running,"backend":status.backend,"tcp_port":status.tcp_port,"discovery_port":status.discovery_port,"devices":status.rows.iter().map(device_json).collect::<Vec<_>>(),"warning":status.warning}),
            )
        }
        "start" | "stop" if args.is_empty() => print(&host_request(
            path,
            "Set-ServerState.ps1",
            &json!({"enabled":command=="start"}),
        )?),
        "publish" | "local" if args.len() == 2 => {
            let enabled = on_off(&args[1])?;
            let status = crate::host_status::collect(path)?;
            let rows: Vec<_> = status
                .rows
                .iter()
                .filter(|d| d.identity.eq_ignore_ascii_case(&args[0]) || d.busid == args[0])
                .collect();
            let row = match rows.as_slice() {
                [one] => *one,
                _ => {
                    return Err(invalid(
                        "device not found or bus ID ambiguous; use its full identity",
                    ));
                }
            };
            let request = if command == "local" {
                if status.backend != "Native" {
                    return Err(invalid("local use when idle requires Native"));
                }
                json!({"identity":row.identity,"publish":row.published,"local_when_idle":enabled})
            } else {
                json!({"identity":row.identity,"publish":enabled})
            };
            print(&host_request(path, "Publish-Device.ps1", &request)?)
        }
        "settings" if args.is_empty() => print(&host),
        "forget" if args.len() == 1 => {
            let status = crate::host_status::collect(path)?;
            if !status.presence_verified {
                return Err(invalid("Cannot verify physical USB presence"));
            }
            let rows: Vec<_> = status
                .rows
                .iter()
                .filter(|d| d.identity.eq_ignore_ascii_case(&args[0]) || d.busid == args[0])
                .collect();
            let [row] = rows.as_slice() else {
                return Err(invalid("device not found or bus ID ambiguous"));
            };
            if row.present {
                return Err(invalid(
                    "The device is connected to USB. Refresh the device list.",
                ));
            }
            print(&host_request(
                path,
                "Publish-Device.ps1",
                &json!({"identity":row.identity,"publish":false,"forget":true}),
            )?)
        }
        "settings" => {
            let options = take_options(
                args,
                &[
                    "--name",
                    "--tcp-port",
                    "--discovery-port",
                    "--backend",
                    "--allow",
                    "--auto-remove-unplugged",
                    "--unplugged-days",
                ],
            )?;
            let state: Value = settings::read(&host.runtime.join("backend-state.json"))?;
            print(&host_request(
                path,
                "Configure-Server.ps1",
                &server_request(&host, &state, &options)?,
            )?)
        }
        _ => Err(invalid(
            "unknown server command or invalid arguments; see --help",
        )),
    }
}
fn server_request(
    host: &HostSettings,
    state: &Value,
    options: &[(String, String)],
) -> io::Result<Value> {
    let mut name = host.name.clone();
    let mut tcp = settings::endpoint(&host.probe_address)?.1;
    let mut discovery = host.discovery_port;
    let mut backend = state["DesiredBackend"]
        .as_str()
        .or_else(|| state["Backend"].as_str())
        .unwrap_or("Native")
        .to_string();
    let mut allowed = Vec::new();
    let mut auto_remove = host.auto_remove_unplugged;
    let mut days = host.unplugged_retention_days;
    for (key, value) in options {
        match key.as_str() {
            "--auto-remove-unplugged" => auto_remove = on_off(value)?,
            "--unplugged-days" => {
                days = value
                    .parse::<u32>()
                    .ok()
                    .filter(|d| (1..=36500).contains(d))
                    .ok_or_else(|| invalid("days must be in 1..36500"))?
            }
            "--name" => name = value.trim().into(),
            "--tcp-port" => tcp = port(value)?,
            "--discovery-port" => discovery = port(value)?,
            "--backend" => {
                backend = match value.to_ascii_lowercase().as_str() {
                    "native" => "Native",
                    "usbipd" => "Usbipd",
                    _ => return Err(invalid("backend must be Native or Usbipd")),
                }
                .into()
            }
            "--allow" => allowed.push(
                value
                    .parse::<std::net::IpAddr>()
                    .map_err(|_| invalid("invalid allowed client IP"))?,
            ),
            _ => return Err(invalid("unknown server setting")),
        }
    }
    if name.is_empty() || name.len() > 128 || tcp == discovery {
        return Err(invalid(
            "server name must contain 1..128 bytes; USB and discovery ports must differ",
        ));
    }
    if allowed.is_empty() {
        allowed = host.allowed_ips.clone();
    }
    Ok(
        json!({"name":name,"tcp_port":tcp,"discovery_port":discovery,"allowed_ips":allowed,"backend":backend,"auto_remove_unplugged":auto_remove,"unplugged_retention_days":days}),
    )
}
fn client(runtime: &Path, path: &Path, command: &str, args: &[String]) -> io::Result<()> {
    let config = client_config(path)?;
    match command {
        "servers" if args.is_empty() => print(
            &json!({"selected":config.connections().first().map(SavedServer::key),"servers":config.connections()}),
        ),
        "settings" if args.is_empty() => print(&config),
        "settings" => {
            let options = take_options(
                args,
                &[
                    "--discovery",
                    "--discovery-port",
                    "--poll-seconds",
                    "--retry-max-seconds",
                ],
            )?;
            settings::edit_client(path, |config| {
                for (key, value) in &options {
                    match key.as_str() {
                        "--discovery" => config.discovery = on_off(value)?,
                        "--discovery-port" => config.discovery_port = port(value)?,
                        "--poll-seconds" => {
                            config.poll_seconds = value
                                .parse()
                                .map_err(|_| invalid("invalid poll interval"))?
                        }
                        "--retry-max-seconds" => {
                            config.retry_max_seconds = value
                                .parse()
                                .map_err(|_| invalid("invalid retry interval"))?
                        }
                        _ => unreachable!(),
                    }
                }
                Ok(())
            })?;
            print(&client_config(path)?)
        }
        "add" if !args.is_empty() => {
            let address = settings::normalize_endpoint(&args[0], 3240)?;
            let options = take_options(&args[1..], &["--name", "--id"])?;
            let mut selected = SavedServer {
                paused: false,
                server: address,
                proxy: None,
                server_id: None,
                name: String::new(),
                devices: Vec::new(),
            };
            for (key, value) in options {
                if key == "--id" {
                    uuid::Uuid::parse_str(&value).map_err(|_| invalid("invalid server UUID"))?;
                    selected.server_id = Some(value);
                } else {
                    selected.name = value;
                }
            }
            settings::edit_client(path, |c| {
                c.select_server(selected);
                Ok(())
            })?;
            print(&client_config(path)?)
        }
        "select" if args.len() == 1 => {
            settings::edit_client(path, |c| {
                let selected = saved(c, &args[0])?;
                c.select_server(selected);
                Ok(())
            })?;
            print(&client_config(path)?)
        }
        "pause" | "resume" if args.len() == 1 => {
            let server = saved(&config, &args[0])?;
            agent::set_server_paused(path, &server, command == "pause")?;
            print(&json!({"success":true,"server":server.key(),"paused":command=="pause"}))
        }
        "address" if args.len() == 2 => {
            let address = settings::normalize_endpoint(&args[1], 3240)?;
            settings::edit_client(path, |c| {
                let server = saved(c, &args[0])?;
                c.update_connection(&server.key(), |s| s.server = address);
                Ok(())
            })?;
            print(&client_config(path)?)
        }
        "remove" if args.len() == 1 => {
            let server = saved(&config, &args[0])?;
            for device in server.devices.iter().filter(|d| d.enabled) {
                agent::release_target(path, &server, &device.identity)?;
            }
            settings::edit_client(path, |c| {
                c.remove_server(&server.key());
                Ok(())
            })?;
            print(&client_config(path)?)
        }
        "proxy" => set_proxy(path, &config, args),
        "disconnect" | "enable" if args.len() == 2 => {
            let server = saved(&config, &args[0])?;
            let identity = target(&server, &args[1])?;
            if command == "disconnect" {
                agent::release_target(path, &server, &identity)?;
            } else {
                settings::edit_client(path, |c| {
                    if !c.update_connection(&server.key(), |s| {
                        s.paused = false;
                        if let Some(d) = s
                            .devices
                            .iter_mut()
                            .find(|d| d.identity.eq_ignore_ascii_case(&identity))
                        {
                            d.enabled = true;
                        }
                    }) {
                        return Err(invalid("server no longer saved"));
                    }
                    Ok(())
                })?;
                agent::start_client(path)?;
            }
            print(&json!({"success":true,"identity":identity,"enabled":command=="enable"}))
        }
        "devices" | "refresh" if args.is_empty() => catalog(&config),
        "discover" | "status" | "ports" if args.is_empty() => {
            delegate(runtime, path, command, args, &config)
        }
        "list" if args.len() == 1 => delegate(runtime, path, command, args, &config),
        "connect" if args.len() == 2 => delegate(runtime, path, command, args, &config),
        "connect" if args.len() == 1 => {
            let server =
                config.connections().into_iter().next().ok_or_else(|| {
                    invalid("select or add a server first, or specify its address")
                })?;
            delegate(
                runtime,
                path,
                command,
                &[server.server, args[0].clone()],
                &config,
            )
        }
        "detach" if args.len() == 1 => delegate(runtime, path, command, args, &config),
        _ => Err(invalid(
            "unknown client command or invalid arguments; see --help",
        )),
    }
}
fn delegate(
    runtime: &Path,
    path: &Path,
    command: &str,
    args: &[String],
    config: &ClientSettings,
) -> io::Result<()> {
    let mut args = args.to_vec();
    if matches!(command, "connect" | "list")
        && let Some(query) = args.first()
    {
        let matches: Vec<_> = config
            .connections()
            .into_iter()
            .filter(|s| {
                s.key().eq_ignore_ascii_case(query)
                    || s.name.eq_ignore_ascii_case(query)
                    || s.server.eq_ignore_ascii_case(query)
            })
            .collect();
        match matches.as_slice() {
            [one] => args[0] = one.server.clone(),
            [] => {}
            _ => return Err(invalid("ambiguous server name; use its UUID or address")),
        }
    }
    let status = Command::new(runtime.join("usbbridge-client.exe"))
        .arg(command)
        .args(&args)
        .arg("--config")
        .arg(path)
        .arg("--discovery-port")
        .arg(config.discovery_port.to_string())
        .arg("--json")
        .status()?;
    if !status.success() {
        return Err(io::Error::other(format!("client command failed: {status}")));
    }
    Ok(())
}
fn catalog(config: &ClientSettings) -> io::Result<()> {
    let targets: Vec<_> = config
        .connections()
        .iter()
        .filter(|s| s.proxy.is_none())
        .flat_map(|s| agent::lookup_endpoint(&s.server, config.discovery_port).unwrap_or_default())
        .collect();
    let found = if config.discovery {
        crate::discovery::find(
            config.discovery_port,
            &targets,
            std::time::Duration::from_secs(2),
        )?
    } else {
        Vec::new()
    };
    let mut output = Vec::new();
    for server in crate::catalog::merge(config, found) {
        let status = crate::management::fetch(&server.saved, config.discovery_port);
        output.push(match status {
            Ok(catalog) => {
                let mut value = json!(catalog);
                value["devices"] =
                    json!(catalog.devices.iter().map(device_json).collect::<Vec<_>>());
                json!({"server":server.saved,"catalog":value})
            }
            Err(e) => json!({"server":server.saved,"error":e.to_string()}),
        });
    }
    print(&output)
}
fn set_proxy(path: &Path, config: &ClientSettings, args: &[String]) -> io::Result<()> {
    if args.len() < 2 {
        return Err(invalid(
            "proxy requires SERVER direct or SERVER KIND HOST:PORT [--auth-stdin]",
        ));
    }
    let server = saved(config, &args[0])?;
    let mut created = None;
    let proxy = match args[1].as_str() {
        "direct" if args.len() == 2 => None,
        "http" | "https" | "socks5"
            if args.len() == 3 || (args.len() == 4 && args[3] == "--auth-stdin") =>
        {
            let mut proxy = crate::proxy::Settings {
                kind: match args[1].as_str() {
                    "http" => crate::proxy::Kind::Http,
                    "https" => crate::proxy::Kind::Https,
                    _ => crate::proxy::Kind::Socks5,
                },
                address: args[2].clone(),
                credential_file: None,
            };
            proxy.validate()?;
            if args.len() == 4 {
                let input = io::stdin();
                let mut input = input.lock();
                let mut username = String::new();
                let mut password = String::new();
                input.read_line(&mut username)?;
                input.read_line(&mut password)?;
                let auth = crate::proxy::Credentials {
                    username: username.trim_end_matches(['\r', '\n']).into(),
                    password: password.trim_end_matches(['\r', '\n']).into(),
                };
                if auth.username.is_empty()
                    || auth.password.is_empty()
                    || auth.username.len() > 255
                    || auth.password.len() > 255
                {
                    return Err(invalid(
                        "provide a non-empty username and password, at most 255 bytes each, on two stdin lines",
                    ));
                }
                let file = crate::credentials::save(&auth)?;
                created = Some(file.clone());
                proxy.credential_file = Some(file);
            }
            Some(proxy)
        }
        _ => return Err(invalid("invalid proxy arguments; see --help")),
    };
    let result = settings::edit_client(path, |c| {
        if !c.update_connection(&server.key(), |s| s.proxy = proxy) {
            return Err(invalid("server no longer saved"));
        }
        Ok(())
    });
    if result.is_err()
        && let Some(file) = created
    {
        let _ = std::fs::remove_file(file);
    }
    result?;
    agent::start_client(path)?;
    print(&json!({"success":true}))
}
fn autostart(path: &Path, role: Role, action: &str) -> io::Result<()> {
    let host = if role == Role::Server {
        Some(settings::read::<HostSettings>(path)?)
    } else {
        None
    };
    let runtime = if let Some(host) = &host {
        host.runtime.clone()
    } else {
        std::env::current_exe()?.parent().unwrap().to_path_buf()
    };
    let role = if role == Role::Server {
        "Server"
    } else {
        "Client"
    };
    if action == "status" {
        let output=Command::new("powershell.exe").args(["-NoProfile","-NonInteractive","-Command"])
            .arg(r"$name='USBBridge.'+$env:USB_BRIDGE_ROLE; $tasks=@(Get-ScheduledTask -TaskPath '\USBBridge\' -ErrorAction SilentlyContinue|Where-Object {$_.TaskName-eq $name-or $_.TaskName-like ('USBBridge.Tray.'+$env:USB_BRIDGE_ROLE+'.*')}); ConvertTo-Json -InputObject @($tasks|ForEach-Object {[pscustomobject]@{name=$_.TaskName;state=[string]$_.State;actions=@($_.Actions|Select-Object Execute,Arguments,WorkingDirectory)}}) -Depth 5")
            .env("USB_BRIDGE_ROLE",role).creation_flags(0x08000000).output()?;
        if !output.status.success() {
            return Err(io::Error::other("cannot inspect autostart tasks"));
        }
        println!("{}", String::from_utf8_lossy(&output.stdout).trim());
        return Ok(());
    }
    let enabled = on_off(action)?;
    let mut command = Command::new("powershell.exe");
    command
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(runtime.join("Set-Autostart.ps1"))
        .args(["-Role", role, "-Config"])
        .arg(path)
        .arg("-Elevate")
        .creation_flags(0x08000000);
    if let Some(host) = host {
        command.arg("-Profile").arg(host.profile);
    }
    if !enabled {
        command.arg("-Disable");
    }
    if !command.status()?.success() {
        return Err(io::Error::other("autostart configuration failed"));
    }
    print(&json!({"success":true,"enabled":enabled}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_settings_do_not_write_partial_configurations() {
        for value in ["0", "65536", "-1", "abc"] {
            assert!(port(value).is_err());
        }
        assert_eq!(port("65535").unwrap(), 65535);
        assert!(take_options(&["--tcp-port".into()], &["--tcp-port"]).is_err());
        assert!(take_options(&["--typo".into(), "1".into()], &["--tcp-port"]).is_err());
        assert!(
            take_options(
                &[
                    "--tcp-port".into(),
                    "1".into(),
                    "--tcp-port".into(),
                    "2".into()
                ],
                &["--tcp-port"]
            )
            .is_err()
        );
        let host = HostSettings {
            auto_remove_unplugged: true,
            unplugged_retention_days: 1,
            version: 1,
            enabled: true,
            server_id: uuid::Uuid::new_v4().to_string(),
            name: "Server".into(),
            runtime: PathBuf::from("C:\\runtime"),
            profile: PathBuf::from("C:\\runtime\\server.psd1"),
            allowed_ips: vec!["192.0.2.1".parse().unwrap()],
            probe_address: "127.0.0.1:3240".into(),
            discovery_port: 3241,
        };
        assert!(
            server_request(&host, &json!({}), &[("--tcp-port".into(), "3241".into())]).is_err()
        );
        let request = server_request(
            &host,
            &json!({"DesiredBackend":"Usbipd"}),
            &[("--name".into(), "New server".into())],
        )
        .unwrap();
        assert_eq!(request["tcp_port"], 3240);
        assert_eq!(request["backend"], "Usbipd");
        assert_eq!(request["allowed_ips"], json!(["192.0.2.1"]));
    }
    #[test]
    fn server_names_must_be_unambiguous_and_usb_identity_survives_port_changes() {
        let mut config = ClientSettings::default();
        config.select_server(SavedServer {
            paused: false,
            server: "192.0.2.1:3240".into(),
            proxy: None,
            server_id: None,
            name: "Desk".into(),
            devices: Vec::new(),
        });
        config.select_server(SavedServer {
            paused: false,
            server: "192.0.2.2:3240".into(),
            proxy: None,
            server_id: None,
            name: "Desk".into(),
            devices: vec![crate::settings::Target {
                identity: "USB\\VID_1234&PID_5678\\SERIAL".into(),
                vendor: 0x1234,
                product: 0x5678,
                busid: "2-8".into(),
                enabled: true,
                low_latency: false,
            }],
        });
        assert!(saved(&config, "Desk").is_err());
        let server = saved(&config, "192.0.2.2:3240").unwrap();
        assert_eq!(
            target(&server, "2-8").unwrap(),
            target(&server, "USB\\VID_1234&PID_5678\\SERIAL").unwrap()
        );
    }
}
