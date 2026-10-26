// RadioMaster Server web portal: signs in and runs the station through the server's own API.
"use strict";

const $ = (id) => document.getElementById(id);
const store = (() => { try { return window.sessionStorage; } catch (e) { return null; } })();
const remember = (k, v) => { try { if (store) store.setItem(k, v); } catch (e) { /* private mode */ } };
const recall = (k) => { try { return (store && store.getItem(k)) || ""; } catch (e) { return ""; } };
let token = recall("rm_token");
let station = recall("rm_station");
// what the server has already sent (it only sends lists again when they change)
let rev = 0, cfgRev = 0, logRev = 0, histRev = 0;
let tabs = null, cfg = null, live = null;
let viewTab = -1;
let timer = null;
let volumeBusy = false;
let view = recall("rm_view") || "studio";
let library = null, libVer = -1, libLoading = false;

// ------------------------------------------------------------------ helpers

/// An element with properties (`on*` are event handlers) and children (strings become text).
function el(tag, props, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (k === "class") e.className = v;
    else if (k in e && k !== "list") e[k] = v;
    else e.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    e.append(c instanceof Node ? c : document.createTextNode(String(c)));
  }
  return e;
}

class ApiError extends Error {
  constructor(msg, status) { super(msg); this.status = status; }
}

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
  if (r.status === 401 && path !== "login") { signedOut(); throw new ApiError("signed out", 401); }
  if (!r.ok) throw new ApiError((data && data.error) || "HTTP " + r.status, r.status);
  return data;
}

let toastTimer = null;
function toast(msg, err) {
  const t = $("toast");
  t.textContent = msg;
  t.className = err ? "err" : "";
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.hidden = true; }, err ? 6000 : 2500);
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

