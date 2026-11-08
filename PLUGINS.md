# RadioMaster plugins

A plugin is a small program, in any language, that RadioMaster runs alongside the station. It can:

- **react** to what happens on air (a song starts, the mic goes live, the hour changes, an encoder drops…);
- **control** the station: play, pause, skip, set the volume, fire jingles, put files in the playlist,
  switch playlists, start encoders, generate playlists;
- **ask** RadioMaster what is on air, what is in a playlist, the play history, the library;
- **add its own controls** to the app: menu items, toolbar buttons, right-click items on playlist rows,
  windows full of buttons, sliders, text boxes and tables, and settings in Settings → Plugins.

Use plugins to post the current song to Discord, update a website, flip a studio "ON AIR" light,
log to your own database, build a studio panel for your presenters, or control RadioMaster from another system.

Open **Settings → Plugins → Open plugins folder**. RadioMaster creates the folder (and a working example)
inside its data folder (`%APPDATA%\RadioMaster\plugins` on Windows). A bigger example that uses
everything below is in [`examples/plugins/studio-panel`](examples/plugins/studio-panel).

## Making a plugin

Make a folder inside `plugins` with a `plugin.json`:

```json
{
  "name": "Now playing to Discord",
  "description": "Posts every song to our #now-playing channel.",
  "command": "python",
  "args": ["discord.py"],
  "events": ["track_started"],
  "mode": "event"
}
```

