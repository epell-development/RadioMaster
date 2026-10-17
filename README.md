<p align="center">
  <img src="assets/logo_light.png" alt="RadioMaster" width="420">
</p>

<p align="center"><b>Radio automation for Windows, written in Rust.</b><br>
Playlists, hour markers, smart crossfades, Icecast / Shoutcast streaming and AzuraCast station management in one studio app.</p>

<p align="center">
  <a href="../../releases/latest">Download</a> ·
  <a href="#playlists">Playlists</a> ·
  <a href="#work-zones">Work zones</a> ·
  <a href="#azuracast-station-manager-tools--azuracast-optional">AzuraCast</a>
</p>

![RadioMaster main window](docs/screenshots/main.png)

RadioMaster uses a classic broadcast-automation layout: playlist tabs in the centre, transport,
VU meters and the Last / On Air / Next tiles on top, panels around it, and tools in their own windows.

## Download
From the [latest release](../../releases/latest):
- **`RadioMaster-Setup-<version>.exe`** — installer with Start-menu / desktop shortcuts and an uninstaller
  (install just for you, no admin needed, or for all users), or
- **`RadioMaster-<version>-windows-x64.zip`** — portable: unzip anywhere and run `RadioMaster.exe`.

On first start you create the local administrator account.
Embedded web panels use the Microsoft Edge WebView2 runtime, which Windows 10/11 already include.

## Build from source (Windows)
Double-click **`run-windows.bat`** (or `cargo run --release` in this folder).
**`build-windows.bat`** produces `target\release\RadioMaster.exe` (with the RadioMaster icon).
**`build-installer.bat`** also makes `dist\RadioMaster-Setup-<version>.exe` (needs [Inno Setup 6](https://jrsoftware.org/isdl.php)).

Linux: `sudo apt install build-essential libasound2-dev pkg-config`, then `cargo run --release`.

Settings, playlists, layout and `history.csv` live in `%APPDATA%\RadioMaster` (settings from an older
OpenBoss install are copied across on first start). If the program ever crashes, the reason is in `crash.log` there.

## Playlists
- As many playlist tabs as you like (the ▶ tab is on air). Double-click a tab to rename it; right-click for
  Put on air, Duplicate, Save/Load M3U, move, Clear, Close. Playing any row of another tab puts that tab on air.
- Columns: #, Track Title, Duration, Start time (projected air time, hour markers included), Last played, Type.
- Right-click rows: Play now, Track editor / Edit…, Move up/down, Copy/Move to another playlist, Upload to AzuraCast, Remove.

![Playlist menu](docs/screenshots/playlist-menu.png)

### Playlist menu
| Item | What it does |
|---|---|
| Playlist Generator Pro… (Ctrl+G) | category/template generator |
| Add file / folder / URL (Alt+F / Alt+D / Alt+U) | URL = relay an internet stream for a set time or until Next |
| Add Line Input… | play the input device live (for a set time or until Next) |
| Add command… (Alt+C) | `STOP`, `PAUSE`, `VOLUME 80`, `ENCODER START 1`, `ENCODER STOP 1`, `LOAD file.m3u`, `PLAYLIST name`, `RUN program args` |
| Add timed pause… (Ctrl+P) | silence for a set time |
| Add teaser… | faded snippets of the next N tracks |
| Add time announcement… | recordings H00…H23 / M00…M59 from a folder, otherwise the computer's speech voice reads your text (`{h12}:{mm} {ampm}`) |
| Add comment… (Alt+W) | a note row; nothing plays |
| Add container… | several files as one row: all in order, or one at random |
| Insert voice track (Ctrl+W) / Record voice track (Ctrl+R) | recordings go to `voicetracks\` |
| Add Track List… + Track List Repeat Protection… | each time the row is reached, one random track from a folder/M3U that hasn't played recently (track and artist minutes) |
| Insert playlist / Insert tracks from playlist… | insert an M3U after the selection, or pick tracks from it |
| Extra ▸ | hour marker, shuffle, sort, reverse, remove duplicates / played / missing, render WAV, export CSV, clear |
| Notepad… (T), Find… (Ctrl+F) | |

Hour markers: *wait* (silence until the time if early), *soft* (finish the current track, then jump) or *hard* (cut in on time).
The clock shows the countdown to the next marker.

![Waiting for an hour marker](docs/screenshots/hour-marker.png)

## Work zones
View → Work zones, the **1 2 3** buttons, or **Ctrl+1/2/3** set how many panels the main window has
(1 = playlist + left panel, 2 = + right panel, 3 = + lower-right panel). **Ctrl+4 / ➕🗗** opens an
**Additional** window — a real window with its own tabbed panel. Open as many as you like and put them on other
screens; they reopen where you left them. Each panel's ➕ adds AUX Playlist, Cart wall, Search, Scheduler,
File Explorer, Notepad, Statistics, Song Requests, Playback Queue, Log, Library, Clock or Web 1–4.
Right-click a tab to move it to another panel or into a new Additional window.

Web 1–4 are empty until you type an address and give the tab a name — nothing is built in.

![An Additional window next to the main window](docs/screenshots/additional-window.png)

## AzuraCast station manager (Tools → AzuraCast, optional)
Connection: server address + station (or *Find stations on this server*). Now playing and listener counts need no key.
With an **API key** (AzuraCast → your account → API Keys) you also get:
Overview (start/stop/restart AutoDJ and broadcast server, skip, disconnect live DJ, restart broadcasting) ·
Queue (remove) · Requests (pending list, delete, clear, add to the local playlist, request songs) ·
Playlists (enable/disable, reshuffle, queue) · Media (browse, upload here, new folder, queue next, delete) ·
Streamers/DJs (add, edit, delete, fill encoder) · Listeners · History · Schedule.
The **Song Requests** panel tab shows the same request list.

![AzuraCast station manager](docs/screenshots/azuracast.png)

## Going on air with AzuraCast
1. AzuraCast → Streamers/DJs: create an account (or do it in the manager) and note the DJ port.
2. Tools → Streams → *AzuraCast DJ preset*, fill host, port, username, password → **Connect**.

## Also included
Two-deck crossfading with smart fading and a waveform track editor (cues, fades, intro/outro), cart wall, mic with ducking,
multiple outputs, 12-band EQ / compressor / leveling, ad scheduler, event scheduler, rotations, reports, metadata export,
remote control HTTP API, station profiles, local operator accounts.

## Contributing
Bug reports and pull requests are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).

## License
[MIT](LICENSE).

RadioMaster is an independent project. It is not affiliated with or endorsed by DJSoft.net (RadioBOSS) or AzuraCast;
those names are used only to describe compatibility and familiar workflows.