function fileName(p) { return String(p || "").split(/[\\/]/).pop(); }
function dirName(p) {
  const s = String(p || "");
  const i = Math.max(s.lastIndexOf("/"), s.lastIndexOf("\\"));
  return i > 0 ? s.slice(0, i) : "";
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

function onAirTab() { return tabs ? tabs.playing : 0; }

function tabOptions(sel, current) {
  sel.replaceChildren();
  if (!tabs) return;
  tabs.tabs.forEach((t, i) => {
    sel.append(el("option", { value: i }, (t.name || "Playlist " + (i + 1)) + (i === tabs.playing ? " (on air)" : "")));
  });
  sel.value = current >= 0 && current < tabs.tabs.length ? current : tabs.playing;
}

function send(...cmds) {
  return api("cmd", cmds).then(poll).catch((e) => toast(e.message, true));
}

/// Change the station settings: `mutate` edits a copy of the latest settings. When someone else
/// changed them first, it is done again on their version.
async function saveCfg(mutate, done) {
  for (let i = 0; i < 2; i++) {
    const c = JSON.parse(JSON.stringify(cfg));
    if (mutate(c) === false) return false;
    try {
      await api("config", { base: cfgRev, cfg: c });
      cfgRev = 0; // fetch the saved copy
      await poll();
      toast(done || "Saved");
      return true;
    } catch (e) {
      if (e.status === 409 && i === 0) { cfgRev = 0; await poll(); continue; }
      toast("Not saved: " + e.message, true);
      return false;
    }
  }
  return false;
}

// ------------------------------------------------------------------ sign-in and views

function showDesk(on) {
  document.body.classList.remove("boot");
  $("login").hidden = on;
  $("views").hidden = !on;
  $("nav").hidden = !on;
  $("signout").hidden = !on;
  $("station").textContent = on ? station : "";
  if (!on) $("update").hidden = true;
}

function signedOut() {
  token = "";
  if (store) store.removeItem("rm_token");
  clearTimeout(timer);
  rev = cfgRev = logRev = histRev = 0;
  tabs = cfg = live = null;
  library = null; libVer = -1;
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
    remember("rm_token", token);
    remember("rm_station", station);
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

function showView(v) {
  if (!document.querySelector(`.view[data-view="${v}"]`)) v = "studio";
  view = v;
  remember("rm_view", v);
  document.querySelectorAll(".view").forEach((x) => { x.hidden = x.dataset.view !== v; });
  document.querySelectorAll("#nav button").forEach((b) => b.classList.toggle("on", b.dataset.view === v));
  renderView();
  window.scrollTo(0, 0);
}

document.querySelectorAll("#nav button").forEach((b) => b.addEventListener("click", () => showView(b.dataset.view)));

/// Draw what the current view shows from the latest copy of the station.
function renderView() {
  if (!tabs || !cfg) return;
  switch (view) {
    case "library": tabOptions($("libTab"), parseInt($("libTab").value, 10)); loadLibrary(); break;
    case "carts": renderCarts(); break;
    case "generator": renderGenerator(); break;
    case "ads": renderRules(); break;
    case "azuracast": renderAzForm(); azLoad(); break;
  }
}

// ------------------------------------------------------------------ studio

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
$("toLibrary").addEventListener("click", () => {
  showView("library");
  $("libTab").value = viewTab < 0 ? onAirTab() : viewTab;
  $("libSearch").focus();
});

function playRow(row) {
  const tab = viewTab < 0 ? onAirTab() : viewTab;
  const xfade = live && live.state === "playing" && cfg ? cfg.crossfade_secs || 0 : 0;
  const cmds = [];
  if (tab !== onAirTab()) cmds.push({ cmd: "tab_on_air", tab });
  cmds.push({ cmd: "play_index", tab, row, xfade });
  send(...cmds);
}

function removeRow(row) {
  const tab = viewTab < 0 ? onAirTab() : viewTab;
  send({ cmd: "remove_rows", tab, rows: [row] });
}

function renderTabs() {
  if (viewTab < 0 || viewTab >= tabs.tabs.length) viewTab = tabs.playing;
  tabOptions($("tab"), viewTab);
  tabOptions($("libTab"), parseInt($("libTab").value, 10));
}

function renderRows() {
  const ol = $("rows");
  ol.replaceChildren();
  if (!tabs) return;
  const t = tabs.tabs[viewTab];
  if (!t) return;
  if (!t.entries.length) {
    ol.append(el("li", { class: "muted" }, "This playlist is empty. ", el("button", { onclick: () => $("toLibrary").click() }, "Add songs")));
    return;
  }
  const frag = document.createDocumentFragment();
  t.entries.forEach((e, i) => {
    const text = rowTitle(e);
    const li = el("li", { class: (e.played ? "played " : "") + (t.current === i ? "current" : ""), ondblclick: () => playRow(i) },
      el("span", { class: "n" }, String(i + 1).padStart(3, "0")),
      el("span", { class: "t", title: text }, text),
      el("span", { class: "d" }, e.track.duration ? fmt(e.track.duration) : ""),
      el("button", { title: "Play now", "aria-label": "Play now", onclick: () => playRow(i) }, "▶"),
      el("button", { class: "ghost", title: "Remove", "aria-label": "Remove from the playlist", disabled: t.current === i, onclick: () => removeRow(i) }, "✕"));
    frag.append(li);
  });
  ol.append(frag);
}

function renderEncoders() {
  const ul = $("encoders");
  ul.replaceChildren();
  const list = (cfg && cfg.encoders) || [];
  if (!list.length) { ul.append(el("li", { class: "muted" }, "No streams set up.")); return; }
  list.forEach((enc, i) => {
    const st = live && live.encoders ? live.encoders[i] : null;
    ul.append(el("li", {},
      el("span", {}, enc.name || "Stream " + (i + 1), " ",
        el("span", { class: "small " + (st && st.state === "live" ? "live" : "muted") }, st ? st.state + (st.error ? ": " + st.error : "") : "off")),
      el("button", { onclick: () => send({ cmd: st ? "encoder_stop" : "encoder_start", i }) }, st ? "Stop" : "Start")));
  });
}

function renderList(id, items, text, err) {
  const ul = $(id);
  ul.replaceChildren();
  const frag = document.createDocumentFragment();
  items.slice().reverse().forEach((x) => frag.append(el("li", { class: err && err(x) ? "err" : "" }, text(x))));
  ul.append(frag);
}

let encKey = "", cartKey = "";

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
  const ck = JSON.stringify(l.carts);
  if (ck !== cartKey) { cartKey = ck; if (view === "carts") renderCarts(); }
  if (view === "library" && library && l.scanning) $("libInfo").textContent = "Scanning the library folders… " + (l.scan_progress || "");
}

// ------------------------------------------------------------------ library

let libTimer = null;
$("libSearch").addEventListener("input", () => { clearTimeout(libTimer); libTimer = setTimeout(renderLibrary, 150); });
$("libFolder").addEventListener("change", renderLibrary);
$("rescan").addEventListener("click", () => send({ cmd: "rescan" }).then(() => toast("Rescanning the library folders")));

async function loadLibrary(force) {
  if (libLoading || (library && !force && libVer === (live ? live.lib_ver : libVer))) { renderLibrary(); return; }
  libLoading = true;
  $("libInfo").textContent = "Loading the library…";
  try {
    library = await api("library");
    libVer = live ? live.lib_ver : 0;
    const dirs = [...new Set(library.map((t) => dirName(t.path)))].sort((a, b) => a.localeCompare(b));
    const sel = $("libFolder"), cur = sel.value;
    sel.replaceChildren(el("option", { value: "" }, "Every folder"), ...dirs.map((d) => el("option", { value: d }, d)));
    sel.value = dirs.includes(cur) ? cur : "";
    renderLibrary();
  } catch (e) {
    $("libInfo").textContent = "Couldn't load the library: " + e.message;
  } finally {
    libLoading = false;
  }
}

/// Library tracks matching the search (all words, anywhere) and folder.
function libMatches(q, folder, limit) {
  const words = q.toLowerCase().split(/\s+/).filter(Boolean);
  const out = [];
  let total = 0;
  for (const t of library || []) {
    if (folder && dirName(t.path) !== folder) continue;
    if (words.length) {
      const hay = (t.artist + " " + t.title + " " + t.album + " " + t.genre + " " + fileName(t.path)).toLowerCase();
      if (!words.every((w) => hay.includes(w))) continue;
    }
    total++;
    if (out.length < limit) out.push(t);
  }
  return { out, total };
}

function renderLibrary() {
  if (!library) return;
  const { out, total } = libMatches($("libSearch").value, $("libFolder").value, 300);
  $("libInfo").textContent = library.length
    ? `${total} of ${library.length} tracks` + (total > out.length ? ` (the first ${out.length} shown: search to narrow it down)` : "")
    : "The library is empty. Add library folders in the app (Settings → Library) and rescan.";
  const ol = $("libRows");
  ol.replaceChildren();
  const frag = document.createDocumentFragment();
  for (const t of out) {
    const text = label(t) || fileName(t.path);
    frag.append(el("li", {},
      el("span", { class: "t", title: t.path }, text, el("small", {}, [t.album, t.genre, fileName(t.path)].filter(Boolean).join(" · "))),
      el("span", { class: "d" }, t.duration ? fmt(t.duration) : ""),
      el("button", { title: "Play after the current song", onclick: () => addTracks([t.path], true) }, "Next"),
      el("button", { title: "Add to the end of the playlist", "aria-label": "Add", onclick: () => addTracks([t.path], false) }, "＋")));
  }
  ol.append(frag);
}

/// Put files into the chosen playlist: at the end, or (`next`) after the row on air.
function addTracks(paths, next) {
  let tab = parseInt($("libTab").value, 10);
  if (isNaN(tab)) tab = onAirTab();
  let at = null;
  if (next) {
    const t = tabs && tabs.tabs[tab];
    at = t && t.current !== null && t.current !== undefined ? t.current + 1 : 0;
  }
  send({ cmd: "insert", tab, paths, at }).then(() => toast(next ? "Plays next" : "Added"));
}

// ------------------------------------------------------------------ dialog

let dlgSave = null;
/// Show the editing dialog; `save` returns false to keep it open.
function dialog(title, body, save, okText) {
  $("dlgTitle").textContent = title;
  $("dlgBody").replaceChildren(...[].concat(body));
  $("dlgOk").textContent = okText || "Save";
  dlgSave = save;
  $("dlg").showModal();
}
$("dlgForm").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const ok = ev.submitter ? ev.submitter.value === "ok" : true;
  if (ok && dlgSave) {
    if (!$("dlgForm").reportValidity()) return;
    $("dlgOk").disabled = true;
    try { if ((await dlgSave()) === false) return; } finally { $("dlgOk").disabled = false; }
  }
  $("dlg").close();
});
// the view isn't redrawn while the dialog is open: catch up with what changed meanwhile
$("dlg").addEventListener("close", () => renderView());

