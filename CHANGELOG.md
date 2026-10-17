# Changelog

## v0.2.3 — 2026-10-17
**New**
- Drag and drop in playlists: reorder rows, drop them on another playlist's tab, or drag songs in from
  the Library and File Explorer
- The playlist follows the on-air track (the last played song stays in view; you can still scroll up)
- Hour lines: "08:00 — new hour" rows are placed automatically where each hour starts (Playlist → Hour lines),
  plus information-only hour markers you can add yourself
- New hour markers go where that time falls in the running order
- Text to speech playlist rows with {next}, {last}, {time} and more; voice and speed; preview
- Song-count rules for adverts and jingles: e.g. every 5 songs play 2 adverts — random, in order, or all,
  from a folder or a hand-picked list
- Stop jingles (Ctrl+J, Jingles menu, or click a playing pad); playing pads light up red
- EQ: exact values under each slider, 10 built-in presets, save your own
- Users and permissions: edit, disable or delete operators, reset passwords, and choose what each may do
  (playback, playlists, tools, streaming, AzuraCast, settings)
- About window with contributors and credits
- Track Info panel tab: last / on air / next with details and your own notes about each song and artist
- Track editor from the Library, stepping through the library list
- Add AzuraCast media straight to the playlist (downloaded and cached)
- Plugins: run your own scripts on events (song started, hour, encoder…) and let them send commands
- Translations: every text can be translated (Settings → General → Language), template included
- Many more keyboard shortcuts (Help → Keyboard shortcuts, Ctrl+/)
- macOS build (RadioMaster.app) and Linux build on every release

**Fixed**
- Web panels: "WebView2 error: Access is denied (0x80070005)" when installed in Program Files
- Streaming: "connection aborted (os error 10053)" — RadioMaster now logs in the classic Icecast way (SOURCE)
  that AzuraCast DJ ports and Icecast both accept, falls back to PUT, explains early disconnects and
  backs off between retries

**Changed**
- The remote control API's default port is now 9258
- The program and installer are published by DovaFM Digital Entertainment Group
- Less redrawing while idle

## v0.2.2 — 2026-10-17
- Update checker: when a newer release is on GitHub, an "Update available" button appears next to the
  RadioMaster logo. Installed copies download the new setup and update themselves (RadioMaster closes,
  updates and starts again); portable copies open the download page.
- Help → Check for updates, and Settings → General → "Check for updates when RadioMaster starts"

## v0.2.1 — 2026-10-17
Performance fixes.

- Additional windows no longer get moved/resized and the layout file rewritten on every frame
  (the main cause of lag on Windows with an Additional window open)
- Turned off vsync so several windows don't each wait for the screen in turn
- Missing-file checks for playlist rows run in the background (fast with OneDrive / network music folders)
- File Explorer reads a folder once instead of every frame and no longer probes every drive letter
- Playlists, cues and play counts are saved on a background thread
- AUX Playlist no longer copies the whole playlist every frame
- Help → Performance (Ctrl+Shift+P) shows frame timings for bug reports

## v0.2.0 — 2026-10-17
First public release.

- Windows installer (`RadioMaster-Setup-0.2.0.exe`) and portable zip

- Multiple playlist tabs (one on air), projected start times, last-played column
- Full Playlist menu: URL relays, Line Input, commands, timed pauses, teasers, time announcements,
  comments, containers, Track Lists with repeat protection, hour markers (wait / soft / hard),
  voice tracks (insert or record), insert playlist / tracks from playlist, Notepad, Find
- Work zones 1/2/3 panels plus unlimited Additional windows for extra screens
- AzuraCast station manager (optional, API key): services, queue, requests, playlists, media,
  streamers/DJs, listeners, history, schedule
- Smart fading and a waveform track editor
- Icecast / Shoutcast encoders, cart wall, mic ducking, multiple outputs, audio processing,
  ad and event schedulers, reports, remote API, embedded web panels (WebView2)
