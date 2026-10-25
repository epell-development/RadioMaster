<p align="center">
  <img src="assets/logo_light.png" alt="RadioMaster" width="420">
</p>

<p align="center"><b>Free, open-source radio automation software for Windows, macOS and Linux, written in Rust.</b><br>
Playlists, hour markers, smart crossfades, Icecast / Shoutcast streaming and AzuraCast station management in one studio app.</p>

<p align="center">
  <a href="https://radiomaster.dovafm.com/">Website</a> ·
  <a href="../../releases/latest">Download</a> ·
  <a href="#playlists">Playlists</a> ·
  <a href="#work-zones">Work zones</a> ·
  <a href="#azuracast-station-manager-tools--azuracast-optional">AzuraCast</a>
</p>

![RadioMaster main window](docs/screenshots/main.png)

RadioMaster uses a classic broadcast-automation layout: playlist tabs in the centre, transport,
VU meters and the Last / On Air / Next tiles on top, panels around it, and tools in their own windows.

## Install
Everything is on the [latest release](../../releases/latest) page.

### Windows 10 / 11
1. Download **`RadioMaster-Setup-<version>.exe`** and run it. You can install just for yourself (no admin
   needed) or for all users; you get Start-menu and desktop shortcuts and an uninstaller.
2. Prefer no installer? Download **`RadioMaster-<version>-windows-x64.zip`**, unzip it anywhere (a USB stick
   works) and run `RadioMaster.exe`.

Embedded web panels use the Microsoft Edge WebView2 runtime, which Windows 10/11 already include.

### macOS 12 or later
Download **`RadioMaster-<version>-macos.zip`**, unzip it and drag **RadioMaster.app** to Applications. The app
isn't signed by Apple, so the first time right-click it → **Open** → **Open**. Allow the microphone when asked
if you use the mic for talk-overs.

### Linux (x64)
Download **`RadioMaster-<version>-linux-x64.tar.gz`**, then:

```
tar xzf RadioMaster-<version>-linux-x64.tar.gz
cd RadioMaster-<version>-linux-x64
./RadioMaster
```

It needs ALSA (`libasound2`) and an X11 or Wayland desktop, which desktop distributions already have.

### First start
You create the local administrator account, then add your music with **File → Add library folder…**.
RadioMaster tells you when a new version is out (the button next to the logo); installed copies update
themselves in one click.