function field(text, input) { return el("label", {}, text, " ", input); }
function check(text, on) {
  const i = el("input", { type: "checkbox", checked: !!on });
  return [el("label", { class: "row inline" }, i, " ", text), i];
}
function num(v, min, max, step) { return el("input", { type: "number", value: v, min, max, step: step || 1 }); }
function select(options, cur) {
  const s = el("select", {}, ...options.map(([v, t]) => el("option", { value: v }, t)));
  s.value = cur;
  return s;
}

/// A search box over the library that fills `target` with the picked file's path.
function filePicker(target, onPick) {
  const q = el("input", { type: "search", placeholder: "Search the library for a file" });
  const list = el("div", { class: "pick", hidden: true });
  const draw = () => {
    list.replaceChildren();
    if (!q.value.trim()) { list.hidden = true; return; }
    const { out } = libMatches(q.value, "", 50);
    list.hidden = false;
    if (!out.length) list.append(el("div", { class: "muted" }, "No matches"));
    out.forEach((t) => list.append(el("div", { tabindex: 0, title: t.path,
      onclick: () => { target.value = t.path; if (onPick) onPick(t); q.value = ""; draw(); } }, label(t) || fileName(t.path))));
  };
  q.addEventListener("input", draw);
  if (!library) loadLibrary();
  return el("div", {}, q, list);
}

/// The library's folders, offered while typing a folder.
function folderList() {
  const id = "folders-list";
  let dl = document.getElementById(id);
  if (!dl) { dl = el("datalist", { id }); document.body.append(dl); }
  const dirs = new Set((library || []).map((t) => dirName(t.path)));
  ((cfg && cfg.library_folders) || []).forEach((d) => dirs.add(d));
  dl.replaceChildren(...[...dirs].sort().map((d) => el("option", { value: d })));
  if (!library) loadLibrary().then(() => folderList());
  return id;
}

// ------------------------------------------------------------------ cart wall

let cartEditing = false;
$("cartEdit").addEventListener("click", () => {
  cartEditing = !cartEditing;
  $("cartEdit").setAttribute("aria-pressed", cartEditing);
  $("cartEdit").textContent = cartEditing ? "Done" : "Edit pads";
  $("cartHint").textContent = cartEditing ? "Tap a pad to choose its sound and name." : "Tap a pad to play it on air.";
  renderCarts();
});
$("cartStop").addEventListener("click", () => send({ cmd: "stop_carts" }));

