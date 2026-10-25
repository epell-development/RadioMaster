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
| `--https`, `--no-https` | serve over HTTPS (see [HTTPS](#https)) |
| `--cert FILE`, `--key FILE` | a PEM certificate (chain) and its private key for HTTPS; without them the server makes its own |
| `--portal`, `--no-portal`, `--portal-port N` | the [web portal](#web-portal) and its port (default 9260) |
| `--no-audio` | don't use a sound card: stream only |
| `--list-devices` | list the sound cards |

With `--setup` these options are saved in `server.json`; without it they apply to that run only.

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

## HTTPS
Turn HTTPS on with `--setup --https` (setup also asks), or in the app with **File → Server settings… → Use
HTTPS** (it takes effect when the server restarts). Everything between the apps, browsers and the server is
then encrypted, and the port answers plain `http://` only with a pointer to `https://`.

- **Your own certificate** (for example Let's Encrypt's): `--cert /etc/letsencrypt/live/radio.example.com/fullchain.pem --key /etc/letsencrypt/live/radio.example.com/privkey.pem`. The
  app and browsers trust it like any website. The server account must be able to read both files; restart the
  server after the certificate is renewed.
- **No certificate:** the server makes a self-signed one the first time (`tls-cert.pem` and `tls-key.pem` in
  its data folder) and prints its SHA-256 fingerprint at start-up. The first time an app connects, it shows
  the fingerprint and asks you to trust it; check it against the one the server printed (it is also in
  **Server settings**). The app remembers it for that server and refuses a different certificate later, warning
  that it changed. Browsers show a warning for a self-signed certificate; use your own certificate to avoid it.

An address typed without `https://` still finds a server that uses HTTPS.

## Web portal
The web portal runs the station from a browser on any computer, tablet or phone, with nothing to install.
Turn it on with `--setup --portal` (choose the port with `--portal-port`, default 9260), or in the app with
**File → Server settings… → Web portal** (after a restart). Open `http://<server>:9260` (or `https://` with
HTTPS on) and sign in with the server's username and password.

It shows what's on air with its progress and levels, play / pause / stop / next / previous, the volume, the
playlists (double-click a row to play it), the streams with start and stop, recently played and the station
log. The portal needs a port of its own; allow it through the firewall like the main port.

## Connect from the desktop app
**From the login screen:** **Connect to Remote Server…** below the sign-in box connects without a local
account: you sign in with the server's username and password. Your local session is saved as the profile
"Local session before remote sign-in" and comes back when you disconnect, which (like **Log out**) returns to
the login screen. Managing local user accounts isn't available this way.

**From the app:**

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
unprivileged account (the systemd unit does). On a network you don't trust, or over the internet, turn on
[HTTPS](#https) (or put the server behind an HTTPS reverse proxy such as Caddy or nginx, or a VPN). Without it
the password and everything else go over the network unencrypted. The web portal uses the same sign-in, and
its pages refuse to be shown inside other sites. With nginx, switch response buffering off
(`proxy_buffering off;`) so the station's sound reaches the app straight away.
