# Changelog

## v0.2.4 — 2026-10-18
**New**
- Reports: choose everything, songs, adverts, jingles or speech for any period (today, this week, last
  month…), click an item to see every time it played, and export:
  - a PDF report (totals, plays per day, every item)
  - proof of play as a PDF — for one advert or jingle, or each item on its own page — with every air time
  - CSV (opens in Excel) for the report or for every single play
- The play history now records what each item was (song, advert, jingle, speech…), and jingles fired
  from the cart wall are included
- Library: select several tracks (Ctrl+click, Shift+click) and drag them into the playlist; right-click
  works on the whole selection
- Track editor: a list of every track in the library (with search) or the playlist beside the editor —
  click any track to edit it next
- Text to speech: every line is read (with a pause you can set between lines), plus pitch, volume,
  read it several times, an intro / outro sound, and reading numbers digit by digit; Stop button for
  the preview; speech is prepared just before it is due so it starts without a gap
- Microphone: choose what the music does while the mic is on — lower it, mute it, or pause it and carry
  on afterwards (right-click the Mic button, or Settings → Audio); the music fades down and up smoothly
- Web panels: add as many as you like (➕ → New web panel); the first one opens
  https://radiomanager.dovafm.com
- Permissions: administrators pick exactly which tools each user can open (track editor, generator,
  adverts, Audio FX, AzuraCast, reports, history, notepad, plugins, scheduler, voice tracks, library,
  file explorer, song requests, web panels), plus a separate Jingles permission
- Characters the built-in fonts lack (other alphabets, symbols in song titles) now use the computer's
  own fonts instead of showing squares

**Fixed**
- Text to speech rows stopped after the first line on air (the pause between lines was taken for the
  end of the item)
- Dragging songs from the library or file explorer into the playlist did nothing
- The line that shows where dragged rows will land now sits exactly between two rows
- Users who may not edit playlists could still add songs (Library → Insert as next, AzuraCast,
  file explorer, search, dropped files)
- Icons that showed as squares (Additional window button, web panel Home, AzuraCast live DJ)
- The track editor no longer grows taller than the screen on small displays; its zoom buttons no longer
  overlap the file details

**Website**
- Cleaner addresses (…/RadioMaster/updates/ instead of updates.html) and clearer Windows, macOS and
  Linux downloads

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