function samePath(a, b) { return String(a).replace(/\\/g, "/").toLowerCase() === String(b).replace(/\\/g, "/").toLowerCase(); }

function renderCarts() {
  const box = $("carts");
  box.classList.toggle("editing", cartEditing);
  box.replaceChildren();
  const playing = (live && live.carts) || [];
  ((cfg && cfg.carts) || []).forEach((pad, i) => {
    const empty = !pad.path;
    const on = !empty && playing.some((p) => samePath(p, pad.path));
    box.append(el("button", {
      class: (empty ? "empty " : "") + (on ? "playing" : ""),
      title: pad.path || "Empty pad",
      onclick: () => (cartEditing || empty ? editCart(i) : send({ cmd: "cart", idx: i })),
    }, el("span", { class: "k" }, String(i + 1)), el("span", { class: "l" }, pad.label || fileName(pad.path) || (cartEditing ? "Set…" : "—"))));
  });
}

function editCart(i) {
  const pad = cfg.carts[i] || { label: "", path: "" };
  const name = el("input", { value: pad.label, placeholder: "Shown on the pad" });
  const path = el("input", { value: pad.path, placeholder: "A file on the server" });
  dialog("Pad " + (i + 1), [
    field("Name", name),
    field("Sound file", path),
    filePicker(path, (t) => { if (!name.value) name.value = t.title || fileName(t.path); }),
    el("p", { class: "muted small" }, "Leave the file empty to clear the pad."),
  ], () => saveCfg((c) => {
    while (c.carts.length <= i) c.carts.push({ label: "", path: "" });
    c.carts[i].label = name.value.trim();
    c.carts[i].path = path.value.trim();
  }));
}

// ------------------------------------------------------------------ playlist generator

/// Sources a template step can use: categories, and the files of advert / jingle rules.
function sources() {
  const s = (cfg.categories || []).map((c) => [c.name, c.name]);
  (cfg.ad_rules || []).forEach((r) => s.push(["rule:" + r.id, (r.adverts ? "Adverts: " : "Jingles: ") + r.name]));
  return s;
}

function renderGenerator() {
  const names = (cfg.templates || []).map((t) => [t.name, t.name]);
  const gt = $("genTemplate"), cur = gt.value;
  gt.replaceChildren(...names.map(([v, t]) => el("option", { value: v }, t)));
  if (names.some(([v]) => v === cur)) gt.value = cur;
  tabOptions($("genTab"), parseInt($("genTab").value, 10));
  const at = $("autoTemplate");
  at.replaceChildren(...names.map(([v, t]) => el("option", { value: v }, t)));
  at.value = cfg.auto_template;
  $("autoOn").checked = !!cfg.auto_enabled;
  $("autoMins").value = cfg.auto_min_mins;

  const tl = $("templates");
  tl.replaceChildren();
  (cfg.templates || []).forEach((t, i) => {
    const steps = (t.pattern || []).map((p) => p.count + "× " + srcName(p.category)).join(", ");
    tl.append(el("li", {},
      el("span", { class: "grow" }, t.name, el("small", {}, `${t.minutes} min · ${steps || "no steps"}`)),
      el("button", { onclick: () => editTemplate(i) }, "Edit"),
      el("button", { class: "ghost", "aria-label": "Delete", title: "Delete",
        onclick: () => { if (confirm(`Delete the template "${t.name}"?`)) saveCfg((c) => { c.templates.splice(i, 1); }, "Deleted"); } }, "✕")));
  });
  if (!(cfg.templates || []).length) tl.append(el("li", { class: "muted" }, "No templates yet."));

  const cl = $("categories");
  cl.replaceChildren();
  (cfg.categories || []).forEach((c, i) => {
    const what = [c.folder || "any folder", c.genre && "genre " + c.genre].filter(Boolean).join(" · ");
    cl.append(el("li", {},
      el("span", { class: "grow" }, c.name, el("small", {}, what)),
      el("button", { onclick: () => editCategory(i) }, "Edit"),
      el("button", { class: "ghost", "aria-label": "Delete", title: "Delete",
        onclick: () => { if (confirm(`Delete the category "${c.name}"?`)) saveCfg((x) => { x.categories.splice(i, 1); }, "Deleted"); } }, "✕")));
  });
  if (!(cfg.categories || []).length) cl.append(el("li", { class: "muted" }, "No categories yet."));
}

function srcName(v) {
  const s = sources().find(([k]) => k === v);
  return s ? s[1] : v;
}

$("genRun").addEventListener("click", () => {
  const name = $("genTemplate").value;
  if (!name) return toast("Make a template first", true);
  const append = document.querySelector('input[name="genMode"]:checked').value === "append";
  const tab = parseInt($("genTab").value, 10);
  if (!append && !confirm("Replace everything in that playlist?")) return;
  send({ cmd: "generate", tab: isNaN(tab) ? onAirTab() : tab, name, append }).then(() => toast("Generated from " + name));
});

$("autoSave").addEventListener("click", () => saveCfg((c) => {
  c.auto_enabled = $("autoOn").checked;
  c.auto_template = $("autoTemplate").value;
  c.auto_min_mins = Math.max(1, parseInt($("autoMins").value, 10) || 30);
}));

