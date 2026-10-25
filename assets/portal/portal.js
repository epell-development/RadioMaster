// RadioMaster Server web portal: signs in and runs the station through the server's own API.
"use strict";

const $ = (id) => document.getElementById(id);
const store = (() => { try { return window.sessionStorage; } catch (e) { return null; } })();
let token = (store && store.getItem("rm_token")) || "";
let station = (store && store.getItem("rm_station")) || "";
// what the server has already sent (it only sends lists again when they change)
let rev = 0, cfgRev = 0, logRev = 0, histRev = 0;
let tabs = null, cfg = null, live = null;
let viewTab = -1;
let timer = null;
let volumeBusy = false;

async function api(path, body) {
  const opts = { method: body === undefined ? "GET" : "POST", headers: {} };
  if (token) opts.headers["Authorization"] = "Bearer " + token;
  if (body !== undefined) {
    opts.headers["Content-Type"] = "application/json";
    opts.body = JSON.stringify(body);
  }
  const r = await fetch("/api/v1/" + path, opts);
  let data = null;
  try { data = await r.json(); } catch (e) { /* empty answer */ }
  if (r.status === 401 && path !== "login") { signedOut(); throw new Error("signed out"); }
  if (!r.ok) throw new Error((data && data.error) || "HTTP " + r.status);
  return data;
}

function fmt(secs) {
  secs = Math.max(0, Math.floor(secs || 0));
  const h = Math.floor(secs / 3600), m = Math.floor((secs % 3600) / 60), s = secs % 60;
  const mm = h ? String(m).padStart(2, "0") : String(m);
  return (h ? h + ":" : "") + mm + ":" + String(s).padStart(2, "0");
}

function label(track) {
  if (!track) return "";
  return track.artist ? track.artist + " - " + track.title : track.title;
}

/// A playlist row's text, like the app shows it.
function rowTitle(e) {
  const k = e.kind;
  if (k === "Track" || k === "VoiceTrack" || k === undefined) return label(e.track);
  if (typeof k === "string") return e.track.title || k;
  const [name, v] = Object.entries(k)[0];
  if (name === "Comment") return "💬 " + v;
  if (name === "Command") return "⚙ " + v;
  if (name === "Url") return "📻 " + (e.track.title || v.url);
  return e.track.title || name;
}

function showDesk(on) {
  $("login").hidden = on;
  $("desk").hidden = !on;
  $("signout").hidden = !on;
  $("station").textContent = on ? station : "";
}

function signedOut() {
  token = "";
  if (store) store.removeItem("rm_token");
  clearTimeout(timer);
  rev = cfgRev = logRev = histRev = 0;
  tabs = cfg = live = null;
  showDesk(false);
}

$("loginform").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  $("loginerr").textContent = "";
  $("loginbtn").disabled = true;
  try {
    const r = await api("login", { user: $("user").value.trim(), password: $("password").value });
    token = r.token;
    station = r.name || "";
    if (store) { store.setItem("rm_token", token); store.setItem("rm_station", station); }
    $("password").value = "";
    start();
  } catch (e) {
    $("loginerr").textContent = e.message;
  } finally {
    $("loginbtn").disabled = false;
  }
});

$("signout").addEventListener("click", async () => {
  try { await api("logout", {}); } catch (e) { /* signed out either way */ }
  signedOut();
});

function send(...cmds) {
  return api("cmd", cmds).then(poll).catch((e) => { $("neterr").textContent = e.message; });
}

function onAirTab() { return tabs ? tabs.playing : 0; }

document.querySelectorAll("[data-cmd]").forEach((b) => b.addEventListener("click", () => {
  switch (b.dataset.cmd) {
    case "play": return send({ cmd: "play_pressed", tab: onAirTab(), selected: null });
    case "pause": return send({ cmd: "pause_toggle" });
    case "stop": return send({ cmd: "stop" });
    case "next": return send({ cmd: "next" });
    case "prev": return send({ cmd: "prev" });
  }
}));

$("volume").addEventListener("input", () => { volumeBusy = true; });
$("volume").addEventListener("change", () => {
  volumeBusy = false;
  send({ cmd: "volume", v: parseFloat($("volume").value) });
});

$("tab").addEventListener("change", () => { viewTab = parseInt($("tab").value, 10); renderRows(); });

function playRow(row) {
  const tab = viewTab < 0 ? onAirTab() : viewTab;
  const xfade = live && live.state === "playing" && cfg ? cfg.crossfade_secs || 0 : 0;
  const cmds = [];
  if (tab !== onAirTab()) cmds.push({ cmd: "tab_on_air", tab });
  cmds.push({ cmd: "play_index", tab, row, xfade });
  send(...cmds);
}

function renderTabs() {
  const sel = $("tab");
  sel.replaceChildren();
  tabs.tabs.forEach((t, i) => {
    const o = document.createElement("option");
    o.value = i;
    o.textContent = (t.name || "Playlist " + (i + 1)) + (i === tabs.playing ? " (on air)" : "");
    sel.appendChild(o);
  });
  if (viewTab < 0 || viewTab >= tabs.tabs.length) viewTab = tabs.playing;
  sel.value = viewTab;
}

