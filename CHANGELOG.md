# Changelog

## v0.2.10 — 2026-10-26
**Fixed**
- The app froze when connecting to a RadioMaster Server, and when opening a file or folder picker while
  connected (v0.2.9). Only the app needs this update; servers on v0.2.9 are fine

## v0.2.9 — 2026-10-26
**New**
- Playlist generator: inserts after N songs, every N minutes (on the clock) or at set times, with hour
  limits; time announcements as inserts; different patterns for times of day; "until hh:mm" length; song
  separation; categories with several genres, years and excluded words; a preview before the running
  order goes in
- Event scheduler: insert any playlist item (time announcement, speech, Track List, stream…), repeat
  events every N minutes or hours until a time, an Hourly time announcement button and Duplicate
- Equalizer: a Parametric mode with up to 16 bands (bell, shelf, cut, notch) next to the 12-band graphic
  EQ, a response curve with draggable handles, a Preamp, and many more presets; save your own in either
  mode
- Help → Shortcuts and MIDI: change any keyboard shortcut, add your own shortcuts that run playlist
  commands, and bind actions (and Volume to a fader) to a MIDI controller with Learn
- RadioMaster Server has its own user accounts with permissions; administrators manage them with File →
  Server users, and the server checks every action
- While connected to a server, file and folder pickers (generator categories, the cart wall, adverts,
  events, library folders…) browse the server's folders, with Upload from this computer
- The file explorer can open in its own window (Tools → File explorer) and show all files; it works with
  the server
- The app says when the server is behind, and administrators can update, restart or stop it from the app
- Station profiles load onto the server with their folders mapped to the server's; profiles can be
  imported
- RadioMaster Server: free HTTPS certificates from Let's Encrypt, renewed on their own; certificates given
  with `--cert`/`--key` are picked up again when they're renewed
- RadioMaster Server: optional Cloudflare Tunnel for the web portal, with your own domain or a quick
  trycloudflare.com address
- Web portal: library, cart wall, playlist generator, advert and jingle rules, and AzuraCast station
  management; works on phones; says when the server is behind an update
- Settings → Appearance: choose how the interface size applies on other screens (follow display scaling,
  same size in pixels, or fit each screen like 1920×1080)

**Fixed**
- Connected to a server: playlist rows were all red; now they're red only when the file is missing on the
  server
- Connected to a server: the track editor stayed on "analysing…"; it now shows the waveform and cues
  (worked out on the server), and says why when a file can't be analysed
- Connected to a server: clocks were out of step; every clock now shows the server's time, and
  elapsed/remaining tick together with it
- `ERR_TOO_MANY_REDIRECTS` with HTTPS on behind IIS or another reverse proxy
- The web portal showed its desk for a moment before signing in, and now has the RadioMaster icon
- Keyboard shortcuts match exactly, so Ctrl+Shift+P no longer also fires Ctrl+P

**Changed**
- Less work per frame: start times are only worked out for the rows in view, library search is cheaper,
  and smart-fade analysis is about 15% faster
- Reports use the server's play history while connected; dropped files and recorded voice tracks are
  uploaded to the server
- Website: new guides, server docs for Let's Encrypt, Cloudflare Tunnel and the portal, and a mobile menu
- More checks before every release (clippy, a server smoke test, version and website checks, a weekly
  security audit)

