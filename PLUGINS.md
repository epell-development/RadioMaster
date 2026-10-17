# RadioMaster plugins

A plugin is a small program, in any language, that RadioMaster runs when something happens on air.
Use plugins to post the current song to Discord, update a website, flip a studio "ON AIR" light,
log to your own database, or control RadioMaster from another system.

Open **Tools → Plugins → Open plugins folder**. RadioMaster creates the folder (and a working example)
inside its data folder (`%APPDATA%\RadioMaster\plugins` on Windows).

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
| `name`, `description` | Shown in the Plugins window |
| `command`, `args` | The program to run; it starts in the plugin's own folder |
| `events` | Which events it wants (see below), or `["*"]` for all |
| `mode` | `"event"` (default): run once per event. `"service"`: start with RadioMaster and keep running |

Then press **Reload plugins**. Each plugin can be switched on and off, and **Send a test event**
runs it straight away. Anything the plugin prints shows up in the **Log** tab.

## Events

| Event | When | Extra fields |
|---|---|---|
| `app_start` | RadioMaster started | `version` |
| `app_exit` | RadioMaster is closing | |
| `track_started` | a song, voice track or jingle from the playlist started | `title`, `artist`, `album`, `genre`, `year`, `path`, `duration`, `text` |
| `item_started` | another playlist row started (stream relay, line input, speech, teaser…) | `title`, `text` |
| `playback_stopped` | playback stopped | |
| `hour` | the top of every hour | `hour` |
| `encoder` | a stream encoder connected, failed or stopped | `encoder`, `name`, `state` |

Every event also has `event` and `time` (ISO 8601).

**Event plugins** get the event as one line of JSON on standard input, and as environment variables:
`RM_EVENT`, `RM_TITLE`, `RM_ARTIST`, `RM_PATH`, … (each field in capitals with `RM_` in front).
A run that takes longer than 60 seconds is stopped.

**Service plugins** get one JSON line on standard input per event, for as long as RadioMaster runs.
When RadioMaster closes, standard input is closed — exit when that happens.

## Controlling RadioMaster

Print a line starting with `RM:` and RadioMaster runs it as a station command — the same commands
as playlist "command" rows:

```
RM: STOP
RM: VOLUME 60
RM: PLAYLIST Evening
RM: ENCODER START 1
RM: LOAD C:\Shows\news.m3u
```

## Examples

**Python** — write the current song to a file for OBS:

```python
import json, sys
e = json.loads(sys.stdin.readline())
open("nowplaying.txt", "w", encoding="utf-8").write(f"{e['artist']} - {e['title']}")
```

**PowerShell** — post to a Discord webhook:

```powershell
$e = [Console]::In.ReadLine() | ConvertFrom-Json
$body = @{ content = "Now playing: $($e.artist) - $($e.title)" } | ConvertTo-Json
Invoke-RestMethod -Uri "https://discord.com/api/webhooks/…" -Method Post -Body $body -ContentType "application/json"
```
(`"command": "powershell", "args": ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "discord.ps1"]`)

**Service** (Python) — drop the volume at the top of every hour:

```python
import json, sys
for line in sys.stdin:
    e = json.loads(line)
    if e["event"] == "hour":
        print("RM: VOLUME 70", flush=True)
```

Plugins run with your Windows account's rights — only install ones you trust.