function renderRows() {
  const ol = $("rows");
  ol.replaceChildren();
  if (!tabs) return;
  const t = tabs.tabs[viewTab];
  if (!t) return;
  if (!t.entries.length) {
    const li = document.createElement("li"); li.className = "muted"; li.textContent = "This playlist is empty.";
    ol.appendChild(li);
    return;
  }
  const frag = document.createDocumentFragment();
  t.entries.forEach((e, i) => {
    const li = document.createElement("li");
    if (e.played) li.classList.add("played");
    if (t.current === i) li.classList.add("current");
    const n = document.createElement("span"); n.className = "n"; n.textContent = String(i + 1).padStart(3, "0");
    const ti = document.createElement("span"); ti.className = "t"; ti.textContent = rowTitle(e); ti.title = ti.textContent;
    const d = document.createElement("span"); d.className = "d"; d.textContent = e.track.duration ? fmt(e.track.duration) : "";
    const b = document.createElement("button"); b.textContent = "▶"; b.title = "Play now";
    b.addEventListener("click", () => playRow(i));
    li.addEventListener("dblclick", () => playRow(i));
    li.append(n, ti, d, b);
    frag.appendChild(li);
  });
  ol.appendChild(frag);
}

function renderEncoders() {
  const ul = $("encoders");
  ul.replaceChildren();
  const list = (cfg && cfg.encoders) || [];
  if (!list.length) {
    const li = document.createElement("li"); li.className = "muted"; li.textContent = "No streams set up.";
    ul.appendChild(li);
    return;
  }
  list.forEach((enc, i) => {
    const st = live && live.encoders ? live.encoders[i] : null;
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.textContent = enc.name || "Stream " + (i + 1);
    const state = document.createElement("span");
    state.className = "small " + (st && st.state === "live" ? "live" : "muted");
    state.textContent = st ? st.state + (st.error ? ": " + st.error : "") : "off";
    const b = document.createElement("button");
    b.textContent = st ? "Stop" : "Start";
    b.addEventListener("click", () => send({ cmd: st ? "encoder_stop" : "encoder_start", i }));
    const left = document.createElement("span"); left.append(name, document.createTextNode(" "), state);
    li.append(left, b);
    ul.appendChild(li);
  });
}

function renderList(id, items, text, err) {
  const ul = $(id);
  ul.replaceChildren();
  const frag = document.createDocumentFragment();
  items.slice().reverse().forEach((x) => {
    const li = document.createElement("li");
    li.textContent = text(x);
    if (err && err(x)) li.className = "err";
    frag.appendChild(li);
  });
  ul.appendChild(frag);
}

let encKey = "";

function renderLive() {
  const l = live;
  $("state").textContent = l.state;
  $("state").className = "badge " + l.state;
  $("lamp").classList.toggle("on", l.state === "playing");
  $("title").textContent = l.on_air ? (l.on_air_label || label(l.on_air)) : (l.on_air_label || "Nothing playing");
  const pct = l.total > 0 ? Math.min(100, (100 * l.pos) / l.total) : 0;
  $("progress").style.width = pct + "%";
  $("pos").textContent = fmt(l.pos);
  $("remain").textContent = "-" + fmt(l.total - l.pos);
  $("vul").style.width = Math.min(100, l.peak_l * 100) + "%";
  $("vur").style.width = Math.min(100, l.peak_r * 100) + "%";
  $("wait").textContent = l.wait_until ? (l.wait_label || "Waiting") + " " + new Date(l.wait_until * 1000).toLocaleTimeString() : "";
  $("notify").textContent = l.notify || "";
  const key = JSON.stringify(l.encoders);
  if (key !== encKey) { encKey = key; renderEncoders(); }
}

async function poll() {
  const s = await api(`state?rev=${rev}&cfg=${cfgRev}&log=${logRev}&hist=${histRev}`);
  live = s.live;
  let rows = false;
  if (s.tabs) { tabs = s.tabs; rev = s.rev; renderTabs(); rows = true; }
  if (s.cfg) {
    cfg = s.cfg; cfgRev = s.cfg_rev;
    if (!volumeBusy) $("volume").value = cfg.volume;
    encKey = ""; // names may have changed
  }
  if (s.log) { logRev = s.log_rev; renderList("log", s.log.slice(-80), (x) => x.time + " " + x.text, (x) => x.error); }
  if (s.history) { histRev = s.hist_rev; renderList("history", s.history.slice(-30), (x) => x.time + "  " + x.text); }
  if (rows) renderRows();
  renderLive();
  $("neterr").textContent = "";
}

async function loop() {
  try {
    await poll();
  } catch (e) {
    if (!token) return;
    $("neterr").textContent = "Can't reach the server: " + e.message;
  }
  // slower while the tab is in the background
  timer = setTimeout(loop, document.hidden ? 5000 : 1000);
}

function start() {
  showDesk(true);
  clearTimeout(timer);
  loop();
}

document.addEventListener("visibilitychange", () => {
  if (!document.hidden && token) { clearTimeout(timer); loop(); }
});

if (token) start(); else showDesk(false);
