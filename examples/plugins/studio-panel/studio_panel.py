"""Studio panel: an example RadioMaster plugin (service mode, Python 3).

Shows what a plugin can do besides reading events:
* its own window (a "panel") that it keeps up to date,
* a toolbar button, a Plugins menu item and a playlist right-click item,
* settings in Settings -> Plugins,
* asking RadioMaster for its state ("get") and controlling playback and the playlist.

RadioMaster writes one JSON line per event to standard input. Everything printed that starts with
"RM:" goes back to RadioMaster: a station command, or a JSON message. See PLUGINS.md.
"""
import json
import sys
import threading
import time

settings = {"station": "My Station", "station_id": "", "refresh": 5}
status = {}
message = ""
lock = threading.Lock()


def send(msg):
    """Send RadioMaster a JSON message (or a station command when msg is a string)."""
    line = msg if isinstance(msg, str) else json.dumps(msg)
    with lock:
        print("RM: " + line, flush=True)


def fmt(secs):
    secs = int(secs or 0)
    return f"{secs // 60}:{secs % 60:02d}"


def draw():
    """Send the whole panel again (cheap: RadioMaster keeps what the user is typing)."""
    s = status
    state = s.get("state", "stopped")
    pos, total = s.get("position", 0), s.get("duration", 0)
    send({
        "do": "panel",
        "title": f"{settings.get('station') or 'Studio'} — studio panel",
        "items": [
            {"type": "heading", "text": s.get("on_air") or "Nothing on air"},
            {"type": "label", "text": f"{state.capitalize()}  ·  {fmt(pos)} / {fmt(total)}",
             "color": "green" if state == "playing" else "gray"},
            {"type": "progress", "value": (pos / total) if total else 0},
            {"type": "label", "text": f"Next: {s.get('next') or '—'}", "small": True},
            {"type": "separator"},
            {"type": "row", "items": [
                {"type": "button", "id": "play", "label": "▶ Play", "command": "PLAY"},
                {"type": "button", "id": "pause", "label": "⏸ Pause", "command": "PAUSE"},
                {"type": "button", "id": "next", "label": "⏭ Next", "command": "NEXT"},
                {"type": "button", "id": "station_id", "label": "Station ID next",
                 "tooltip": "Plays the jingle from this plugin's settings after the song on air"},
            ]},
            {"type": "slider", "id": "volume", "label": "Volume", "min": 0, "max": 100,
             "value": s.get("volume", 100)},
            {"type": "checkbox", "id": "stop_after", "label": "Stop after this track",
             "value": bool(s.get("stop_after"))},
            {"type": "separator"},
            {"type": "text", "id": "message", "label": "Message", "hint": "Type and press Enter",
             "value": message},
            {"type": "button", "id": "say", "label": "Show the message in RadioMaster"},
        ],
    })


def poll():
    """Ask for the station's state every few seconds (the answer comes back as a "reply" event)."""
    while True:
        send({"do": "get", "what": "status", "request": "poll"})
        try:
            wait = max(1.0, float(settings.get("refresh") or 5))
        except ValueError:
            wait = 5.0
        time.sleep(wait)


def action(e):
    """Someone used one of our controls."""
    global message
    what, values = e.get("id"), e.get("values", {})
    if what == "open":
        send({"do": "open"})
    elif what == "station_id":
        jingle = settings.get("station_id")
        if jingle:
            send({"do": "insert", "paths": [jingle], "at": "next"})
            send({"do": "notify", "text": "Station ID plays next"})
        else:
            send({"do": "notify", "text": "Pick the station ID jingle in Settings → Plugins first"})
    elif what == "volume":
        send(f"VOLUME {int(e.get('value', 100))}")
    elif what == "stop_after":
        send("STOPAFTER ON" if e.get("value") else "STOPAFTER OFF")
    elif what in ("message", "say"):
        message = values.get("message", "")
        if message:
            send({"do": "notify", "text": message})
    elif what == "shout_out":
        rows = e.get("rows", [])
        if rows:
            message = f"Shout out: {rows[0]['label']}"
            send({"do": "set", "id": "message", "value": message})
            send({"do": "open"})


def main():
    global status
    threading.Thread(target=poll, daemon=True).start()
    for line in sys.stdin:  # ends when RadioMaster closes
        try:
            e = json.loads(line)
        except ValueError:
            continue
        kind = e.get("event")
        if kind == "settings":
            settings.update(e.get("settings", {}))
            draw()
        elif kind == "reply" and e.get("what") == "status":
            status = e.get("data", {})
            draw()
            send({"do": "status", "text": f"On air: {status.get('on_air') or 'nothing'}"})
        elif kind == "action":
            action(e)
        elif kind in ("track_started", "item_started", "playback_stopped", "paused", "resumed"):
            send({"do": "get", "what": "status", "request": "event"})
        elif kind == "mic":
            send({"do": "notify", "text": "Mic is live" if e.get("on") else "Mic off"})


if __name__ == "__main__":
    main()
