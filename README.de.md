# USB to Network Bridge

[![Version](https://img.shields.io/github/v/release/AlexeySmirnov74/USB-to-Network-Bridge?style=flat-square&logo=github&label=version)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/AlexeySmirnov74/USB-to-Network-Bridge/ci.yml?branch=main&style=flat-square&logo=githubactions&logoColor=white&label=build)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/actions/workflows/ci.yml)
[![Windows 11 x64](https://img.shields.io/badge/platform-Windows%2011%20x64-0078D4?style=flat-square)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge#quick-start)
[![Rust 2024](https://img.shields.io/badge/Rust-2024%20edition-CE422B?style=flat-square&logo=rust&logoColor=white)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/blob/main/Cargo.toml)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square&logo=opensourceinitiative&logoColor=white)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/blob/main/LICENSE)
[![Stars](https://img.shields.io/github/stars/AlexeySmirnov74/USB-to-Network-Bridge?style=flat-square&logo=github)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/stargazers)

[English](README.md) · [Français](README.fr.md) · [Русский](README.ru.md) · [Deutsch](README.de.md) · [简体中文](README.zh-CN.md)

Ein quelloffener USB-over-IP-Redirector (USB/IP) für Windows 11 x64. Teilen Sie echte USB-Geräte über LAN oder VPN zwischen Windows-PCs, darunter Webcams und ADB/Speicher von Android-Telefonen. Der Rust-Server gibt ausgewählte Geräte frei; der Windows-Client bindet sie über einen virtuellen USB-Controller ein. Keine Aktivierung, keine Testzeitbegrenzung und kein lizenzbedingtes Gerätelimit.

**v0.2.4 — Windows 11 x64.** USB-Server und Client in Rust mit grafischer Oberfläche, Kommandozeile, automatischer Suche und Wiederverbindung sowie MSI-Installer und portablem ZIP. [Aktuelle Version herunterladen](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest).

**Agentenprüfung in v0.2.1:** Server- und Client-Controller haben eigene Supervisoren. Beendete Controller starten automatisch neu; ein eingefrorener Heartbeat wird nach zwei Minuten erkannt. Unabhängige Aufgaben prüfen die Supervisoren jede Minute. Veraltete Berichte melden Geräte nicht mehr als verbunden. Diagnose: `usbbridge --config FILE server health` oder `client health`. [Änderungen](CHANGELOG.md).

![USB-Bridge-Architektur: physische USB-Geräte vom Server zu Clients und OBS-/Unity-Capture-Video vom Client zur USB/UVC-Kamera des Servers; englische Grafik](docs/assets/architecture.svg)


**v0.2.3:** Kameranamen werden zwischen DirectShow und Media Foundation synchronisiert. Die Aufnahme wacht bei einem neuen Bild auf, verwendet begrenzte Arbeitspuffer erneut und meldet Verarbeitungszeiten. Für eine Videoanwendung auf dem Client-PC gibt es eine [OBS-/Unity-Capture-Anleitung](docs/third-party-video.md) auf Englisch mit CLI-Beispielen und Aufgabenbeschreibung für ein weiteres Projekt.

## USB-Kamera aus einer entfernten Videoquelle

**Neu in v0.2.2:** Video vom Windows-Client an eine USB/UVC-Kamera auf dem Windows-Server senden. Unter **Server → USB-Kamera — Video empfangen** den Namen festlegen, etwa `4k web camera`. Auf dem Client unter **Gerät → Video senden** Quelle, Serveradresse und denselben Verbindungsschlüssel wählen. Die Kamera nutzt den signierten USBIP-WIN2-Controller und Windows `usbvideo.sys`, ohne den vorgeschriebenen Media-Foundation-Namenszusatz. Installierte OBS-/Unity-Capture-Quellen werden über DirectShow ausgewählt. [Video-Schnellstart](docs/virtual-camera.md).

Der authentifizierte TCP/MJPEG-Pfad hält die USB-Kamera bei Verbindungsproblemen angemeldet und zeigt Schwarz, bis die Quelle zurückkehrt. Beide Endpunkte haben Autostart und unabhängige Heartbeat-Prüfungen. Der Name bestimmt nicht die Auflösung; mit einer echten Kamera wurden ungefähr 30 FPS bei 1280×720 gemessen. Ein vertrauenswürdiges LAN oder VPN verwenden: die Authentifizierung verschlüsselt die Bilder nicht. Der virtuelle USB-Controller bleibt im Windows-Gerätebaum erkennbar.

## Funktionen

- Automatische Serversuche oder direkte IP/Hostname mit Port; mehrere gespeicherte Server und Wiederverbindung nach Neustart oder erneutem Einstecken.
- Einzelne Geräte freigeben, ohne andere USB-Sitzungen neu zu starten. Pro Gerät ist jeweils ein entfernter Client möglich.
- Umschalten zwischen Rust-Transport `Native` und `usbipd-win`. Beide verwenden den unveränderten signierten VBoxUSB-Treiber; der Client nutzt den signierten virtuellen USBIP-WIN2-Controller.
- Dunkle Oberfläche, Infobereich und [Kommandozeile](docs/command-line.md). Standardsprache Englisch; Deutsch, Französisch, Russisch und vereinfachtes Chinesisch verfügbar.
- HTTP CONNECT, HTTPS CONNECT oder SOCKS5 pro Server; Zugangsdaten lokal durch Windows DPAPI geschützt.
- **Camera Test** mit empfangenen/dekodierten FPS. Der Client speichert den Schwachlichtausgleich pro Kamera; Aktivierung kann die FPS senken. Das Testfenster sperrt bei beiden Rollen das Hauptfenster bis zum Schließen.
- Windows-Autostart, Ports, IP-Freigaben, lokale Nutzung ohne Client und Bereinigung entfernter Geräte nach standardmäßig einem Tag.

**Enabled** in der Servertabelle erlaubt lokale Nutzung ohne verbundenen Client, unabhängig von **Share**. Für eine dauerhaft reservierte Webcam deaktiviert lassen. Gleichzeitige lokale und entfernte Nutzung desselben erfassten Geräts ist nicht möglich.

## usbipd-win und Native Rust

Dies sind zwei **Servermodule**, keine unterschiedlichen Kerneltreiber. [usbipd-win](https://github.com/dorssel/usbipd-win) stellt den ursprünglichen USB/IP-Dienst und die Geräteverwaltung bereit; **Native** ist unsere eigene USB-Erfassung und unser USB/IP-Server in Rust.

| Komponente | Standard-usbipd-win | Native Rust |
| --- | --- | --- |
| Server für USB-Daten | usbipd-win-Dienst in C#/.NET | Unsere Rust-Anwendung usbbridge-server.exe |
| Physische USB-Erfassung | Offiziell signierter VBoxUSB-Treiber | Derselbe unveränderte, signierte VBoxUSB-Treiber |
| Datenpfad auf dem Server | USB → VBoxUSB → usbipd-win-Dienst → TCP | USB → VBoxUSB → Rust-Server → TCP |
| Übertragungen | Implementierung des usbipd-win-Projekts | Direkte asynchrone IOCTL/IOCP-Erfassung, parallele ausstehende Anfragen, begrenzte Puffer/Warteschlangen und TCP_NODELAY |
| Windows-Client | Unser Client steuert den signierten virtuellen USBIP-WIN2-Controller | Derselbe Client und USBIP-WIN2-Controller |
| Abhängigkeit von usbipd-win | Standardserver und Geräteverwaltung | Weiterhin für Installation, Inventar und Reservierung nötig; der Dienst leitet keine Native-USB-Daten weiter |

Mit Native können wir den USB-/Netzwerkpfad direkt abstimmen. Beispielsweise stellte die Initialisierung der Gerätegeschwindigkeit im Treiber vor der Übernahme bei der getesteten Chicony im Modus 1280 × 720 MJPEG etwa **30 FPS** wieder her. Das ist ein gerätespezifisches Messergebnis, keine allgemeine Geschwindigkeitsgarantie. Siehe [Messungen](docs/performance.md) und [Architektur](docs/architecture.md).

Serversuche, GUI/Tray, gespeicherte Auswahl, Wiederverbindung und Proxy-Unterstützung gehören zu USB to Network Bridge und stehen bei beiden Modulen bereit. Das Modul lässt sich in den Servereinstellungen wechseln; aktive Geräte verbinden sich dabei neu. Test Signing oder der Austausch des signierten Kerneltreibers sind nicht erforderlich.

## Screenshots

### Android-Telefon auf dem Windows-Client

![Über USB to Network Bridge verbundenes Android-Telefon mit ADB-Schnittstelle im Geräte-Manager und Telefonspeicher im Explorer](docs/assets/connect-phone.jpg)

Der Client hat ein realme GT 8 Pro eingebunden. Auf dem entfernten Windows-PC sind dessen ADB-Schnittstelle und interner Speicher verfügbar.

### Webcam-Vorschau und FPS auf dem Windows-Client

![Auf dem Windows-Client eingebundene USB-Webcam mit Live-Vorschau und FPS-Anzeige in Camera Test](docs/assets/connect-camera.jpg)

1280 × 720 MJPEG im 30-FPS-Modus. Camera Test zeigt **30,15 empfangene und dekodierte FPS**, im **Durchschnitt 29,84 FPS**. Der Schwachlichtausgleich ist ausgeschaltet.

Eine kurze Übersicht und Einsatzbeispiele bietet die [englische Projekt-FAQ](docs/project-overview.md).

## Schnellstart

Benötigt werden zwei Windows-11-x64-PCs, Administratorrechte für Treiber/Einrichtung und ein vertrauenswürdiges LAN/VPN. Secure Boot kann eingeschaltet bleiben; offizielle Treiber erfordern keinen Testsignaturmodus. Die Übernahme einer Maus, Tastatur oder Festplatte macht sie lokal vorübergehend unbenutzbar.

Laden Sie das Artefakt `usbbridge-windows-x64` eines erfolgreichen [GitHub-Actions-Laufs](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/actions/workflows/ci.yml) herunter (GitHub-Anmeldung eventuell erforderlich) oder bauen Sie das Projekt wie unten beschrieben. Entpacken Sie das Anwendungs-ZIP auf beiden PCs nach `C:\USBBridge`. Die `.ps1`-Dateien müssen neben den EXE-Dateien bleiben.

Nach der Einrichtung starten Sie den Server per Doppelklick auf **`USB-to-Network-Bridge-Server.exe`** und den Client über **`USB-to-Network-Bridge-Client.exe`**. Die passende Oberfläche öffnet sich ohne Konsolenfenster mit den gespeicherten Einstellungen. Belassen Sie die EXE-Dateien im vollständigen Anwendungsordner. Auch die MSI-Startmenüeinträge verwenden diese Starter; `usbbridge.exe` bleibt das Kommandozeilenwerkzeug.

[Server-EXE herunterladen](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest/download/USB-to-Network-Bridge-Server.exe) · [Client-EXE herunterladen](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest/download/USB-to-Network-Bridge-Client.exe). Legen Sie einzeln heruntergeladene EXE-Dateien im oben beschriebenen vollständigen Anwendungsordner ab.

Auf dem **Server**, in Windows PowerShell **als Administrator**:

```powershell
Set-Location C:\USBBridge
.\Install-ProductionDrivers.ps1 -Role Server -AllowedPeer 192.0.2.10
Copy-Item .\server.example.psd1 .\server.local.psd1
notepad .\server.local.psd1
```

Ersetzen Sie die Beispieladresse durch die echte Client-IP, sowohl im Befehl als auch unter `AllowedPeers`. Für den ersten Start `Export = @()` beibehalten. Falls angefordert, Windows vor dem Fortfahren manuell neu starten. Vorhandene Treiberinstallationen werden nicht überschrieben; dafür `Inspect-Host.ps1` und das jeweilige Originalinstallationsprogramm verwenden.

```powershell
.\Switch-Backend.ps1 -Backend Native -RuntimeDirectory C:\USBBridge
.\Set-Autostart.ps1 -Role Server -RuntimeDirectory C:\USBBridge `
    -Config C:\USBBridge\host.local.json -Profile C:\USBBridge\server.local.psd1
.\USB-to-Network-Bridge-Server.exe
```

Rechtsklick auf ein Gerät → **Share**, alternativ Doppelklick auf eine noch nicht freigegebene Zeile. Das Standardmodul kann in den Einstellungen oder mit `Switch-Backend.ps1 -Backend Usbipd` gewählt werden. Ein Modulwechsel verbindet aktive Geräte neu.

Auf dem **Client**, in Windows PowerShell **als Administrator**:

```powershell
Set-Location C:\USBBridge
.\Install-ProductionDrivers.ps1 -Role Client
Copy-Item .\client.example.json .\client.local.json
# Falls angefordert, zunächst Windows manuell neu starten.
.\Set-Autostart.ps1 -Role Client -RuntimeDirectory C:\USBBridge `
    -Config C:\USBBridge\client.local.json
.\USB-to-Network-Bridge-Client.exe
```

**Find servers** wählen, Server und freigegebenes Gerät markieren, dann **Connect**. Bei Bedarf über **Add server** die LAN/VPN-Adresse eintragen. Die Auswahl wird gespeichert; **Disconnect** beendet die entsprechende automatische Wiederverbindung. Über den Serverknoten lassen sich alle seine Geräte pausieren, ohne andere Server zu beeinflussen.

Webcamtest: Rechtsklick → **Camera Test**. Der lokale Servertest benötigt eine lokal verfügbare Kamera; eine entfernt übernommene Kamera wird auf dem Client getestet. Ohne stabile eindeutige USB-Identität kann nach einem Portwechsel eine erneute Auswahl nötig sein.

| Kanal | Standardport | Zweck |
| --- | --- | --- |
| TCP | 3240 | USB/IP-Daten |
| UDP | 3241 | Lokale IPv4-Serversuche |
| TCP | 3241 | Gerätekatalog und Status |

Ports **1–65535**; USB- und Katalogport müssen unterschiedlich sein. Suchport auf Clients passend einstellen. Ein Proxy muss beide TCP-Ports zulassen; UDP-Suche wird nicht durch den Proxy geleitet.

## Betrieb und Build

Das Schließen der Oberfläche beendet keine Hintergrundarbeit. Startaufgaben stellen Verbindungen bereits vor der Benutzeranmeldung wieder her; danach erscheint das Symbol. Eine Instanz je Rolle und PC. Autostart deaktivieren: denselben `Set-Autostart.ps1`-Befehl mit gleichen Pfaden und `-Disable` ausführen.

Rotierende Logs: `logs\*.debug.jsonl` neben der Konfiguration. Keine Bilder, USB-Nutzdaten oder Passwörter; IPs, Pfade und Seriennummern können enthalten sein. Vor Veröffentlichung prüfen. USB/IP ist hier weder verschlüsselt noch authentifiziert: LAN/VPN und IP-Filter verwenden, Ports nicht direkt ins Internet öffnen. HTTPS-Proxy verschlüsselt nur den Weg Client–Proxy. Längere Ausfälle können eine USB-Neuerkennung erzwingen; unterbrechungsfreie Sitzungen sind nicht garantiert.

Buildvoraussetzungen: Rust stable MSVC mit `rustfmt`/`clippy`, MSVC C++ Build Tools und Windows SDK:

```powershell
git clone https://github.com/AlexeySmirnov74/USB-to-Network-Bridge.git
Set-Location USB-to-Network-Bridge
.\Build.ps1
.\Package.ps1
```

Das ZIP unter `dist` enthält EXE-Dateien, Laufzeitskripte, Beispiele und Lizenzen. Treiber werden separat geladen und anhand festgelegter SHA-256-Werte und Authenticode geprüft (`driver-installers.json`). CI installiert keine Treiber und ersetzt keine Hardwaretests. Optional installiert `Bootstrap.ps1` Rust; `-InstallBuildTools` ergänzt Microsoft-Werkzeuge.

**Windows PowerShell 5.1 und die Laufzeit-`.ps1` sind erforderlich** für Freigabe, Erfassung, Modulwechsel und Autostart. Buildskripte werden auf einem eingerichteten PC nicht benötigt. Projekteigene EXE-Dateien und das MSI sind nicht Authenticode-signiert; USB-Treiber behalten ihre offiziellen Signaturen. Das MSI unterstützt Installation, Reparatur, Upgrade, Rollback und Deinstallation. [Installation](docs/installation.md) · [Build und Veröffentlichung](docs/production-readiness.md).

[Architektur](docs/architecture.md) · [CLI](docs/command-line.md) · [Kameras](docs/camera-preview.md) · [Leistung](docs/performance.md) · [Hardwaretests](docs/hardware-tests.md) · [Kennungen](docs/device-identifiers.md) · [Speicherprüfung](docs/memory-audit.md). Einige technische Notizen sind auf Russisch.

Projektcode unter [MIT](LICENSE); Abhängigkeiten und Treiber behalten ihre [Lizenzen](THIRD-PARTY.txt). Lokale Einstellungen, Logs, USB-Inventare, Zugangsdaten und Signaturschlüssel sind nicht in öffentlichen Paketen enthalten.
