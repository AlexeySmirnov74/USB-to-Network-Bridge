//! Independent local USB/UVC endpoint; never captures a physical USB device.
use std::{
    io,
    net::{IpAddr, SocketAddr, TcpListener},
    path::PathBuf,
    sync::Arc,
};
use usbbridge::{
    server::Server,
    uvc::{self, Config, Frames, UvcProvider},
};
fn main() {
    if let Err(error) = run() {
        eprintln!("usbbridge-camera: {error}");
        std::process::exit(1);
    }
}
fn run() -> io::Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    #[cfg(windows)]
    if matches!(
        command.as_str(),
        "receive" | "send" | "check-receive" | "check-send" | "check-local-send"
    ) {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetConsoleProcessList(processes: *mut u32, count: u32) -> u32;
            fn FreeConsole() -> i32;
        }
        // Task Scheduler can create a private console for an interactive worker.
        // Preserve a user's existing terminal when invoked from the command line.
        let mut processes = [0u32; 2];
        if unsafe { GetConsoleProcessList(processes.as_mut_ptr(), 2) } == 1 {
            unsafe {
                FreeConsole();
            }
        }
    }

    if matches!(
        command.as_str(),
        "check-receive" | "check-send" | "check-local-send"
    ) {
        if args.next().as_deref() != Some("--config") {
            return Err(io::Error::other("video health requires --config FILE"));
        }
        let config = PathBuf::from(
            args.next()
                .ok_or_else(|| io::Error::other("missing video config"))?,
        );
        if args.next().is_some() {
            return Err(io::Error::other("unexpected health arguments"));
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            if usbbridge::lifecycle::video_exit_requested(&config)? {
                return Ok(());
            }
            let role = if command == "check-receive" {
                "Receive"
            } else if command == "check-local-send" {
                "LocalSend"
            } else {
                "Send"
            };
            usbbridge::diagnostics::init(
                &config,
                &format!("video-{}-health", role.to_ascii_lowercase()),
            );
            let output = std::process::Command::new("powershell.exe")
                .args([
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                ])
                .arg(std::env::current_exe()?.with_file_name("Restart-VideoEndpoint.ps1"))
                .args(["-Role", role, "-Config"])
                .arg(&config)
                .creation_flags(0x08000000)
                .output()?;
            usbbridge::diagnostics::event(
                "video.health.result",
                serde_json::json!({"success":output.status.success(),"result":String::from_utf8_lossy(&output.stdout),"error":String::from_utf8_lossy(&output.stderr)}),
            );
            if !output.status.success() {
                return Err(io::Error::other(
                    "video health recovery failed; see component log",
                ));
            }
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            return Err(io::Error::other("Windows is required"));
        }
    }
    if command == "driver-ready" {
        #[cfg(windows)]
        {
            usbbridge::vhci::imported_devices()?;
            println!("{{\"ready\":true}}");
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            return Err(io::Error::other("Windows is required"));
        }
    }
    if command == "validate" {
        let role = args
            .next()
            .ok_or_else(|| io::Error::other("validate requires receive|send FILE"))?;
        let path = PathBuf::from(
            args.next()
                .ok_or_else(|| io::Error::other("validate requires settings FILE"))?,
        );
        if args.next().is_some() {
            return Err(io::Error::other("unexpected validation arguments"));
        }
        if role == "receive" {
            let value: usbbridge::video::ReceiverSettings = usbbridge::settings::read(&path)?;
            value.validate()?;
        } else if role == "send" {
            let value: usbbridge::video::SenderSettings = usbbridge::settings::read(&path)?;
            value.validate()?;
        } else {
            return Err(io::Error::other("unknown video role"));
        }
        println!("{{\"valid\":true}}");
        return Ok(());
    }
    if command == "detach-local" {
        let endpoint = args
            .next()
            .ok_or_else(|| io::Error::other("detach-local requires 127.0.0.1:PORT"))?;
        let socket: SocketAddr = endpoint.parse().map_err(io::Error::other)?;
        if !socket.ip().is_loopback() || socket.port() == 0 || args.next().is_some() {
            return Err(io::Error::other("invalid local camera endpoint"));
        }
        #[cfg(windows)]
        {
            for device in usbbridge::vhci::imported_devices()?
                .into_iter()
                .filter(|d| d.busid == uvc::BUSID && d.server == endpoint)
            {
                usbbridge::vhci::detach(device.port)?;
            }
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            return Err(io::Error::other("Windows is required"));
        }
    }
    if command == "keygen" {
        let path = PathBuf::from(
            args.next()
                .ok_or_else(|| io::Error::other("keygen requires a new .key file"))?,
        );
        if path.extension().is_none_or(|e| e != "key") || args.next().is_some() {
            return Err(io::Error::other("keygen requires a new .key file"));
        }
        let secret = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        usbbridge::credentials::save_key(&path, &secret)?;
        println!("{}", serde_json::json!({"created":path}));
        return Ok(());
    }
    if command == "receive" || command == "send" {
        if args.next().as_deref() != Some("--config") {
            return Err(io::Error::other("receive/send requires --config FILE"));
        }
        let path = PathBuf::from(
            args.next()
                .ok_or_else(|| io::Error::other("missing video settings file"))?,
        );
        if args.next().is_some() {
            return Err(io::Error::other("unexpected video arguments"));
        }
        #[cfg(windows)]
        {
            if usbbridge::lifecycle::video_exit_requested(&path)? {
                return Ok(());
            }
            return if command == "receive" {
                usbbridge::video::receive(&usbbridge::settings::read(&path)?, &path)
            } else {
                usbbridge::video::send(&usbbridge::settings::read(&path)?, &path)
            };
        }
        #[cfg(not(windows))]
        {
            return Err(io::Error::other("Windows is required"));
        }
    }
    if command == "attach" {
        let address = args
            .next()
            .ok_or_else(|| io::Error::other("attach requires HOST:PORT"))?;
        #[cfg(windows)]
        {
            let port = usbbridge::vhci::attach(&address, uvc::BUSID, false)?;
            println!(
                "{}",
                serde_json::json!({"port":port,"server":address,"busid":uvc::BUSID})
            );
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            return Err(io::Error::other("Windows USBIP-WIN2 is required"));
        }
    }
    if command == "detach" {
        let port: i32 = args
            .next()
            .ok_or_else(|| io::Error::other("detach requires PORT"))?
            .parse()
            .map_err(io::Error::other)?;
        #[cfg(windows)]
        {
            let device = usbbridge::vhci::imported_devices()?
                .into_iter()
                .find(|d| d.port == port && d.busid == uvc::BUSID)
                .ok_or_else(|| {
                    io::Error::other("refusing to detach a device not owned by the UVC endpoint")
                })?;
            usbbridge::vhci::detach(device.port)?;
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            return Err(io::Error::other("Windows USBIP-WIN2 is required"));
        }
    }
    if command != "serve" {
        println!(
            "USB to Network Bridge v.{} - USB/UVC camera\nusbbridge-camera serve [--name NAME] [--serial ID] [--width N] [--height N] [--fps N]\n  [--listen IP:PORT] [--allow-ip IP] [--no-test-pattern] [--microphone] [--status FILE]\nusbbridge-camera attach HOST:PORT\nusbbridge-camera detach PORT\nusbbridge-camera receive|send --config FILE\nusbbridge-camera keygen NEW.key",
            usbbridge::APP_VERSION
        );
        return Ok(());
    }
    let mut config = Config::default();
    let mut listen: SocketAddr = "127.0.0.1:13250".parse().unwrap();
    let mut allowed = Vec::<IpAddr>::new();
    let mut status = PathBuf::from("uvc.local.json");
    while let Some(key) = args.next() {
        if key == "--no-test-pattern" {
            config.test_pattern = false;
            continue;
        }
        if key == "--microphone" {
            config.microphone = true;
            continue;
        }
        let value = args
            .next()
            .ok_or_else(|| io::Error::other(format!("{key} requires a value")))?;
        match key.as_str() {
            "--name" => config.name = value,
            "--serial" => config.serial = value,
            "--width" => config.width = value.parse().map_err(io::Error::other)?,
            "--height" => config.height = value.parse().map_err(io::Error::other)?,
            "--fps" => config.fps = value.parse().map_err(io::Error::other)?,
            "--listen" => listen = value.parse().map_err(io::Error::other)?,
            "--allow-ip" => allowed.push(value.parse().map_err(io::Error::other)?),
            "--status" => status = value.into(),
            _ => return Err(io::Error::other(format!("unknown option {key}"))),
        }
    }
    config.validate()?;
    if listen.port() == 0 || (!listen.ip().is_loopback() && allowed.is_empty()) {
        return Err(io::Error::other(
            "non-loopback camera USB listeners need --allow-ip; port must be 1..65535",
        ));
    }
    usbbridge::diagnostics::init(&status, "uvc-camera");
    let frames = Frames::new(config.clone())?;
    let listener = TcpListener::bind(listen)?;
    usbbridge::diagnostics::event(
        "uvc.start",
        serde_json::json!({"config":config,"listen":listen}),
    );
    usbbridge::settings::save(
        &status,
        &serde_json::json!({"process_id":std::process::id(),"version":usbbridge::APP_VERSION,"config":config,"listen":listen,"busid":uvc::BUSID}),
    )?;
    println!(
        "{}",
        serde_json::json!({"name":config.name,"listen":listen,"busid":uvc::BUSID,"mode":format!("{}x{} MJPEG {} FPS",config.width,config.height,config.fps)})
    );
    Arc::new(Server::new(
        Arc::new(UvcProvider::new(frames)),
        vec![uvc::BUSID.into()],
        allowed,
    ))
    .serve(listener)
}
