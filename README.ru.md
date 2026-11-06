# USB to Network Bridge

[![Version](https://img.shields.io/github/v/release/AlexeySmirnov74/USB-to-Network-Bridge?style=flat-square&logo=github&label=version)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest)
[![Build](https://img.shields.io/github/actions/workflow/status/AlexeySmirnov74/USB-to-Network-Bridge/ci.yml?branch=main&style=flat-square&logo=githubactions&logoColor=white&label=build)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/actions/workflows/ci.yml)
[![Windows 11 x64](https://img.shields.io/badge/platform-Windows%2011%20x64-0078D4?style=flat-square)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge#quick-start)
[![Rust 2024](https://img.shields.io/badge/Rust-2024%20edition-CE422B?style=flat-square&logo=rust&logoColor=white)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/blob/main/Cargo.toml)
[![License](https://img.shields.io/badge/license-MIT-green?style=flat-square&logo=opensourceinitiative&logoColor=white)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/blob/main/LICENSE)
[![Stars](https://img.shields.io/github/stars/AlexeySmirnov74/USB-to-Network-Bridge?style=flat-square&logo=github)](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/stargazers)

[English](README.md) · [Français](README.fr.md) · [Русский](README.ru.md) · [Deutsch](README.de.md) · [简体中文](README.zh-CN.md)

Открытый USB-редиректор для Windows 11 x64: USB over IP (USB/IP) по локальной сети или VPN. Передавайте настоящие USB-устройства между компьютерами Windows, включая веб-камеры и ADB/память телефона Android. Сервер Rust публикует выбранные устройства, клиент подключает их через виртуальный USB-контроллер. Нет активации, trial-таймера и лицензионного лимита устройств.

**v0.2.0 — Windows 11 x64.** Сервер и клиент USB на Rust с графическим интерфейсом, командной строкой, автоматическим поиском и переподключением, установщиком MSI и переносимым ZIP. [Скачать последнюю версию](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest).

![Сервер Windows, подключённое USB-оборудование, сеть и клиенты Windows; схема на английском](docs/assets/architecture.svg)

## Возможности

- Автопоиск или прямой IP/имя и порт; несколько сохранённых серверов и автоматическое восстановление устройств после перезапуска или возврата кабеля.
- Публикация отдельных устройств без перезапуска остальных сеансов. Одним устройством одновременно владеет один удалённый клиент.
- Переключение между `Native` (сервер Rust) и `usbipd-win`. Оба используют официальный подписанный VBoxUSB; клиент — подписанный виртуальный контроллер USBIP-WIN2.
- Тёмный GUI, трей и [CLI](docs/command-line.md). Язык по умолчанию английский; доступны русский, французский, немецкий и упрощённый китайский.
- HTTP CONNECT, HTTPS CONNECT и SOCKS5; пароль защищён локально Windows DPAPI.
- Camera Test с FPS. На клиенте Low-light compensation сохраняется отдельно для каждой камеры; включение может снижать FPS. Окно теста блокирует главное окно обеих ролей до закрытия.
- Автозагрузка Windows, настраиваемые порты/IP, локальное использование без клиента, удаление отключённых записей через один день по умолчанию.

Колонка **Enabled / Включено** разрешает локальное использование без клиента и независима от **Share / Опубликовать**. Для постоянного резервирования камеры оставьте флажок выключенным. Одновременное локальное и удалённое использование захваченного устройства не поддерживается.

## Отличия usbipd-win и Native Rust

Это два **серверных модуля**, а не два разных драйвера ядра. [usbipd-win](https://github.com/dorssel/usbipd-win) предоставляет штатную службу USB/IP и управление устройствами; **Native** — наш собственный модуль захвата и сервер USB/IP на Rust.

| Компонент | Штатный usbipd-win | Native Rust |
| --- | --- | --- |
| Сервер передачи USB | Служба usbipd-win на C#/.NET | Наш процесс usbbridge-server.exe на Rust |
| Захват физического USB | Официальный подписанный драйвер VBoxUSB | Тот же подписанный VBoxUSB без изменений |
| Путь данных на сервере | USB → VBoxUSB → служба usbipd-win → TCP | USB → VBoxUSB → сервер Rust → TCP |
| Передача запросов | Реализация вышестоящего проекта usbipd-win | Прямые асинхронные IOCTL/IOCP, запросы в конвейере, ограниченные буферы и очереди, TCP_NODELAY |
| Клиент Windows | Наш клиент управляет подписанным USBIP-WIN2 | Тот же клиент и контроллер USBIP-WIN2 |
| Зависимость от usbipd-win | Штатный сервер и управление устройствами | Нужен для установки, инвентаризации и резервирования; его служба не передаёт USB-трафик Native |

Native позволяет непосредственно настраивать путь USB/сеть. Например, инициализация скорости устройства в драйвере перед захватом вернула около **30 FPS** на проверенной Chicony в режиме 1280 × 720 MJPEG. Это результат для конкретной камеры, а не обещание превосходства на любом устройстве. Подробнее: [измерения](docs/performance.md) и [архитектура](docs/architecture.md).

Поиск серверов, GUI/трей, сохранение выбора, переподключение и прокси реализованы самим USB to Network Bridge и доступны с обоими модулями. Модуль переключается в настройках сервера; при переключении активные устройства переподключаются. Test Signing и замена подписанного драйвера ядра не требуются.

## Скриншоты

### Подключение телефона Android на клиенте Windows

![Телефон Android подключён через USB to Network Bridge: ADB Interface в диспетчере устройств и память телефона в Проводнике](docs/assets/connect-phone.jpg)

Клиент подключил realme GT 8 Pro. На удалённом компьютере Windows видны интерфейс ADB и внутренняя память телефона.

### Просмотр камеры и FPS на клиенте Windows

![Камера подключена на клиенте Windows: окно Camera Test с видеопотоком и показателями FPS](docs/assets/connect-camera.jpg)

Режим 1280 × 720 MJPEG, 30 FPS. В Camera Test видны **30,15 FPS при приёме и декодировании**, среднее — **29,84 FPS**. Low-light compensation выключен.

Краткое описание назначения и сценариев использования: [FAQ о проекте на английском](docs/project-overview.md).

## Быстрый старт

Нужны два ПК Windows 11 x64, права администратора для настройки и доверенная сеть/VPN. Secure Boot можно оставить включённым; тестовая подпись для официальных драйверов не нужна. Захват мыши, клавиатуры или диска временно лишает сервер локального доступа к этому устройству.

Получите артефакт `usbbridge-windows-x64` из успешного запуска [GitHub Actions](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/actions/workflows/ci.yml) (может потребоваться вход в GitHub) или соберите проект ниже. Распакуйте ZIP приложения в `C:\USBBridge` на обоих ПК. Сохраните `.ps1` рядом с EXE.

После настройки запускайте сервер двойным щелчком по **`USB-to-Network-Bridge-Server.exe`**, а клиент — по **`USB-to-Network-Bridge-Client.exe`**. Открывается нужный GUI без консольного окна, с сохранёнными настройками. Оставляйте EXE в полной папке приложения. Эти же файлы используются ярлыками MSI в меню «Пуск»; `usbbridge.exe` остаётся инструментом командной строки.

[Скачать EXE сервера](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest/download/USB-to-Network-Bridge-Server.exe) · [Скачать EXE клиента](https://github.com/AlexeySmirnov74/USB-to-Network-Bridge/releases/latest/download/USB-to-Network-Bridge-Client.exe). Скачанные отдельно EXE поместите в полную папку приложения, описанную выше.

На **сервере**, в Windows PowerShell от администратора:

```powershell
Set-Location C:\USBBridge
.\Install-ProductionDrivers.ps1 -Role Server -AllowedPeer 192.0.2.10
Copy-Item .\server.example.psd1 .\server.local.psd1
notepad .\server.local.psd1
```

Замените пример IP на настоящий адрес клиента в команде и `AllowedPeers` профиля. Оставьте `Export = @()` для первого запуска. При запросе установщика перезагрузите Windows вручную. Существующие драйверы сценарий не заменяет: используйте `Inspect-Host.ps1` и установщик соответствующего проекта.

```powershell
.\Switch-Backend.ps1 -Backend Native -RuntimeDirectory C:\USBBridge
.\Set-Autostart.ps1 -Role Server -RuntimeDirectory C:\USBBridge `
    -Config C:\USBBridge\host.local.json -Profile C:\USBBridge\server.local.psd1
.\USB-to-Network-Bridge-Server.exe
```

В таблице нажмите правой кнопкой → **Share** или дважды щёлкните неопубликованное устройство. Для штатного модуля выберите его в настройках либо выполните `Switch-Backend.ps1 -Backend Usbipd`; смена модуля переподключает активные устройства.

На **клиенте**, в Windows PowerShell от администратора:

```powershell
Set-Location C:\USBBridge
.\Install-ProductionDrivers.ps1 -Role Client
Copy-Item .\client.example.json .\client.local.json
# При необходимости сначала перезагрузите Windows вручную.
.\Set-Autostart.ps1 -Role Client -RuntimeDirectory C:\USBBridge `
    -Config C:\USBBridge\client.local.json
.\USB-to-Network-Bridge-Client.exe
```

Нажмите **Find servers**, выберите сервер, опубликованное устройство и **Connect**. Для другой подсети используйте **Add server** с LAN/VPN-адресом. Выбор запоминается; **Disconnect** отменяет автоподключение. На узле сервера можно приостановить все его устройства независимо от остальных серверов.

Для камеры: правое меню → **Camera Test**. Локальный тест на сервере требует локально доступной камеры; захваченную удалённо камеру проверяйте на клиенте. Без уникальной постоянной USB-идентичности после смены порта иногда нужен повторный выбор.

| Канал | По умолчанию | Назначение |
| --- | --- | --- |
| TCP | 3240 | USB/IP |
| UDP | 3241 | Автопоиск в локальных IPv4-сетях |
| TCP | 3241 | Список устройств и состояние |

Порты: **1–65535**, порт данных должен отличаться от порта списка/поиска. На клиентах задайте тот же порт поиска. Прокси должен разрешать оба TCP-порта; UDP-поиск через него не передаётся.

## Эксплуатация и сборка

Закрытие GUI оставляет фоновые задачи работать. Автозапуск восстанавливает соединения до входа пользователя; трей появляется после входа. Один экземпляр каждой роли на Windows. Отключение автозапуска: та же команда `Set-Autostart.ps1` с прежними путями и `-Disable`.

Журналы: `logs\*.debug.jsonl` рядом с конфигурацией, с ротацией. Изображения, USB-данные и пароли не записываются; IP, пути и серийные номера могут присутствовать. Проверьте их перед публичной отправкой. USB/IP здесь не шифруется и не проверяет личность сервера: используйте LAN/VPN и IP-фильтры, не открывайте порты в Интернет. HTTPS-прокси шифрует только участок клиент–прокси. Длительный обрыв может вызвать повторную USB-нумерацию; непрерывность любого сеанса не гарантируется.

Для сборки нужны Rust stable MSVC, `rustfmt`, `clippy`, MSVC C++ Build Tools и Windows SDK:

```powershell
git clone https://github.com/AlexeySmirnov74/USB-to-Network-Bridge.git
Set-Location USB-to-Network-Bridge
.\Build.ps1
.\Package.ps1
```

ZIP появится в `dist`; внутри EXE, рабочие сценарии, примеры и лицензии зависимостей. Драйверы загружаются отдельно с проверкой закреплённых SHA-256 и Authenticode (`driver-installers.json`). CI не устанавливает драйверы и не заменяет аппаратные проверки. Необязательный `Bootstrap.ps1` устанавливает Rust; `-InstallBuildTools` добавляет инструменты Microsoft.

**PowerShell 5.1 и рабочие `.ps1` обязательны** для публикации, захвата, переключения модулей и автозапуска. Сценарии сборки на установленном ПК не нужны. Свои EXE и MSI выпускаем без Authenticode; официальные подписи USB-драйверов сохраняются. MSI поддерживает установку, восстановление, обновление, откат и удаление. [Установка](docs/installation.md) · [Сборка и выпуск](docs/production-readiness.md).

[Архитектура](docs/architecture.md) · [CLI](docs/command-line.md) · [Камеры](docs/camera-preview.md) · [Производительность](docs/performance.md) · [Аппаратные испытания](docs/hardware-tests.md) · [Идентификаторы](docs/device-identifiers.md) · [Память](docs/memory-audit.md)

Код проекта — [MIT](LICENSE); зависимости и внешние драйверы сохраняют свои [лицензии](THIRD-PARTY.txt). Локальные настройки, журналы, инвентарь устройств, пароли и ключи подписи не входят в публичные пакеты.
