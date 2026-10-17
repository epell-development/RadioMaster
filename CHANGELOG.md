# Changelog

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