## v0.2.8 — 2026-10-25
**New**
- HTTPS for RadioMaster Server (`--https`): use your own certificate (e.g. Let's Encrypt) or let the
  server make a self-signed one, which the app shows by its fingerprint and trusts once you agree. The
  app warns if the certificate changes later
- Web portal for RadioMaster Server (`--portal`, port 9260 by default): run the station from a browser,
  including on a phone, with what's on air, the transport buttons, volume, playlists, streams, history
  and the log
- Connect to Remote Server on the login screen: sign in with the server's account without a local login
- Settings → Devices → Sound card buffer: Low delay, Normal, Slow computer or Very slow computer

**Fixed**
- Playback stuttered on slow PCs: songs are now decoded ahead on their own thread and the sound card gets
  a bigger buffer, so a slow disk or a busy CPU no longer causes gaps
- Playback stopped (and Play did nothing until a restart) when another app such as Spotify took over the
  sound card; RadioMaster now reopens the device within about 2 seconds and keeps the stream and
  automation running in the meantime
- No sound in the app while connected to RadioMaster Server: the app now plays the station's on-air mix
  on its own outputs ("Hear the station here" in the connection window turns it off)
- The Mic button while connected to a server: your microphone now goes to the server and on air, with
  the station's mic settings

**Changed**
- Less work on the UI and audio threads: faster Search tab and file explorer, event plugins start in the
  background, and unchanged files aren't rewritten on every save
- Website: a RadioMaster Server page and a "How to start an internet radio station" guide

## v0.2.7 — 2026-10-25
**New**
- RadioMaster Server: a headless version for Linux and Windows Server that plays, schedules and streams
  without a window. Set it up with a station name, a username and password and a port
  (`RadioMaster-Server --setup`); it can play through its own sound cards or only stream. Downloads for
  Linux (with a systemd service) and Windows, see docs/SERVER.md
- File → Connect to Server: run a server's station from the app. Your local session can be saved as a
  profile and comes back exactly when you disconnect. A green button next to the logo shows the
  connection, and File → Server settings changes the server's name, sound output and password
- Studio monitor outputs (Settings → Devices → Outputs): outputs marked as studio monitors go quiet while
  the mic is on, so the host gets no feedback; the stream and other outputs carry on
- New mic mode "keeps playing at full level", for stations that don't want the music lowered on air
- Install steps for Windows, macOS, Linux and RadioMaster Server in the README

**Fixed**
- Inserting a pause, comment, text to speech, voice track or tracks from a playlist while viewing another
  playlist tab put the row into Main instead; Library → Insert as next now goes into the playlist you're
  viewing
- Track editor changes to a song's tags were lost from the library after a restart; they now reach the
  file (every tag in it), the library and every playlist row, and a failed write is reported and undone
- The app froze or lagged while adding files and folders, dragging from the file explorer, using
  Library → Add all, opening playlists, picking adverts and jingles, opening Settings → Devices or
  Reports, and while saving; that work now happens in the background

**Changed**
- The website moved to https://radiomaster.dovafm.com, with a robots.txt, image sitemap and fixed links

## v0.2.6 — 2026-10-25
**New**
- Library cleanup (Tools → Library cleanup): finds duplicate tracks (identical files and, optionally, the
  same song in different files), keeps the best copy and moves playlist rows, cues and play counts over
  to it; finds tracks and playlist rows whose file is missing and relinks them by file name or artist and
  title; lists files that won't play; merges different spellings of the same genre or artist and fills
  in the artist for "Artist - Title" files. Every change is previewed first, removed files go to a
  "Removed tracks" folder unless you choose to delete them for good, and everything is logged to
  cleanup-log.csv. It has its own permission, off for accounts without settings access
- Playlist → Add event: an Event row runs a scheduler action when playback reaches it — e.g. end a
  scheduled show's playlist with "Put a playlist tab on air → Main" and Main carries on where it left
  off instead of dead air
- New action "Play the on-air playlist again from the top" keeps a playlist looping; it works in the
  event scheduler too

**Changed**
- Website: clearer title and description, social sharing card, FAQ section, sitemap and a 404 page

## v0.2.5 — 2026-10-20
**New**
- Advertisement scheduler redesigned: one list of advert and jingle rules on the left, the chosen rule
  on the right in three steps — what plays, how often, when it's in force — plus an Adverts list where
  each advert has its own run dates and days. Rules show whether they're in force right now
- Rules can go off after every N songs, every N minutes or hours (on the clock, e.g. :00 and :30, or
  counted from the last time) or at set times, and can be limited to dates, days of the week and hours
  (22:00–06:00 works over midnight). Old song-count rules and commercial breaks are carried over
- Playlist generator: jingle and advert rules can be pattern steps (e.g. 2 songs, a jingle) or inserted
  by time ("every 15 min insert 1 × Station IDs"), using the generator's running-order timing; "Hour
  marker at the top of every hour" is now a checkbox
- Event scheduler is its own window (Tools → Event scheduler, F3) and can generate a new playlist from a
  template, append a template, play a rule's adverts / jingles, put a playlist tab on air or run any
  station command; events can be limited to dates and have a "Run now" button
- Streams can be MP3 (built in), AAC, Opus, Ogg Vorbis or FLAC (made by ffmpeg), or a custom encoder —
  any program that reads PCM and writes the stream. RadioMaster finds ffmpeg next to itself, in its
  settings folder or on the PATH, or you can choose it
- Date pickers (a calendar) wherever a date is entered: reports, adverts, rules, events, AzuraCast history
- Web panels can be deleted
- More text to speech voices: Windows 10/11 voices are listed beside the classic ones, every espeak-ng
  language and voice variant on Linux, and all macOS voices; the voice list has a search box and loads
  in the background
- New station commands for plugins, the remote API and events: GENERATE / APPEND <template> and
  RULE <rule name>

**Fixed**
- The text to speech window grew past the screen with long text and hid its buttons: the text box now
  has a fixed height and scrolls, and every add / edit dialog keeps OK and Cancel on screen

**Changed**
- One way of doing things throughout: the same delete button (asks "Delete! / Keep" first) for rules,
  adverts, events, categories, templates and web panels; the same day buttons (with Every day /
  Weekdays / Weekends) and date pickers everywhere; tools open the same way from the Tools menu, the
  quick buttons and shortcuts; the Scheduler joins the quick buttons

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
