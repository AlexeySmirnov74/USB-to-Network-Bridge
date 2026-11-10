# USB to Network Bridge

[![Version](https://img.shields.io/github/v/release/AlexeySmirnov74/USB-to-Network-Bridge?style=flat-square&logo=github&label=version)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/AlexeySmirnov74/USB-to-Network-Bridge/ci.yml?branch=main&style=flat-square&logo=githubactions&logoColor=white&label=build)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/actions/workflows/ci.yml)
[![Windows 11 x64](https://img.shields.io/badge/platform-Windows%2011%20x64-0078D4?style=flat-square)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge#quick-start)
[![Rust 2024](https://img.shields.io/badge/Rust-2024%20edition-CE422B?style=flat-square&logo=rust&logoColor=white)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/blob/main/Cargo.toml)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square&logo=opensourceinitiative&logoColor=white)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/blob/main/LICENSE)
[![Stars](https://img.shields.io/github/stars/AlexeySmirnov74/USB-to-Network-Bridge?style=flat-square&logo=github)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/stargazers)

[English](README.md) · [Français](README.fr.md) · [Русский](README.ru.md) · [Deutsch](README.de.md) · [简体中文](README.zh-CN.md)

Un redirecteur USB over IP (USB/IP) open source pour Windows 11 x64. Partagez de vrais périphériques USB sur un LAN ou un VPN, dont webcams et interfaces ADB/stockage de téléphones Android. Le serveur Rust exporte les appareils sélectionnés ; le client Windows les importe via un contrôleur USB virtuel. Sans activation, expiration d'essai ni quota de périphériques lié à une licence.

**v0.2.2 — Windows 11 x64.** Serveur et client USB en Rust avec interface graphique, ligne de commande, découverte et reconnexion automatiques, installateur MSI et ZIP portable. [Télécharger la dernière version](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest).

**État des agents dans la v0.2.1 :** les contrôleurs serveur et client ont des superviseurs distincts. Un contrôleur arrêté redémarre automatiquement ; un signal de vie figé est détecté après deux minutes. Des tâches indépendantes vérifient aussi les superviseurs chaque minute. Les anciens rapports ne déclarent plus un périphérique connecté. Diagnostic : `usbbridge --config FILE server health` ou `client health`. [Modifications](CHANGELOG.md).

![Périphériques USB, serveur Windows, réseau de confiance et clients Windows ; schéma en anglais](docs/assets/architecture.svg)

## Caméra USB alimentée par une source vidéo distante

**Nouveau dans v0.2.2 :** envoyer la vidéo du client Windows vers une caméra USB/UVC sur le serveur Windows. Dans **Serveur → Caméra USB — réception vidéo**, choisir son nom, par exemple `4k web camera`. Sur le client, ouvrir **Périphérique → Envoyer la vidéo**, puis sélectionner la source, l’adresse du serveur et la même clé de connexion. La caméra utilise le contrôleur signé USBIP-WIN2 et `usbvideo.sys` de Windows, sans suffixe Media Foundation obligatoire. Les sources OBS/Unity Capture installées sont sélectionnées via DirectShow. [Démarrage rapide vidéo](docs/virtual-camera.md).

Le transport TCP/MJPEG authentifié conserve la caméra USB lors d’une perte de source et affiche du noir jusqu’à la reconnexion automatique. Les deux modules ont un démarrage automatique et un contrôle indépendant du heartbeat. Le nom ne fixe pas la résolution : environ 30 FPS à 1280×720 ont été mesurés avec une vraie caméra. Utiliser un LAN de confiance ou un VPN : l’authentification ne chiffre pas les images. Le contrôleur USB virtuel reste identifiable dans l’arbre des périphériques Windows.

## Fonctionnalités

- Découverte automatique ou adresse IP/nom et port ; plusieurs serveurs enregistrés et reconnexion après redémarrage ou rebranchement.
- Partage individuel sans redémarrer les autres sessions USB ; un seul client distant par périphérique à la fois.
- Serveur Rust `Native` ou service `usbipd-win`, tous deux avec le pilote VBoxUSB signé fourni en amont. Le client utilise le contrôleur virtuel USBIP-WIN2 signé.
- Interface sombre, zone de notification et [ligne de commande](docs/command-line.md). Anglais par défaut ; français, russe, allemand et chinois simplifié disponibles.
- HTTP CONNECT, HTTPS CONNECT et SOCKS5 par serveur ; identifiants protégés localement par Windows DPAPI.
- **Camera Test** avec FPS reçus/décodés ; compensation de faible éclairage mémorisée par caméra sur le client. Cette option peut réduire les FPS. La fenêtre de test bloque la fenêtre principale des deux rôles jusqu’à sa fermeture.
- Démarrage Windows, ports et IP autorisées configurables, usage local sans client et nettoyage des appareils débranchés après un jour par défaut.

La case **Enabled** du serveur autorise l’usage local en l’absence de client, indépendamment de **Share**. Laissez-la décochée pour réserver une webcam au distant. Un appareil capturé ne peut pas être utilisé simultanément localement et à distance.

## usbipd-win et Native Rust

Il s'agit de deux **modules serveur**, et non de deux pilotes noyau différents. [usbipd-win](https://github.com/dorssel/usbipd-win) fournit le service USB/IP amont et la gestion des appareils ; **Native** est notre propre module de capture et serveur USB/IP en Rust.

| Composant | usbipd-win standard | Native Rust |
| --- | --- | --- |
| Serveur du trafic USB | Service usbipd-win en C#/.NET | Notre exécutable Rust usbbridge-server.exe |
| Capture USB physique | Pilote VBoxUSB amont signé pour la production | Le même pilote VBoxUSB signé, sans modification |
| Chemin des données côté serveur | USB → VBoxUSB → service usbipd-win → TCP | USB → VBoxUSB → serveur Rust → TCP |
| Transferts | Implémentation maintenue par le projet usbipd-win | Capture IOCTL/IOCP asynchrone directe, requêtes en pipeline, files/tampons bornés et TCP_NODELAY |
| Client Windows | Notre client pilote le contrôleur virtuel USBIP-WIN2 signé | Le même client et contrôleur USBIP-WIN2 |
| Dépendance à usbipd-win | Serveur standard et gestion des appareils | Toujours nécessaire pour installation, inventaire et réservation ; son service ne transporte pas le trafic USB Native |

Native permet d'ajuster directement le chemin USB/réseau. Initialiser la vitesse du périphérique dans le pilote avant sa capture a, par exemple, rétabli environ **30 FPS** pour la Chicony testée en MJPEG 1280 × 720. Ce résultat concerne cet appareil et ne garantit pas que Native soit plus rapide dans tous les cas. Voir les [mesures](docs/performance.md) et l'[architecture](docs/architecture.md).

Découverte, GUI/tray, choix mémorisés, reconnexion et proxy sont des fonctions de USB to Network Bridge disponibles avec les deux modules. Changez de module dans les paramètres du serveur ; les appareils actifs se reconnectent. Aucun mode Test Signing ni remplacement du pilote noyau signé n'est nécessaire.

## Captures d'écran

### Téléphone Android connecté au client Windows

![Téléphone Android importé : interface ADB dans le Gestionnaire de périphériques et stockage dans l'Explorateur Windows](docs/assets/connect-phone.jpg)

Le client a importé un realme GT 8 Pro. Windows expose son interface ADB et sa mémoire interne sur l'ordinateur distant.

### Aperçu webcam et FPS sur le client Windows

![Webcam USB importée sur le client Windows, avec aperçu et compteurs FPS dans Camera Test](docs/assets/connect-camera.jpg)

Mode MJPEG 1280 × 720 à 30 FPS. Camera Test affiche **30,15 FPS reçus et décodés**, avec une **moyenne de 29,84 FPS** ; la compensation de faible luminosité est désactivée.

Présentation et cas d'utilisation : [FAQ du projet en anglais](docs/project-overview.md).

## Démarrage rapide

Prérequis : deux PC Windows 11 x64, droits administrateur pour les pilotes et la configuration, réseau de confiance/VPN. Secure Boot reste activé ; les pilotes officiels ne nécessitent pas de signature de test. Capturer une souris, un clavier ou un disque supprime temporairement son accès local.

Téléchargez l’artefact `usbbridge-windows-x64` d’une exécution réussie de [GitHub Actions](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/actions/workflows/ci.yml) (connexion GitHub éventuellement nécessaire), ou compilez ci-dessous. Extrayez le ZIP de l’application dans `C:\USBBridge` sur les deux PC. Conservez les `.ps1` avec les EXE.

Après la configuration, double-cliquez sur **`USB-to-Network-Bridge-Server.exe`** côté serveur ou **`USB-to-Network-Bridge-Client.exe`** côté client. Ils ouvrent la bonne interface sans console et réutilisent les paramètres enregistrés. Conservez-les dans le dossier complet de l’application. Les raccourcis du MSI utilisent aussi ces lanceurs ; `usbbridge.exe` reste l’outil en ligne de commande.

[Télécharger l’EXE serveur](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest/download/USB-to-Network-Bridge-Server.exe) · [Télécharger l’EXE client](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest/download/USB-to-Network-Bridge-Client.exe). Placez les EXE téléchargés séparément dans le dossier complet de l’application décrit ci-dessus.

Sur le **serveur**, dans Windows PowerShell **en administrateur** :

```powershell
Set-Location C:\USBBridge
.\Install-ProductionDrivers.ps1 -Role Server -AllowedPeer 192.0.2.10
Copy-Item .\server.example.psd1 .\server.local.psd1
notepad .\server.local.psd1
```

Remplacez l’adresse d’exemple par l’IP réelle du client, dans la commande et dans `AllowedPeers`. Gardez `Export = @()` au premier démarrage. Si l’installateur le demande, redémarrez manuellement avant de continuer. Les installations de pilotes existantes ne sont pas écrasées : utilisez `Inspect-Host.ps1` et l’installateur du projet concerné.

```powershell
.\Switch-Backend.ps1 -Backend Native -RuntimeDirectory C:\USBBridge
.\Set-Autostart.ps1 -Role Server -RuntimeDirectory C:\USBBridge `
    -Config C:\USBBridge\host.local.json -Profile C:\USBBridge\server.local.psd1
.\USB-to-Network-Bridge-Server.exe
```

Clic droit sur un appareil → **Share**, ou double-clic sur une ligne non partagée. Pour le service standard, sélectionnez-le dans les paramètres ou utilisez `Switch-Backend.ps1 -Backend Usbipd`. Le changement de module reconnecte les appareils actifs.

Sur le **client**, dans Windows PowerShell **en administrateur** :

```powershell
Set-Location C:\USBBridge
.\Install-ProductionDrivers.ps1 -Role Client
Copy-Item .\client.example.json .\client.local.json
# Redémarrez manuellement d’abord si l’installateur le demande.
.\Set-Autostart.ps1 -Role Client -RuntimeDirectory C:\USBBridge `
    -Config C:\USBBridge\client.local.json
.\USB-to-Network-Bridge-Client.exe
```

Cliquez sur **Find servers**, choisissez un serveur puis un appareil partagé et **Connect**. Utilisez **Add server** avec son adresse LAN/VPN si nécessaire. Les choix sont mémorisés ; **Disconnect** annule la reconnexion correspondante. Sur le nœud serveur, vous pouvez suspendre tous ses appareils sans affecter les autres serveurs.

Pour une webcam : clic droit → **Camera Test**. Le test local du serveur nécessite une caméra disponible localement ; testez sur le client une caméra capturée à distance. Sans identité USB stable et unique, un changement de port peut nécessiter une nouvelle sélection.

| Canal | Port par défaut | Usage |
| --- | --- | --- |
| TCP | 3240 | Données USB/IP |
| UDP | 3241 | Découverte IPv4 locale |
| TCP | 3241 | Catalogue et état |

Ports **1–65535** ; le port USB doit différer du port catalogue/découverte. Configurez le même port de découverte sur les clients. Le proxy doit autoriser les deux ports TCP ; la découverte UDP ne le traverse pas.

## Utilisation et compilation

Fermer l’interface laisse les tâches de fond actives. Les tâches de démarrage restaurent les connexions avant ouverture de session ; l’icône apparaît ensuite. Une instance de chaque rôle par PC. Pour désactiver le démarrage, réutilisez `Set-Autostart.ps1` avec les mêmes chemins et `-Disable`.

Journaux avec rotation : `logs\*.debug.jsonl` près de la configuration. Aucune image, charge utile USB ou mot de passe ; des IP, chemins et numéros de série peuvent subsister. Relisez avant publication. USB/IP n’est ici ni chiffré ni authentifié : utilisez LAN/VPN et filtrage IP, sans exposer les ports à Internet. HTTPS proxy chiffre uniquement le trajet client–proxy. Une longue coupure peut provoquer une réénumération USB ; la continuité permanente n’est pas garantie.

Compilation : Rust stable MSVC avec `rustfmt`/`clippy`, MSVC C++ Build Tools et Windows SDK :

```powershell
git clone https://github.com/AlexeySmirnov74/USB-to-Network-Bridge.git
Set-Location USB-to-Network-Bridge
.\Build.ps1
.\Package.ps1
```

Le ZIP dans `dist` contient les EXE, scripts nécessaires, exemples et licences. Les pilotes sont téléchargés séparément avec vérification SHA-256 et Authenticode (`driver-installers.json`). La CI n’installe pas de pilotes et ne remplace pas les essais matériels. `Bootstrap.ps1` installe facultativement Rust ; `-InstallBuildTools` ajoute les outils Microsoft.

**Windows PowerShell 5.1 et les scripts `.ps1` d’exécution sont nécessaires** pour le partage, la capture, les modules et le démarrage. Les scripts de compilation ne sont pas requis sur le PC installé. Les EXE et le MSI du projet ne sont pas signés Authenticode ; les pilotes USB conservent leurs signatures officielles. Le MSI prend en charge installation, réparation, mise à niveau, restauration et désinstallation. [Installation](docs/installation.md) · [Compilation et publication](docs/production-readiness.md).

[Architecture](docs/architecture.md) · [CLI](docs/command-line.md) · [Caméras](docs/camera-preview.md) · [Performances](docs/performance.md) · [Essais matériels](docs/hardware-tests.md) · [Identifiants](docs/device-identifiers.md) · [Mémoire](docs/memory-audit.md). Certaines notes techniques sont en russe.

Code sous [MIT](LICENSE) ; les dépendances et pilotes conservent leurs [licences](THIRD-PARTY.txt). Les configurations locales, journaux, inventaires USB, identifiants secrets et clés de signature sont exclus des paquets publics.