$("tplAdd").addEventListener("click", () => editTemplate(-1));
$("catAdd").addEventListener("click", () => editCategory(-1));

/// Rows of (source, numbers…) with add and remove: template steps and markers.
function stepRows(items, make, fields, addText) {
  const box = el("div");
  const draw = () => {
    box.replaceChildren();
    items.forEach((it, k) => {
      const src = select(sources(), it.category);
      src.addEventListener("change", () => { it.category = src.value; });
      const nums = fields.map(([key, text, min, max]) => {
        const n = num(it[key], min, max);
        n.addEventListener("change", () => { it[key] = Math.max(min, parseInt(n.value, 10) || min); });
        return el("label", { class: "row inline small" }, text, " ", n);
      });
      box.append(el("div", { class: "row wrap" }, src, ...nums,
        el("button", { type: "button", class: "ghost", "aria-label": "Remove", onclick: () => { items.splice(k, 1); draw(); } }, "✕")));
    });
    box.append(el("button", { type: "button", class: "small", onclick: () => { items.push(make()); draw(); } }, addText));
  };
  draw();
  return box;
}

function editTemplate(i) {
  const t = i >= 0 ? JSON.parse(JSON.stringify(cfg.templates[i]))
    : { name: "New template", minutes: 120, pattern: [{ category: (cfg.categories[0] || {}).name || "Music", count: 1 }], markers: [], artist_gap: 5, hour_markers: false };
  t.pattern = t.pattern || [];
  t.markers = t.markers || [];
  const first = (sources()[0] || ["Music"])[0];
  const name = el("input", { value: t.name, required: true });
  const minutes = num(t.minutes, 1, 1440 * 7);
  const gap = num(t.artist_gap, 0, 100);
  const [hourRow, hour] = check("An hour marker at the top of every hour", t.hour_markers);
  dialog(i >= 0 ? "Template" : "New template", [
    field("Name", name),
    el("div", { class: "pair" }, field("Length (minutes)", minutes), field("Songs before an artist repeats", gap)),
    el("div", { class: "sub" }, el("strong", {}, "Steps"), el("p", { class: "muted small" }, "Played in order, then again from the top, until the length is reached."),
      stepRows(t.pattern, () => ({ category: first, count: 1 }), [["count", "×", 1, 50]], "＋ Step")),
    el("div", { class: "sub" }, el("strong", {}, "Markers"), el("p", { class: "muted small" }, "Put in every so many minutes, e.g. a station ID every 30 minutes."),
      stepRows(t.markers, () => ({ category: first, every_mins: 30, count: 1 }), [["every_mins", "every (min)", 1, 1440], ["count", "×", 1, 20]], "＋ Marker")),
    hourRow,
  ], () => {
    const n = name.value.trim();
    if ((cfg.templates || []).some((x, k) => k !== i && x.name === n)) { toast("There is already a template called " + n, true); return false; }
    t.name = n;
    t.minutes = Math.max(1, parseInt(minutes.value, 10) || 60);
    t.artist_gap = Math.max(0, parseInt(gap.value, 10) || 0);
    t.hour_markers = hour.checked;
    const old = i >= 0 ? cfg.templates[i].name : null;
    return saveCfg((c) => {
      const k = old === null ? -1 : c.templates.findIndex((x) => x.name === old);
      if (k >= 0) c.templates[k] = t; else c.templates.push(t);
      if (old !== null && old !== n && c.auto_template === old) c.auto_template = n;
    });
  });
}

function editCategory(i) {
  const c = i >= 0 ? JSON.parse(JSON.stringify(cfg.categories[i])) : { name: "New category", folder: "", genre: "", min_secs: 0, max_secs: 0 };
  const name = el("input", { value: c.name, required: true });
  const folder = el("input", { value: c.folder, list: folderList(), placeholder: "Any folder" });
  const genre = el("input", { value: c.genre, placeholder: "Any genre" });
  const min = num(c.min_secs, 0, 36000);
  const max = num(c.max_secs, 0, 36000);
  dialog(i >= 0 ? "Category" : "New category", [
    field("Name", name),
    field("Folder on the server (and the folders in it)", folder),
    field("Genre contains", genre),
    el("div", { class: "pair" }, field("Shortest (seconds)", min), field("Longest (seconds, 0: any)", max)),
  ], () => {
    const n = name.value.trim();
    if ((cfg.categories || []).some((x, k) => k !== i && x.name === n)) { toast("There is already a category called " + n, true); return false; }
    const old = i >= 0 ? cfg.categories[i].name : null;
    c.name = n;
    c.folder = folder.value.trim();
    c.genre = genre.value.trim();
    c.min_secs = parseFloat(min.value) || 0;
    c.max_secs = parseFloat(max.value) || 0;
    return saveCfg((x) => {
      const k = old === null ? -1 : x.categories.findIndex((y) => y.name === old);
      if (k >= 0) x.categories[k] = c; else x.categories.push(c);
      // the templates follow a renamed category
      if (old !== null && old !== n) {
        (x.templates || []).forEach((t) => [...(t.pattern || []), ...(t.markers || [])].forEach((p) => { if (p.category === old) p.category = n; }));
      }
    });
  });
}

