#![cfg(windows)]
use serde_json::{Value, json};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("usbbridge-cli-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn config(&self) -> PathBuf {
        self.0.join("client.local.json")
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_usbbridge"))
            .arg("--config")
            .arg(self.config())
            .args(args)
            .output()
            .unwrap()
    }
    fn ok(&self, args: &[&str]) -> Value {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn console_edits_preserve_other_servers_and_device_preferences_and_reject_invalid_ports_atomically()
{
    let fixture = Fixture::new();
    fixture.ok(&["client", "add", "192.0.2.1:3240", "--name", "First"]);
    let mut config: Value = serde_json::from_slice(&fs::read(fixture.config()).unwrap()).unwrap();
    config["devices"] = json!([{"identity":"USB\\VID_1234&PID_5678\\SERIAL","vendor":4660,"product":22136,"busid":"2-8","enabled":true,"low_latency":false}]);
    fs::write(fixture.config(), serde_json::to_vec(&config).unwrap()).unwrap();
    fixture.ok(&["client", "add", "192.0.2.2:65535", "--name", "第二台服务器"]);
    fixture.ok(&["client", "select", "First"]);
    fixture.ok(&[
        "client",
        "settings",
        "--discovery-port",
        "65535",
        "--discovery",
        "off",
    ]);
    let config = fixture.ok(&["client", "settings"]);
    assert_eq!(config["server"], "192.0.2.1:3240");
    assert_eq!(
        config["devices"][0]["identity"],
        "USB\\VID_1234&PID_5678\\SERIAL"
    );
    assert_eq!(config["servers"][0]["name"], "第二台服务器");
    assert_eq!(config["servers"][0]["server"], "192.0.2.2:65535");
    let before = fs::read(fixture.config()).unwrap();
    for args in [
        vec!["client", "settings", "--discovery-port", "65536"],
        vec!["client", "settings", "--discovery-port", "0"],
        vec!["client", "settings", "--poll-seconds", "0"],
        vec!["client", "settings", "--unknown", "value"],
        vec!["client", "proxy", "First", "socks5", "host:65536"],
    ] {
        assert!(!fixture.run(&args).status.success());
        assert_eq!(fs::read(fixture.config()).unwrap(), before);
    }
    fixture.ok(&["client", "address", "192.0.2.2:65535", "[2001:db8::1]:3240"]);
    let servers = fixture.ok(&["client", "servers"]);
    assert_eq!(servers["servers"][1]["server"], "[2001:db8::1]:3240");
}
#[test]
fn language_is_english_without_preferences_and_saved_independently_from_usb_settings() {
    let fixture = Fixture::new();
    assert_eq!(fixture.ok(&["client", "language"])["language"], "en");
    assert!(!fixture.config().exists());
    for language in ["de", "fr", "zh", "ru", "en"] {
        assert_eq!(
            fixture.ok(&["client", "language", language])["language"],
            language
        );
        assert_eq!(fixture.ok(&["client", "language"])["language"], language);
    }
    assert!(
        !fixture
            .run(&["client", "language", "invalid"])
            .status
            .success()
    );
    assert_eq!(fixture.ok(&["client", "language"])["language"], "en");
    assert!(!fixture.config().exists());
}

#[test]
fn help_is_localized_without_translating_command_syntax_or_saving_an_override() {
    let fixture = Fixture::new();
    for (language, title) in [
        ("en", "command line control"),
        ("ru", "управление из командной строки"),
        ("de", "Befehlszeilensteuerung"),
        ("fr", "contrôle en ligne de commande"),
        ("zh", "命令行控制"),
    ] {
        let output = fixture.run(&["--language", language, "--help"]);
        assert!(output.status.success());
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(title));
        assert!(text.contains("publish ID on|off"));
        assert!(text.contains("--auth-stdin"));
    }
    assert!(!fixture.config().with_extension("preferences.json").exists());
}

#[test]
fn server_console_uses_transactional_helper_and_propagates_failure_without_leaking_requests() {
    let fixture = Fixture::new();
    let host = usbbridge::settings::HostSettings {
        auto_remove_unplugged: true,
        unplugged_retention_days: 1,
        version: 1,
        enabled: true,
        server_id: uuid::Uuid::new_v4().to_string(),
        name: "Before".into(),
        runtime: fixture.0.clone(),
        profile: fixture.0.join("server.psd1"),
        allowed_ips: vec!["192.0.2.11".parse().unwrap()],
        probe_address: "127.0.0.1:3240".into(),
        discovery_port: 3241,
    };
    usbbridge::settings::save(&fixture.config(), &host).unwrap();
    fs::write(
        fixture.0.join("backend-state.json"),
        br#"{"DesiredBackend":"Native"}"#,
    )
    .unwrap();
    let helper = fixture.0.join("Configure-Server.ps1");
    fs::write(&helper,r#"param([string]$Config,[string]$RequestFile,[switch]$Elevate)
$ErrorActionPreference='Stop'
Copy-Item -LiteralPath $RequestFile -Destination (Join-Path $PSScriptRoot 'last-request.json') -Force
[IO.File]::WriteAllText(($RequestFile+'.result.json'),'{"Success":true}')
"#).unwrap();
    assert_eq!(
        fixture.ok(&[
            "server",
            "settings",
            "--name",
            "After",
            "--tcp-port",
            "65535"
        ])["Success"],
        true
    );
    let request: Value =
        serde_json::from_slice(&fs::read(fixture.0.join("last-request.json")).unwrap()).unwrap();
    assert_eq!(request["name"], "After");
    assert_eq!(request["tcp_port"], 65535);
    assert_eq!(request["discovery_port"], 3241);
    assert_eq!(request["allowed_ips"], json!(["192.0.2.11"]));
    assert_eq!(request["backend"], "Native");
    assert_eq!(request["auto_remove_unplugged"], true);
    assert_eq!(request["unplugged_retention_days"], 1);
    assert_eq!(
        fixture.ok(&[
            "server",
            "settings",
            "--auto-remove-unplugged",
            "off",
            "--unplugged-days",
            "7"
        ])["Success"],
        true
    );
    let policy: Value =
        serde_json::from_slice(&fs::read(fixture.0.join("last-request.json")).unwrap()).unwrap();
    assert_eq!(policy["auto_remove_unplugged"], false);
    assert_eq!(policy["unplugged_retention_days"], 7);
    for days in ["0", "36501", "-1", "1.5", "4294967296"] {
        assert!(
            !fixture
                .run(&["server", "settings", "--unplugged-days", days])
                .status
                .success()
        );
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(fixture.0.join("last-request.json")).unwrap()
            )
            .unwrap(),
            policy
        );
    }
    fs::write(&helper,r#"param([string]$Config,[string]$RequestFile,[switch]$Elevate)
[IO.File]::WriteAllText(($RequestFile+'.result.json'),'{"Success":false,"Error":"fixture denied action"}')
exit 1
"#).unwrap();
    let output = fixture.run(&["server", "settings", "--name", "Fail"]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("fixture denied action"));
    assert!(fs::read_dir(&fixture.0).unwrap().all(|p| {
        !p.unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".local.request.json")
    }));
    assert_eq!(
        usbbridge::settings::read::<usbbridge::settings::HostSettings>(&fixture.config())
            .unwrap()
            .name,
        "Before"
    );
}
