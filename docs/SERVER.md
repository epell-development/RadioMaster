# RadioMaster Server

RadioMaster Server is RadioMaster without a window, for Linux and Windows Server. It plays the playlists,
runs the scheduler, adverts and jingles, and streams to Icecast/Shoutcast on its own. Presenters run it
from the desktop app with **File → Connect to Server**.

## Download
Each release has:
- **`RadioMaster-Server-<version>-linux-x64.tar.gz`** — `RadioMaster-Server`, this guide and a systemd unit.
  Needs `libasound2` (ALSA), which nearly every distribution has.
- **`RadioMaster-Server-<version>-windows-x64.zip`** — `RadioMaster-Server.exe` and this guide.

## Set it up
Run the setup once. It asks for the station name, the **username and password** desktop apps sign in with
(at least 12 characters), and the port (9259 unless you choose another):

```
RadioMaster-Server --setup
```

Without a terminal (a script, a container), give the answers as options. The password can come from the
`RADIOMASTER_PASSWORD` environment variable so it stays out of your shell history:

```
RADIOMASTER_PASSWORD='a long password' RadioMaster-Server --setup --user studio --name "DovaFM"
```

Then start it:

```
RadioMaster-Server
```

The station log goes to the console. Ctrl+C (or stopping the service) saves everything and stops the
encoders. Run `RadioMaster-Server --help` for every option.

| Option | What it does |
|---|---|
| `--setup` | set the station name, username, password and port, then exit |
| `--user`, `--password`, `--name` | answers for `--setup` |
| `--port N`, `--bind ADDRESS` | where to listen (default `0.0.0.0:9259`) |
| `--data FOLDER` | the data folder (default `~/.config/RadioMaster-Server`, or `%APPDATA%\RadioMaster-Server` on Windows) |
| `--no-audio` | don't use a sound card: stream only |
| `--list-devices` | list the sound cards |

The server keeps its own data folder, so it never mixes with a desktop RadioMaster on the same machine. To
move an existing station to the server, copy `config.json`, `playlists.json`, `library.json`, `cues.json` and
`plays.json` from the desktop's data folder into the server's (then fix any music folder paths).

**Sound output:** the server can play through its own sound cards (e.g. into a desk or an FM transmitter) as
well as stream. By default it uses its default sound card. In the desktop app, **File → Server settings…**
switches "Play through the server's sound cards" on or off and ticks which cards to use; the change takes
effect at once and is shown with each card's status. With no sound card (or with it switched off, or with
`--no-audio`) the server runs on a silent clock, so automation and streaming carry on as normal.

**File → Server settings…** also changes the station name, whether the server starts playing when it starts,
and the server password (you type the current one first).

### Run it as a service
**Linux (systemd):** the package includes `radiomaster-server.service`; the steps are at the top of that file.
`journalctl -u radiomaster-server -f` shows the station log.

**Windows Server:** run `RadioMaster-Server.exe --setup` once in a console, then start it at boot, for
example with Task Scheduler (*At startup*, *Run whether user is logged on or not*) or a service wrapper such as
NSSM. Allow the port through Windows Firewall.

## Connect from the desktop app
1. **File → Connect to Server…**, type the server's address (e.g. `192.168.1.20:9259`), the username and the
   password.
2. RadioMaster asks whether to **save your local session** as a station profile. If you save it, your own
   playlists and settings come back exactly as they were when you disconnect. If you don't, the server's
   playlists and settings stay in the app after you disconnect.
3. While connected, a green button with the station's name sits left of the RadioMaster logo (amber while
   the connection is being restored). Click it for the details or to disconnect; **File → Disconnect from
   Server** does the same.

While connected, everything you do in the app happens on the server: playlist edits, play/stop/next, carts,
encoders, the scheduler, adverts, the generator and settings. Your own playback and encoders stop while you're
connected (one station on air at a time) and come back on disconnect. Several apps can be connected at once;
if two people change the same playlist at the same moment, the server's version wins and the app says so.

**Hearing the station and going live:** while connected, the app plays the server's on-air mix on your own
outputs (Settings → Audio, including Studio monitor outputs), about half a second behind the server. Untick
**Hear the station here** in the server connection window (click the green button) when you're in the same room
as the server's speakers. The **Mic** button sends your microphone to the server, which puts it on air with the
station's mic settings (lower, mute or pause the music); Studio monitor outputs on both machines go quiet while
it is on. Wear headphones rather than speakers, and expect to hear your own voice come back slightly late.

Things that stay on your computer: the interface language, sound cards, the remote-control API, plugins,
update checks and ffmpeg. The music files, folders and library are the server's, so a library folder is typed
as the server sees it (e.g. `/srv/music`). Previews and waveforms in the fade editor need the files on your own
computer and don't work while connected.

## Security
The username and password are checked with Argon2 (only the hash is stored, in `server.json`), sign-ins
expire after a day without use, and repeated wrong passwords are slowed down. Whoever signs in can do everything the station can, including playlist
`RUN` commands that start programs on the server, so use a strong password and run the server under its own
unprivileged account (the systemd unit does). The server speaks plain HTTP:
on a network you don't trust, or over the internet, put it behind an HTTPS reverse proxy (Caddy, nginx) or a
VPN, and connect to its `https://` address. With nginx, switch response buffering off (`proxy_buffering off;`)
so the station's sound reaches the app straight away.