// ------------------------------------------------------------------ advert and jingle rules

const DAYS = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
const pad2 = (n) => String(n).padStart(2, "0");

/// One line saying what a rule does (as the app says it).
function ruleSummary(r) {
  const what = r.adverts ? "advert" : "jingle";
  const n = Math.max(1, r.count);
  const pick = r.pick === "All" ? `all the ${what}s` : r.pick === "InOrder" ? `${n} ${what}${n === 1 ? "" : "s"} in turn` : `${n} random ${what}${n === 1 ? "" : "s"}`;
  let when;
  if (r.trigger === "Songs") when = `after every ${Math.max(1, r.every_songs)} song${r.every_songs === 1 ? "" : "s"}`;
  else if (r.trigger === "Minutes") {
    const m = Math.max(1, r.every_mins);
    when = "every " + (m % 60 === 0 ? `${m / 60} hour${m === 60 ? "" : "s"}` : `${m} minutes`) + (r.on_clock ? " (on the clock)" : "");
  } else when = "at " + ((r.times || []).map(([h, m]) => pad2(h) + ":" + pad2(m)).join(", ") || "—");
  const limits = [];
  if (!(r.days || []).every(Boolean)) limits.push(DAYS.filter((_, k) => r.days[k]).join(" "));
  if (r.from_hour !== r.to_hour) limits.push(`${pad2(r.from_hour)}:00–${pad2(r.to_hour)}:00`);
  if (r.from_date || r.to_date) limits.push(`${r.from_date || "…"} → ${r.to_date || "…"}`);
  return `${pick} ${when}` + (limits.length ? " · " + limits.join(" · ") : "");
}

function renderRules() {
  const ul = $("rules");
  ul.replaceChildren();
  (cfg.ad_rules || []).forEach((r, i) => {
    const on = el("input", { type: "checkbox", checked: r.enabled, title: "On", "aria-label": "Rule on" });
    on.addEventListener("change", () => saveCfg((c) => { const x = c.ad_rules.find((y) => y.id === r.id); if (x) x.enabled = on.checked; }, on.checked ? "Rule on" : "Rule off"));
    ul.append(el("li", {},
      on,
      el("span", { class: "grow" }, el("span", { class: "badge " + (r.adverts ? "advert" : "jingle") }, r.adverts ? "Adverts" : "Jingles"), " ", r.name,
        el("small", {}, ruleSummary(r))),
      el("button", { title: "Put them in the on-air playlist now", onclick: () => send({ cmd: "fire_rule", tab: onAirTab(), id: r.id, after: null }).then(() => toast("Queued " + r.name)) }, "Play now"),
      el("button", { onclick: () => editRule(i) }, "Edit"),
      el("button", { class: "ghost", "aria-label": "Delete", title: "Delete",
        onclick: () => { if (confirm(`Delete the rule "${r.name}"?`)) saveCfg((c) => { c.ad_rules = c.ad_rules.filter((y) => y.id !== r.id); }, "Deleted"); } }, "✕")));
  });
  if (!(cfg.ad_rules || []).length) ul.append(el("li", { class: "muted" }, "No rules yet."));
}

$("ruleAdd").addEventListener("click", () => editRule(-1));