### RadioMaster Server
For a station that runs on its own on a Linux or Windows server, download
**`RadioMaster-Server-<version>-linux-x64.tar.gz`** or **`RadioMaster-Server-<version>-windows-x64.zip`**
and follow [Installing RadioMaster Server](#installing-radiomaster-server) below.

## Build from source (Windows)
Double-click **`run-windows.bat`** (or `cargo run --release` in this folder).
**`build-windows.bat`** produces `target\release\RadioMaster.exe` (with the RadioMaster icon).
**`build-installer.bat`** also makes `dist\RadioMaster-Setup-<version>.exe` (needs [Inno Setup 6](https://jrsoftware.org/isdl.php)).

Linux: `sudo apt install build-essential libasound2-dev pkg-config`, then `cargo run --release`.
macOS: install Rust, then `cargo run --release`.

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
| Add event… | runs a scheduler action when playback reaches the row: put another playlist on air, play this one again from the top, generate tracks, adverts/jingles, a file, encoders, volume, a command |
| Add timed pause… (Ctrl+P) | silence for a set time |
| Add teaser… | faded snippets of the next N tracks |
| Add time announcement… | recordings H00…H23 / M00…M59 from a folder, otherwise the computer's speech voice reads your text (`{h12}:{mm} {ampm}`) |
| Add text to speech… (Alt+T) | reads your text, every line: {time}, {next}, {last}, {station}…; voice, speed, pitch, volume, pause between lines, repeat, intro/outro sound |
| Add comment… (Alt+W) | a note row; nothing plays |
| Add container… | several files as one row: all in order, or one at random |
| Insert voice track (Ctrl+W) / Record voice track (Ctrl+R) | recordings go to `voicetracks\` |
| Add Track List… + Track List Repeat Protection… | each time the row is reached, one random track from a folder/M3U that hasn't played recently (track and artist minutes) |
| Insert playlist / Insert tracks from playlist… | insert an M3U after the selection, or pick tracks from it |
| Extra ▸ | hour marker, shuffle, sort, reverse, remove duplicates / played / missing, render WAV, export CSV, clear |
| Notepad… (T), Find… (Ctrl+F) | |

Playlist events: to stop a scheduled show ending in dead air, finish its playlist with an event row "Put a playlist tab on air → Main"; Main carries on where it left off. End Main with "Play the on-air playlist again from the top" to keep it looping.

Hour markers: *wait* (silence until the time if early), *soft* (finish the current track, then jump) or *hard* (cut in on time).
The clock shows the countdown to the next marker.

![Waiting for an hour marker](docs/screenshots/hour-marker.png)

## Work zones
View → Work zones, the **1 2 3** buttons, or **Ctrl+1/2/3** set how many panels the main window has
(1 = playlist + left panel, 2 = + right panel, 3 = + lower-right panel). **Ctrl+4 / ➕🗖** opens an
**Additional** window — a real window with its own tabbed panel. Open as many as you like and put them on other
screens; they reopen where you left them. Each panel's ➕ adds AUX Playlist, Cart wall, Search, Scheduler,
File Explorer, Notepad, Statistics, Song Requests, Playback Queue, Log, Library, Clock, Track Info or a web panel.
Right-click a tab to move it to another panel or into a new Additional window.

Web panels: type an address and give the tab a name. **➕ → New web panel** adds as many as you like. On a fresh
install the first one opens https://radiomanager.dovafm.com.

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

## Adverts and jingles (Tools → Advertisement scheduler)
**Rules** say what plays (an advert / jingle folder, chosen files, or the Adverts list), how often (after every N
songs, every N minutes or hours, or at set times) and when the rule is in force (dates, days, hours). The **Adverts
list** holds your adverts with the dates and days each may run. Rules can also be used in the playlist generator
(as a pattern step or inserted every N minutes) and run by the Event scheduler or the `RULE <name>` command.

## Event scheduler (Tools → Event scheduler, F3)
At a set time (on chosen days and dates): generate a new playlist from a template, append one, play a rule's
adverts / jingles, put a playlist tab on air, load an M3U, play a file, start / stop encoders, set the volume, or
run any station command.

## Streaming formats
MP3 is built in. AAC, Opus, Ogg Vorbis and FLAC are made by [ffmpeg](https://ffmpeg.org/download.html) — put
`ffmpeg` next to RadioMaster, in the settings folder's `ffmpeg` folder, or on the PATH, or choose it in Streams.
**Custom encoder** runs any program you give it: it receives 16-bit 44.1 kHz stereo PCM on stdin and writes the
stream to stdout (`{bitrate}` is replaced with the bitrate).

## Library cleanup (Tools → Library cleanup)
- **Duplicates**: byte-for-byte identical copies, and (optionally) the same song in different files (same artist and
  title, lengths within 3 seconds). The best copy is kept (lossless, then highest bitrate); playlist rows, cue points
  and play counts of the removed copies move to it.
- **Missing & broken**: library tracks and playlist rows whose file has gone are looked for in the library folders
  (and any folder you add) by file name, then by artist and title, and can be pointed at the file found. Files that
  can't be played (empty, damaged, not audio) are listed too.
- **Genres** and **Artists**: spellings of the same value ("Hip-Hop" / "hip hop", "R&B" / "RnB", "Beatles, The" /
  "The Beatles", "ft." / "feat.") are suggested for merging, and any spellings can be merged by hand. Untagged files
  named "Artist - Title" can have their artist filled in.
- Nothing changes until you have looked at the list and pressed **Do it**. Removed files go to the `Removed tracks`
  folder in the data folder unless you choose *delete for good*, and every change is written to `cleanup-log.csv`.

## Reports (Tools → Reports)
Pick **Everything, Songs, Adverts, Jingles** or **Speech, voice & other**, a period (Today, This week, Last month…
or any dates), and optionally only items containing a word. Click an item to see every time it played.
**📄 PDF…** saves a report with totals, plays per day and every item; **⬇ Export** also has CSV (opens in Excel),
every single play, and **proof of play** PDFs — for the selected advert or jingle, or each item on its own page,
with every air time and lines to sign. Plays are read from `history.csv` in the settings folder.

## Microphone
The 🎤 Mic button (Ctrl+M) puts your input on air. Right-click it (or Settings → Audio) to choose what the music
does meanwhile: lowered to the level of the slider next to the mic volume, muted, or paused until the mic goes off.

## Plugins
Small programs in any language that run when something happens on air, and can send commands back.
See [PLUGINS.md](PLUGINS.md) and **Tools → Plugins**.

## Translations
Settings → General → Language. To translate RadioMaster, see [lang/README.md](lang/README.md).

## Users and permissions
File → Users: administrators can add operators and choose what each one may do — playback, jingles,
editing playlists, streaming and settings — and exactly which tools they can open (track editor, generator,
adverts, Audio FX, AzuraCast, reports, history, notepad, plugins, scheduler, voice tracks, library, file explorer,
song requests, web panels). They can also reset passwords and disable or delete accounts.

## RadioMaster Server (headless)
RadioMaster Server runs the station without a window on Linux or Windows Server: playlists, scheduler,
adverts, streaming and, if you want, its own sound cards. Presenters run it from the desktop app with
**File → Connect to Server**. RadioMaster offers to save your local session as a station profile first and
brings it back when you disconnect; while connected, a green button with the station's name sits next to
the logo, and **File → Server settings…** changes the server's name, sound output, password, HTTPS and web
portal. **Connect to Remote Server…** on the login screen signs straight in to a server, without a local
account. The optional **web portal** runs the station from a browser on a port you choose.

### Installing RadioMaster Server
**Linux** (Debian/Ubuntu shown; any x64 distribution with glibc 2.35 or later works):

```
sudo apt install libasound2                 # ALSA, usually there already
tar xzf RadioMaster-Server-<version>-linux-x64.tar.gz
cd RadioMaster-Server-<version>-linux-x64
sudo cp RadioMaster-Server /usr/local/bin/
# run it as its own account, as a service
sudo useradd --system --create-home --home-dir /var/lib/radiomaster radiomaster
sudo usermod -aG audio radiomaster          # only if it should play through a sound card
sudo -u radiomaster RadioMaster-Server --data /var/lib/radiomaster --setup
sudo cp radiomaster-server.service /etc/systemd/system/
sudo systemctl enable --now radiomaster-server
journalctl -u radiomaster-server -f         # the station log
```

**Windows Server:** unzip **`RadioMaster-Server-<version>-windows-x64.zip`** (e.g. to
`C:\RadioMaster-Server`), open a command prompt there and run `RadioMaster-Server.exe --setup`. Then run
`RadioMaster-Server.exe`, or start it at boot with Task Scheduler (*At startup*, *Run whether user is logged on
or not*) or a service wrapper such as NSSM. Allow the port (9259) through Windows Firewall.

`--setup` asks for the station name, the **username and password** the desktop apps sign in with (at least 12
characters), the port, whether to use HTTPS and whether to turn on the web portal (and its port). To script it,
use `--user`, `--name`, `--port`, `--https`, `--portal`, `--portal-port` and the `RADIOMASTER_PASSWORD`
environment variable.

**Sound:** the server plays through its default sound card, or the ones ticked in **File → Server settings…**
in the app. Switch "Play through the server's sound cards" off (or start it with `--no-audio`) for a server
that only streams. With no sound card at all it runs on a silent clock, and streaming works as normal.

**Connect:** in the desktop app, **File → Connect to Server…**, type the server's address (e.g.
`192.168.1.20:9259`), the username and the password. Over the internet, turn on **HTTPS**: give the server
your certificate with `--cert` and `--key` (e.g. from Let's Encrypt), or let it make its own, whose fingerprint
the app asks you to trust once.

**Web portal:** start the server with `--setup --portal` (port 9260 unless you set `--portal-port`), then open
`http://<server>:9260` (or `https://`) in a browser and sign in with the server's username and password.

More detail (options, moving an existing station to a server, security): [docs/SERVER.md](docs/SERVER.md).

## macOS and Linux
Each release also has `RadioMaster-<version>-macos.zip` (RadioMaster.app; unsigned, so right-click → Open the
first time) and `RadioMaster-<version>-linux-x64.tar.gz`. Embedded web panels are Windows-only for now; elsewhere
they open in your browser.

## Contributing
Bug reports and pull requests are welcome — see [CONTRIBUTING.md](CONTRIBUTING.md).

## License
[MIT](LICENSE).

RadioMaster is an independent project. It is not affiliated with or endorsed by DJSoft.net (RadioBOSS) or AzuraCast;
those names are used only to describe compatibility and familiar workflows.
