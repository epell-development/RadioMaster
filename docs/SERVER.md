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
| `--acme DOMAINS`, `--acme-email`, `--acme-staging`, `--acme-http-port N`, `--no-acme` | a free [Let's Encrypt certificate](#lets-encrypt) for these domain names |
| `--portal`, `--no-portal`, `--portal-port N` | the [web portal](#web-portal) and its port (default 9260) |
| `--tunnel`, `--no-tunnel`, `--tunnel-token TOKEN`, `--cloudflared FILE` | put the portal on the internet with a [Cloudflare Tunnel](#cloudflare-tunnel) |
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
  app and browsers trust it like any website. The server account must be able to read both files. When the
  files are renewed (certbot, win-acme…), the server picks up the new certificate within a few minutes, without
  a restart.
- **A free certificate from Let's Encrypt, looked after by the server:** see [Let's Encrypt](#lets-encrypt).
- **No certificate:** the server makes a self-signed one the first time (`tls-cert.pem` and `tls-key.pem` in
  its data folder) and prints its SHA-256 fingerprint at start-up. The first time an app connects, it shows
  the fingerprint and asks you to trust it; check it against the one the server printed (it is also in
  **Server settings**). The app remembers it for that server and refuses a different certificate later, warning
  that it changed. Browsers show a warning for a self-signed certificate; use your own certificate to avoid it.

An address typed without `https://` still finds a server that uses HTTPS.

### Let's Encrypt
The server can get its own certificate from Let's Encrypt and renew it by itself (30 days before it runs out),
so browsers and apps trust it with no warning. You need a domain name (e.g. `radio.example.com`) that points
at the server, and **port 80** of that name has to reach the server (forward it on your router), because
Let's Encrypt checks the name by fetching a file from `http://radio.example.com/.well-known/acme-challenge/…`.

```
RadioMaster-Server --setup --acme radio.example.com --acme-email you@example.com
```

or in the app: **File → Server settings… → Use HTTPS → Get a free certificate from Let's Encrypt**, with the
domain names (comma-separated) and an optional email. Until the first certificate arrives the server uses its
self-signed one; the new one is swapped in without a restart, and the settings window shows how it went and
when the certificate runs out. Tick *staging* to try it out first (Let's Encrypt limits how often real
certificates can be asked for; staging certificates aren't trusted by browsers).

On Linux, listening on port 80 needs root or `sudo setcap cap_net_bind_service=+ep RadioMaster-Server`. Port
80 also sends browsers to the `https://` portal. If something else (IIS, nginx) already uses port 80, pass
`/.well-known/acme-challenge/` on to the server's port and start it with `--acme-http-port 0`; or let that
program get the certificate (win-acme, certbot) and give the files to the server with `--cert` and `--key`.

### Behind a reverse proxy (IIS, nginx, Caddy)
A proxy that serves HTTPS itself can talk to the server over plain HTTP. Either:

- turn the server's HTTPS **off** and let the proxy do it (simplest), or
- keep it on: plain requests from a proxy on the same machine or the local network that says the browser used
  HTTPS (`X-Forwarded-Proto: https`, IIS's `X-ARR-SSL`, Cloudflare's `CF-Visitor`) are served as they are instead
  of being sent to `https://`. (From other addresses those headers are ignored, so they can't be faked.)
  (Before 0.2.9 that redirect sent browsers round in a loop, `ERR_TOO_MANY_REDIRECTS`.)

**IIS** (URL Rewrite + Application Request Routing): add a reverse-proxy rule to `http://localhost:9260` for
the portal (or 9259 for the apps), keep *Reverse rewrite host in response headers* off, and set the ARR proxy's
*Response buffer threshold* to 0 so the station's sound reaches the app straight away. **nginx:** set
`proxy_buffering off;` and `proxy_set_header X-Forwarded-Proto $scheme;`. More examples (nginx, Caddy, IIS) are in
the [online documentation](https://radiomaster.dovafm.com/server/docs/#proxy).

### Cloudflare Tunnel
A Cloudflare Tunnel puts the web portal on the internet with no port forwarding and no certificate to look
after. Install Cloudflare's `cloudflared` on the server (or put it next to `RadioMaster-Server`), then either:

- **Your own domain:** in the Cloudflare dashboard, *Zero Trust → Networks → Tunnels → Create a tunnel*, add a
  public hostname (e.g. `radio.example.com`) with the service `http://localhost:9260`, and copy the tunnel's
  token. Start the server with `--tunnel-token <token>` (or set `CLOUDFLARE_TUNNEL_TOKEN`), or paste it in
  **Server settings → Cloudflare Tunnel**.
- **Just try it:** `--tunnel` with no token gives a random `https://….trycloudflare.com` address (it changes
  each time the tunnel starts); it is printed in the log and shown in Server settings.

The server starts `cloudflared`, starts it again if it stops, and stops it on exit. Switching the tunnel on
or off in Server settings takes effect at once. The desktop app can connect through the tunnel address too.

## Web portal
The web portal runs the station from a browser on any computer, tablet or phone, with nothing to install.
Turn it on with `--setup --portal` (choose the port with `--portal-port`, default 9260), or in the app with
**File → Server settings… → Web portal** (after a restart). Open `http://<server>:9260` (or `https://` with
HTTPS on) and sign in with the server's username and password.

It works on phones (the sections sit along the bottom of the screen) as well as computers:

- **Studio:** what's on air with its progress and levels, play / pause / stop / next / previous, the volume, the
  playlists (double-click a row to play it, ✕ to take it out) and the streams with start and stop.
- **Library:** search the server's library and add songs to any playlist, at the end or to play next; rescan.
- **Carts:** the cart wall; tap a pad to play it, or *Edit pads* to choose each pad's sound and name.
- **Generator:** make a playlist from a template, keep the on-air playlist filled, and edit the templates and
  categories (folders are the server's).
- **Adverts:** the advert and jingle rules: switch them on and off, play one now, add and edit them.
- **AzuraCast:** the station's now playing and listeners, skip, restart, start and stop broadcasting and the
  AutoDJ, switch AzuraCast playlists on and off, and the connection settings (the calls go
  through the server, with the API key from Settings → AzuraCast).
- **Log:** recently played and the station log.

A banner says when a newer RadioMaster release is out than the server runs. The portal needs a port of its
own; allow it through the firewall like the main port.

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