function editRule(i) {
  const r = i >= 0 ? JSON.parse(JSON.stringify(cfg.ad_rules[i])) : {
    id: 0, name: "Jingles", enabled: true, adverts: false, trigger: "Songs", every_songs: 4, every_mins: 30, on_clock: true,
    times: [[12, 0]], use_ad_list: false, folder: "", files: [], count: 1, pick: "Random", next: 0,
    from_date: "", to_date: "", days: [true, true, true, true, true, true, true], from_hour: 0, to_hour: 0,
  };
  const name = el("input", { value: r.name, required: true });
  const kind = select([["false", "Jingles"], ["true", "Adverts"]], String(!!r.adverts));
  const trigger = select([["Songs", "After a number of songs"], ["Minutes", "Every so many minutes"], ["Times", "At set times"]], r.trigger);
  const songs = num(r.every_songs, 1, 1000);
  const mins = num(r.every_mins, 1, 1440);
  const [clockRow, clock] = check("Counted from the top of the hour (:00, :30…)", r.on_clock);
  const times = el("input", { value: (r.times || []).map(([h, m]) => pad2(h) + ":" + pad2(m)).join(", "), placeholder: "08:00, 12:30, 17:45" });
  const songsRow = field("Songs between", songs);
  const minsBox = el("div", {}, field("Minutes between", mins), clockRow);
  const timesRow = field("Times of day", times);
  const showTrigger = () => {
    songsRow.hidden = trigger.value !== "Songs";
    minsBox.hidden = trigger.value !== "Minutes";
    timesRow.hidden = trigger.value !== "Times";
  };
  trigger.addEventListener("change", showTrigger);
  showTrigger();
  const [adListRow, adList] = check("Take them from the Adverts list (each with its own dates and days)", r.use_ad_list);
  const folder = el("input", { value: r.folder, list: folderList(), placeholder: "A folder on the server" });
  const files = el("textarea", { value: (r.files || []).join("\n"), placeholder: "Or chosen files, one per line (used instead of the folder)" });
  const picker = filePicker({ set value(p) { files.value = (files.value.trim() ? files.value.trim() + "\n" : "") + p; } });
  const count = num(r.count, 1, 50);
  const pick = select([["Random", "Random ones"], ["InOrder", "One after another"], ["All", "All of them"]], r.pick);
  const showKind = () => { adListRow.hidden = kind.value !== "true"; };
  kind.addEventListener("change", showKind);
  showKind();
  const days = r.days.map((d) => el("input", { type: "checkbox", checked: d }));
  const hours = [...Array(24).keys()].map((h) => [h, pad2(h) + ":00"]);
  const fromH = select(hours, r.from_hour), toH = select(hours, r.to_hour);
  const fromD = el("input", { type: "date", value: r.from_date }), toD = el("input", { type: "date", value: r.to_date });
  dialog(i >= 0 ? "Rule" : "New rule", [
    el("div", { class: "pair" }, field("Name", name), field("Plays", kind)),
    el("div", { class: "sub" }, field("When", trigger), songsRow, minsBox, timesRow),
    el("div", { class: "sub" }, el("strong", {}, "What plays"), adListRow, field("Folder", folder), files, picker,
      el("div", { class: "pair" }, field("How many each time", count), field("Which", pick))),
    el("div", { class: "sub" }, el("strong", {}, "In force"),
      el("div", { class: "days" }, ...DAYS.map((d, k) => el("label", { class: "row inline" }, days[k], " ", d))),
      el("div", { class: "pair" }, field("From", fromH), field("Until (the same: all day)", toH)),
      el("div", { class: "pair" }, field("From date", fromD), field("Until date", toD))),
  ], () => {
    const parsed = [];
    if (trigger.value === "Times") {
      for (const p of times.value.split(/[,;\s]+/).filter(Boolean)) {
        const m = /^(\d{1,2})[:.]?(\d{2})$/.exec(p);
        if (!m || +m[1] > 23 || +m[2] > 59) { toast("Times look like 08:00 or 17:45 (not " + p + ")", true); return false; }
        parsed.push([+m[1], +m[2]]);
      }
      if (!parsed.length) { toast("Give at least one time", true); return false; }
    }
    Object.assign(r, {
      name: name.value.trim(), adverts: kind.value === "true", trigger: trigger.value,
      every_songs: Math.max(1, parseInt(songs.value, 10) || 1), every_mins: Math.max(1, parseInt(mins.value, 10) || 1), on_clock: clock.checked,
      times: parsed.length ? parsed : r.times, use_ad_list: kind.value === "true" && adList.checked,
      folder: folder.value.trim(), files: files.value.split("\n").map((s) => s.trim()).filter(Boolean),
      count: Math.max(1, parseInt(count.value, 10) || 1), pick: pick.value,
      days: days.map((d) => d.checked), from_hour: +fromH.value, to_hour: +toH.value, from_date: fromD.value, to_date: toD.value,
    });
    return saveCfg((c) => {
      c.ad_rules = c.ad_rules || [];
      const k = r.id ? c.ad_rules.findIndex((y) => y.id === r.id) : -1;
      if (k >= 0) c.ad_rules[k] = r;
      else { r.id = c.ad_rules.reduce((m, y) => Math.max(m, y.id), 0) + 1; c.ad_rules.push(r); }
    });
  });
}

// ------------------------------------------------------------------ AzuraCast

let azTimer = null;

function azReady() { return cfg && cfg.azuracast && cfg.azuracast.base_url && cfg.azuracast.station; }

function renderAzForm() {
  if (document.activeElement && $("azForm").contains(document.activeElement)) return;
  const a = cfg.azuracast || {};
  $("azUrl").value = a.base_url || "";
  $("azStation").value = a.station || "";
  $("azKey").value = "";
  $("azKey").placeholder = a.api_key ? "saved (type to replace)" : "needed for the controls and playlists";
  $("azOn").checked = !!a.enabled;
}

$("azForm").addEventListener("submit", (ev) => {
  ev.preventDefault();
  saveCfg((c) => {
    c.azuracast.base_url = $("azUrl").value.trim().replace(/\/+$/, "");
    c.azuracast.station = $("azStation").value.trim();
    if ($("azKey").value.trim()) c.azuracast.api_key = $("azKey").value.trim();
    c.azuracast.enabled = $("azOn").checked;
  }).then((ok) => { if (ok) { document.activeElement.blur(); renderAzForm(); azLoad(); } });
});

function az(method, path, body) { return api("azuracast", { method, path, body }); }