| Field | Meaning |
|---|---|
| `name`, `description` | Shown in Settings → Plugins |
| `command`, `args` | The program to run; it starts in the plugin's own folder |
| `events` | Which events it wants (see below), or `["*"]` for all |
| `mode` | `"event"` (default): run once per event. `"service"`: start with RadioMaster and keep running |
| `settings` | Settings shown under the plugin in Settings → Plugins ([below](#settings)) |
| `menu` | Items in the **Plugins** menu ([below](#menu-items-toolbar-buttons-and-playlist-items)) |
| `buttons` | Buttons on the toolbar |
| `playlist_menu` | Items in the right-click menu of playlist rows |
| `panel` | A window of controls ([below](#windows-of-controls)) |

Then press **Reload plugins**. Each plugin can be switched on and off, and **Send a test event**
runs it straight away. Anything the plugin prints shows up in the **Log** tab.

**Event or service?** An event plugin is simplest: it runs, does its job and exits. A service keeps
running, so it can remember things, answer clicks straight away, keep a window up to date and ask
RadioMaster questions. Menu items, buttons and windows work for both kinds, but an event plugin starts
afresh for every click.

## Events

| Event | When | Extra fields |
|---|---|---|
| `app_start` | RadioMaster started | `version` |
| `app_exit` | RadioMaster is closing | |
| `track_started` | a song, voice track or jingle from the playlist started | `title`, `artist`, `album`, `genre`, `year`, `path`, `duration`, `text` |
| `item_started` | another playlist row started (stream relay, line input, speech, teaser…) | `title`, `text` |
| `playback_stopped` | playback stopped | |
| `paused` | playback was paused | |
| `resumed` | playback carried on after a pause | |
| `cart` | a pad on the cart wall was fired | `pad` (1 = the first), `label`, `title`, `artist`, `path` |
| `mic` | the microphone was switched on or off | `on` (true/false) |
| `hour` | the top of every hour | `hour` |
| `encoder` | a stream encoder connected, failed or stopped | `encoder`, `name`, `state` |

These go to one plugin only, whatever its `events` list says:

| Event | When | Fields |
|---|---|---|
| `action` | someone used the plugin's menu item, button, playlist item or window control, or a `PLUGIN` command named it | `id`, `source`, `values`, and `value` / `rows` / `playlist` (see below) |
| `reply` | the answer to a `get` message | `request`, `what`, `data` |
| `settings` | a service's settings: its first line, and again whenever they change | `settings` |

Every event also has `event` and `time` (ISO 8601).

**Event plugins** get the event as one line of JSON on standard input, and as environment variables:
`RM_EVENT`, `RM_TITLE`, `RM_ARTIST`, `RM_PATH`, … (each field in capitals with `RM_` in front).
Their settings are in the JSON as `settings`, and in `RM_SETTING_<ID>` variables.
A run that takes longer than 60 seconds is stopped.

**Service plugins** get one JSON line on standard input per event, for as long as RadioMaster runs.
The first line is always a `settings` event. Their settings are also in `RM_SETTING_<ID>` variables
when they start. When RadioMaster closes, standard input is closed — exit when that happens.

## Talking back to RadioMaster

A plugin talks to RadioMaster by printing lines that start with `RM:`. Print one line per message and
flush after each one (Python: `print(..., flush=True)`).

### Station commands

`RM:` followed by a station command runs it — the same commands as playlist "command" rows:

```
RM: PLAY
RM: STOP
RM: NEXT
RM: VOLUME 60
RM: CART 3
RM: INSERT C:\Jingles\station-id.mp3
RM: PLAYLIST Evening
RM: ENCODER START 1
```

| Command | Does |
|---|---|
| `PLAY`, `PAUSE`, `RESUME`, `STOP` | Play (or carry on), pause, carry on after a pause, stop |
| `NEXT`, `PREV` | Next or previous row |
| `SEEK 30` | Jump to 30 seconds into the track on air |
| `STOPAFTER ON` / `STOPAFTER OFF` | Stop (or don't) when the item on air ends |
| `VOLUME 80` | Set the volume, 0 to 100 |
| `CART 1` | Fire cart pad 1 (press again to stop it) |
| `INSERT <file>` / `ADD <file>` | Play a file after the item on air / add it at the end of the playlist |
| `LOAD <file.m3u>` | Load an M3U playlist and carry on from its first row |
| `PLAYLIST Evening` | Put the playlist tab called Evening on air |
| `GENERATE <template>` / `APPEND <template>` | Make a new playlist from a template / add a template's tracks after the playlist |
| `RULE <name>` | Queue an advert or jingle rule's files after the song on air |
| `ENCODER START 1` / `ENCODER STOP 1` | Start or stop an encoder (1 = the first) |
| `PLUGIN <plugin> <action>` | Send a plugin an `action` event (see below) |
| `RUN <program and arguments>` | Start a program |

### JSON messages

`RM:` followed by a JSON object is a message. Its `"do"` says what it is:

| Message | Does |
|---|---|
| `{"do": "notify", "text": "Hello"}` | Show a line in the status bar (and the Log) |
| `{"do": "log", "text": "…"}` | Write a line in the Log only |
| `{"do": "status", "text": "Connected to Discord"}` | A short line shown under the plugin in Settings → Plugins |
| `{"do": "command", "text": "VOLUME 60"}` | Run a station command (the same as `RM: VOLUME 60`) |
| `{"do": "insert", "paths": ["C:\\a.mp3"], "at": "next"}` | Put files in the playlist: `"at": "next"` (after the item on air, the default) or `"end"`. Add `"playlist": "Evening"` for another playlist tab |
| `{"do": "get", "what": "status", "request": 1}` | Ask for something; the answer comes back as a `reply` event with the same `request` (services only) |
| `{"do": "menu", "items": […]}` | Replace the plugin's Plugins menu items |
| `{"do": "buttons", "items": […]}` | Replace its toolbar buttons |
| `{"do": "playlist_menu", "items": […]}` | Replace its playlist right-click items |
| `{"do": "panel", "title": "…", "items": […], "open": true}` | Replace its window's controls (and open or close it) |
| `{"do": "set", "id": "now", "text": "…"}` | Change some fields of one control in the window |
| `{"do": "open"}` / `{"do": "close"}` | Open or close its window |

What `get` can ask for:

| `what` | `data` in the reply |
|---|---|
| `status` | `state` (playing / paused / stopped), `on_air`, `title`, `artist`, `album`, `path`, `position`, `duration`, `volume` (0–100), `playlist`, `row`, `next`, `stop_after`, `shuffle`, `repeat_track`, `loop` |
| `playlist` | the playlist on air (or `"playlist": "name"`): `name`, `on_air`, `current`, and `rows` with `row`, `title`, `artist`, `label`, `path`, `kind`, `duration`, `played` |
| `playlists` | every playlist tab: `name`, `on_air`, `rows` |
| `history` | the last 100 plays, newest first: `time`, `text`, `kind` |
| `library` | up to 200 library tracks matching `"search": "words"` |
| `carts` | the cart wall pads: `pad`, `label`, `path` |
| `encoders` | `encoder`, `name`, `state` |
| `templates`, `rules` | generator template names, advert / jingle rule names |

## Menu items, toolbar buttons and playlist items

```json
"menu": [
  { "id": "open", "label": "Show the studio panel" },
  { "id": "news", "label": "Play the news jingle", "command": "RULE News" }
],
"buttons": [
  { "id": "station_id", "label": "Station ID", "tooltip": "Play the station ID next" }
],
"playlist_menu": [
  { "id": "shout_out", "label": "Shout out this song" }
]
```

Menu items appear in a **Plugins** menu in the menu bar (it shows up once a plugin adds something),
buttons on the toolbar, and playlist items at the bottom of a playlist row's right-click menu.

An item with a `command` runs that station command. Any other item sends the plugin an `action` event:

```json
{ "event": "action", "id": "station_id", "source": "button", "values": {} }
```

`source` is `menu`, `button`, `playlist`, `panel` or `command`. Playlist items also get the selected rows:

```json
{ "event": "action", "id": "shout_out", "source": "playlist", "playlist": "Main",
  "rows": [{ "row": 4, "label": "Artist - Title", "title": "Title", "artist": "Artist", "path": "C:\\Music\\song.mp3" }] }
```

The `PLUGIN` station command sends an action too, so playlist command rows and the event scheduler
can call a plugin: `PLUGIN Studio panel refresh` sends `{"event": "action", "id": "refresh", "source": "command"}`
to the plugin whose name (or folder name) is "Studio panel".

## Windows of controls

A plugin can have one window. Put it in `plugin.json` (with `"open": true` to open it at start-up), or
send it at any time with `{"do": "panel", …}`; sending it again replaces the controls, so a service can
keep it up to date (what someone is typing isn't lost). The window can be opened from the Plugins menu
and from Settings → Plugins.

```json
"panel": {
  "title": "Studio panel",
  "open": true,
  "items": [
    { "type": "heading", "text": "Now playing" },
    { "type": "label", "id": "now", "text": "…", "color": "green" },
    { "type": "progress", "id": "pos", "value": 0.4, "text": "1:23 / 3:30" },
    { "type": "row", "items": [
      { "type": "button", "id": "next", "label": "Next", "command": "NEXT" },
      { "type": "button", "id": "id", "label": "Station ID" }
    ]},
    { "type": "slider", "id": "volume", "label": "Volume", "min": 0, "max": 100, "value": 80 },
    { "type": "text", "id": "message", "label": "Message", "hint": "Type and press Enter" }
  ]
}
```

| `type` | Fields | Sends an `action` when… |
|---|---|---|
| `heading` | `text` | |
| `label` | `text`, `color` (red, green, amber, gray, blue or `#rrggbb`), `bold`, `small`, `mono` | |
| `separator`, `space` | `space`: `size` | |
| `button` | `id`, `label`, `tooltip`, `command` | clicked (or runs `command`) |
| `text` | `id`, `label`, `value`, `hint`, `multiline`, `password` | Enter is pressed |
| `number` | `id`, `label`, `value`, `min`, `max`, `step` | changed |
| `slider` | `id`, `label`, `value`, `min`, `max` | let go |
| `checkbox` | `id`, `label`, `value`, `tooltip` | ticked or unticked |
| `choice` | `id`, `label`, `options` (list of text), `value` | another option is picked |
| `progress` | `value` (0 to 1), `text` | |
| `link` | `text`, `url` | |
| `table` | `columns` (list of text), `rows` (list of lists), `id` | the first cell of a row is clicked (`value` = row number) |
| `row` | `items` (controls side by side) | |
| `group` | `title`, `items`, `collapsed` | |

Every `action` from the window carries `value` (the control's new value) and `values`: the current value
of every input in the window, by `id`, so a "Send" button can read the text boxes next to it. When the
window is closed the plugin gets an action with `"id": "panel_closed"`.

## Settings

```json
"settings": [
  { "id": "webhook", "label": "Discord webhook", "type": "password" },
  { "id": "channel", "label": "Channel", "default": "now-playing" },
  { "id": "delay", "label": "Delay (seconds)", "type": "number", "default": 5 },
  { "id": "covers", "label": "Post album covers", "type": "bool", "default": true },
  { "id": "style", "label": "Style", "type": "choice", "options": ["Short", "Long"], "default": "Short",
    "help": "Shown when the mouse is over the setting" }
]
```

Types: `text` (the default), `password`, `number`, `bool`, `choice`. Each plugin's settings show under
it in Settings → Plugins and are saved with RadioMaster's settings. Plugins get them as described in
[Events](#events).

## Examples

**Python** — write the current song to a file for OBS:

```python
import json, sys
e = json.loads(sys.stdin.readline())
open("nowplaying.txt", "w", encoding="utf-8").write(f"{e['artist']} - {e['title']}")
```

**PowerShell** — post to a Discord webhook from the plugin's settings:

```powershell
$e = [Console]::In.ReadLine() | ConvertFrom-Json
$body = @{ content = "Now playing: $($e.artist) - $($e.title)" } | ConvertTo-Json
Invoke-RestMethod -Uri $e.settings.webhook -Method Post -Body $body -ContentType "application/json"
```
(`"command": "powershell", "args": ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "discord.ps1"]`,
`"settings": [{ "id": "webhook", "label": "Discord webhook", "type": "password" }]`)

**Service** (Python) — a toolbar button that plays a random jingle from a folder:

```python
import json, os, random, sys

def send(msg):
    print("RM: " + (msg if isinstance(msg, str) else json.dumps(msg)), flush=True)

send({"do": "buttons", "items": [{"id": "jingle", "label": "Random jingle"}]})
for line in sys.stdin:
    e = json.loads(line)
    if e["event"] == "settings":
        folder = e["settings"].get("folder", "")
    elif e["event"] == "action" and e["id"] == "jingle":
        files = [os.path.join(folder, f) for f in os.listdir(folder)] if folder else []
        if files:
            send({"do": "insert", "paths": [random.choice(files)], "at": "next"})
        else:
            send({"do": "notify", "text": "Pick a jingle folder in Settings → Plugins"})
```
(`"mode": "service"`, `"settings": [{ "id": "folder", "label": "Jingle folder" }]`)

**Service** (Python) — drop the volume at the top of every hour:

```python
import json, sys
for line in sys.stdin:
    e = json.loads(line)
    if e["event"] == "hour":
        print("RM: VOLUME 70", flush=True)
```

The [studio panel example](examples/plugins/studio-panel) shows a window kept up to date with `get`,
a toolbar button, menu items, a playlist right-click item and settings.

## Good to know

- Menu items, buttons and windows from plugins need the **Plugins** tool to be allowed for the
  operator who is signed in (Settings → Users & access control).
- While the app is connected to a RadioMaster Server, the server runs its own plugins: this
  computer's plugin menus, buttons and windows come back when you disconnect.
- Plugins run with your account's rights — only install ones you trust.