async function azLoad() {
  clearTimeout(azTimer);
  if (view !== "azuracast") return;
  const st = $("azStatus");
  if (!azReady()) {
    st.textContent = "Set up the connection to AzuraCast below.";
    $("azPlaylists").replaceChildren();
    return;
  }
  try {
    const np = await az("GET", "/nowplaying/{st}");
    const now = np.now_playing || {};
    const song = now.song || {};
    st.replaceChildren(
      el("div", { class: "row wrap" }, el("strong", {}, (np.station && np.station.name) || cfg.azuracast.station),
        el("span", { class: "badge " + (np.is_online ? "playing" : "off") }, np.is_online ? "online" : "offline"),
        np.live && np.live.is_live ? el("span", { class: "badge paused" }, "live: " + np.live.streamer_name) : null),
      el("div", { class: "title" }, song.text || [song.artist, song.title].filter(Boolean).join(" - ") || "—"),
      el("div", { class: "muted small" }, `${(np.listeners && np.listeners.current) || 0} listening`,
        np.playing_next && np.playing_next.song ? " · next: " + (np.playing_next.song.text || np.playing_next.song.title) : ""));
  } catch (e) {
    st.textContent = "Couldn't reach AzuraCast: " + e.message;
  }
  azPlaylists();
  azTimer = setTimeout(azLoad, document.hidden ? 60000 : 15000);
}

async function azPlaylists() {
  const ul = $("azPlaylists");
  if (!cfg.azuracast.api_key) { ul.replaceChildren(el("li", { class: "muted" }, "An API key is needed to see the station's playlists.")); return; }
  try {
    const list = await az("GET", "/station/{st}/playlists");
    ul.replaceChildren();
    (Array.isArray(list) ? list : []).forEach((p) => ul.append(el("li", {},
      el("span", { class: "grow" }, p.name, el("small", {}, [p.type, p.num_songs !== undefined ? p.num_songs + " songs" : "", p.is_enabled ? "on" : "off"].filter(Boolean).join(" · "))),
      el("button", { onclick: () => az("PUT", `/station/{st}/playlist/${encodeURIComponent(p.id)}/toggle`).then(() => { toast(p.is_enabled ? "Switched off" : "Switched on"); azPlaylists(); }).catch((e) => toast(e.message, true)) },
        p.is_enabled ? "Switch off" : "Switch on"))));
    if (!ul.children.length) ul.append(el("li", { class: "muted" }, "The station has no playlists."));
  } catch (e) {
    ul.replaceChildren(el("li", { class: "err" }, e.message));
  }
}

$("azRefresh").addEventListener("click", azLoad);
document.querySelectorAll("[data-az]").forEach((b) => b.addEventListener("click", () => {
  if (!azReady()) return toast("Set up the connection to AzuraCast first", true);
  if (b.dataset.confirm && !confirm(b.dataset.confirm)) return;
  const [method, path] = b.dataset.az.split(" ");
  b.disabled = true;
  az(method, path).then((r) => { toast((r && r.message) || "Done"); setTimeout(azLoad, 1500); })
    .catch((e) => toast(e.message, true)).finally(() => { b.disabled = false; });
}));

// ------------------------------------------------------------------ updates

/// "RadioMaster Server 0.3.0 is out" when the server is behind.
async function checkServer() {
  try {
    const info = await api("server");
    const b = $("update");
    if (info && info.latest) {
      b.replaceChildren(`RadioMaster ${info.latest} is out; this server runs ${info.version}. Update it on the server. `,
        info.release_page ? el("a", { href: info.release_page, target: "_blank", rel: "noopener noreferrer" }, "What's new") : "");
      b.hidden = false;
    } else {
      b.hidden = true;
    }
  } catch (e) { /* only for those allowed to see the server's settings */ }
}

// ------------------------------------------------------------------ polling

async function poll() {
  const s = await api(`state?rev=${rev}&cfg=${cfgRev}&log=${logRev}&hist=${histRev}`);
  live = s.live;
  live.lib_ver = s.lib_ver;
  let rows = false, cfgChanged = false;
  if (s.tabs) { tabs = s.tabs; rev = s.rev; renderTabs(); rows = true; }
  if (s.cfg) {
    cfg = s.cfg; cfgRev = s.cfg_rev; cfgChanged = true;
    if (!volumeBusy) $("volume").value = cfg.volume;
    encKey = ""; // names may have changed
  }
  if (s.log) { logRev = s.log_rev; renderList("log", s.log.slice(-150), (x) => x.time + " " + x.text, (x) => x.error); }
  if (s.history) { histRev = s.hist_rev; renderList("history", s.history.slice(-60), (x) => x.time + "  " + x.text); }
  if (rows) renderRows();
  renderLive();
  // what the open view shows changed (not while a dialog is editing it)
  if ((cfgChanged || rows) && !$("dlg").open && view !== "azuracast" && view !== "library") renderView();
  if (cfgChanged && view === "azuracast") renderAzForm();
  if (view === "library" && library && libVer !== s.lib_ver && !libLoading) loadLibrary(true);
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

let serverTimer = null;
function start() {
  showDesk(true);
  showView(view);
  clearTimeout(timer);
  loop();
  clearInterval(serverTimer);
  checkServer();
  serverTimer = setInterval(() => { if (token) checkServer(); }, 30 * 60 * 1000);
}

document.addEventListener("visibilitychange", () => {
  if (!document.hidden && token) { clearTimeout(timer); loop(); }
});

if (token) start(); else showDesk(false);
