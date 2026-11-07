import { TEMPLATES, ACCENTS, renderCard, ensureFonts, fmtTime } from "./templates.js";

// ─── Bridge ────────────────────────────────────────────────────────────
// Inside the app this is Tauri. In a plain browser a mock lets the UI be previewed.
const api = window.__TAURI__
  ? {
      invoke: window.__TAURI__.core.invoke,
      listen: window.__TAURI__.event.listen,
      win: window.__TAURI__.window.getCurrentWindow(),
      onDrop: (cb) => window.__TAURI__.webview.getCurrentWebview().onDragDropEvent(cb),
    }
  : (await import("./dev-mock.js")).createMock();

const $ = (s, el = document) => el.querySelector(s);
const $$ = (s, el = document) => [...el.querySelectorAll(s)];

// ─── State ─────────────────────────────────────────────────────────────
const DEFAULTS = {
  format: "mp4", template: "ember", accent: ACCENTS[0], wave: true,
  res: "1920x1080", fit: "blur", fps: "30", bitrate: "256", rate: "48000",
  encoder: "libx264", quality: "balanced", smart: true, chapters: true,
  linkMode: "audio",
  dlDir: null, // null = "Downloads" next to the app
};
const settings = (() => {
  try { return { ...DEFAULTS, ...JSON.parse(localStorage.getItem("aio.settings") || "{}") }; }
  catch { return { ...DEFAULTS }; }
})();
// drop saved values that no longer exist (e.g. from an older version)
if (!ACCENTS.includes(settings.accent)) settings.accent = DEFAULTS.accent;
if (!TEMPLATES.some((t) => t.id === settings.template)) settings.template = DEFAULTS.template;
const save = () => { try { localStorage.setItem("aio.settings", JSON.stringify(settings)); } catch {} };

const state = {
  clips: [],            // { id, info, thumb, img }
  selected: new Set(),
  anchor: null,
  outDir: null,
  outConfirmed: false,
  toolsOk: true,
  encoders: [],
  fast: { possible: false },
  merging: false,
  lastResult: null,
};
let nextId = 1;

// ─── Utilities ─────────────────────────────────────────────────────────
function fmtBytes(b) {
  if (!b) return "0 B";
  const u = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(u.length - 1, Math.floor(Math.log(b) / Math.log(1024)));
  return `${(b / 1024 ** i).toFixed(i >= 3 ? 1 : 0)} ${u[i]}`;
}
const stem = (name) => name.replace(/\.[^.]+$/, "");
const dirOf = (p) => p.replace(/[\\/][^\\/]*$/, "");
const sepOf = (p) => (p.includes("\\") ? "\\" : "/");
const resWH = () => settings.res.split("x").map(Number);
const titleOf = (c) => c.info.title || stem(c.info.name);
const totalDuration = () => state.clips.reduce((a, c) => a + c.info.duration, 0);

function toast(msg) {
  const t = document.createElement("div");
  t.className = "toast";
  t.textContent = msg;
  $("#toasts").append(t);
  setTimeout(() => { t.classList.add("out"); setTimeout(() => t.remove(), 250); }, 3000);
}

function trackOf(clip, index, count) {
  const i = clip.info;
  return { title: titleOf(clip), artist: i.artist || "", album: i.album || "", duration: i.duration, index, count };
}

function loadImage(src) {
  return new Promise((res) => {
    if (!src) return res(null);
    const img = new Image();
    img.onload = () => res(img);
    img.onerror = () => res(null);
    img.src = src;
  });
}

// ─── Adding files ──────────────────────────────────────────────────────
let probingCount = 0;
async function addPaths(paths) {
  if (!paths?.length) return;
  probingCount += paths.length;
  $("#probing").hidden = false;
  try {
    const infos = await api.invoke("probe_files", { paths });
    const added = [];
    let bad = 0;
    for (const info of infos) {
      if (info.error) { bad++; continue; }
      const clip = { id: nextId++, info, thumb: null, img: null };
      state.clips.push(clip);
      added.push(clip);
    }
    if (bad) toast(`Skipped ${bad} unreadable file${bad > 1 ? "s" : ""}`);
    else if (!added.length) toast("No audio or video files found");
    render();
    queueThumbs(added);
  } catch (e) {
    toast(String(e));
  } finally {
    probingCount = Math.max(0, probingCount - paths.length);
    $("#probing").hidden = probingCount === 0;
  }
}

const thumbQueue = [];
let thumbActive = 0;
function queueThumbs(clips) {
  thumbQueue.push(...clips.filter((c) => c.info.kind === "video" || c.info.hasCover));
  pumpThumbs();
}
function pumpThumbs() {
  while (thumbActive < 3 && thumbQueue.length) {
    const clip = thumbQueue.shift();
    thumbActive++;
    api.invoke("thumbnail", { path: clip.info.path, kind: clip.info.kind, duration: clip.info.duration })
      .then(async (url) => {
        if (!url) return;
        clip.thumb = url;
        if (clip.info.kind === "audio") clip.img = await loadImage(url);
        if (state.clips.includes(clip)) { updateRow(clip); schedulePreview(); }
      })
      .catch(() => {})
      .finally(() => { thumbActive--; pumpThumbs(); });
  }
}

// ─── Clip list ─────────────────────────────────────────────────────────
const list = $("#clipList");
const rows = new Map(); // id -> element

const ICON = {
  audio: `<svg viewBox="0 0 24 24"><path d="M9 18V5l11-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="17" cy="16" r="3"/></svg>`,
  video: `<svg viewBox="0 0 24 24"><rect x="2.5" y="5" width="14" height="14" rx="2.5"/><path d="M16.5 10l5-3v10l-5-3"/></svg>`,
  handle: `<svg viewBox="0 0 10 16"><circle cx="2.5" cy="3" r="1.4"/><circle cx="7.5" cy="3" r="1.4"/><circle cx="2.5" cy="8" r="1.4"/><circle cx="7.5" cy="8" r="1.4"/><circle cx="2.5" cy="13" r="1.4"/><circle cx="7.5" cy="13" r="1.4"/></svg>`,
  x: `<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg>`,
};

function subline(info) {
  if (info.kind === "video") {
    const size = info.width ? `${info.width}×${info.height}` : "Video";
    return info.hasAudio ? size : `${size} · no sound`;
  }
  return info.artist || (info.audioCodec || "Audio").toUpperCase();
}

function makeRow(clip) {
  const li = document.createElement("li");
  li.className = "clip";
  li.dataset.id = clip.id;
  li.innerHTML = `
    <div class="handle" title="Drag to reorder">${ICON.handle}</div>
    <div class="clip-no"></div>
    <div class="thumb">${ICON[clip.info.kind]}</div>
    <div class="clip-main"><div class="clip-title"></div><div class="clip-sub"></div></div>
    <div class="clip-dur"></div>
    <button class="remove" aria-label="Remove">${ICON.x}</button>
    <div class="dl-bar"><i></i></div>`;
  rows.set(clip.id, li);
  return li;
}

function updateRow(clip, index) {
  const li = rows.get(clip.id);
  if (!li) return;
  if (index !== undefined) li.querySelector(".clip-no").textContent = index + 1;
  const title = li.querySelector(".clip-title");
  title.textContent = titleOf(clip);
  title.title = clip.info.path;
  const p = clip.pending;
  li.classList.toggle("pending", !!p);
  li.querySelector(".clip-sub").textContent = p ? `${p.mode === "video" ? "Video" : "Song"} · ${p.status}` : subline(clip.info);
  li.querySelector(".clip-dur").textContent = clip.info.duration ? fmtTime(clip.info.duration) : "";
  if (p) {
    const bar = li.querySelector(".dl-bar");
    bar.classList.toggle("busy", p.percent == null);
    bar.firstElementChild.style.width = `${p.percent ?? 0}%`;
  }
  li.classList.toggle("selected", state.selected.has(clip.id));
  const thumb = li.querySelector(".thumb");
  if (clip.thumb && !thumb.querySelector("img")) {
    const img = document.createElement("img");
    img.src = clip.thumb;
    img.alt = "";
    thumb.replaceChildren(img);
  }
}

/** Measures rows, runs `mutate`, then animates rows from old to new spots. */
function flip(mutate) {
  const before = new Map();
  for (const [id, el] of rows) before.set(id, el.getBoundingClientRect().top);
  mutate();
  for (const [id, el] of rows) {
    if (el.classList.contains("dragging")) continue;
    const old = before.get(id);
    if (old === undefined) continue;
    const dy = old - el.getBoundingClientRect().top;
    if (!dy) continue;
    el.style.transition = "none";
    el.style.transform = `translateY(${dy}px)`;
    requestAnimationFrame(() => {
      el.style.transition = "transform .24s cubic-bezier(.2,.8,.2,1)";
      el.style.transform = "";
    });
  }
}

function renderList() {
  const ids = new Set(state.clips.map((c) => c.id));
  for (const [id, el] of rows) if (!ids.has(id)) { el.remove(); rows.delete(id); }
  state.clips.forEach((clip, i) => {
    const el = rows.get(clip.id) || makeRow(clip);
    if (list.children[i] !== el) list.insertBefore(el, list.children[i] || null);
    updateRow(clip, i);
  });
}

function renderHeader() {
  const n = state.clips.length;
  $("#emptyState").hidden = n > 0;
  $("#summary").textContent = n ? `${n} item${n > 1 ? "s" : ""} · ${fmtTime(totalDuration())}` : "";
}

function render() {
  renderHeader();
  renderList();
  renderDeck();
  schedulePreview();
  scheduleFastCheck();
}

// selection & removal
list.addEventListener("click", (e) => {
  const li = e.target.closest(".clip");
  if (!li) return;
  const id = Number(li.dataset.id);
  if (e.target.closest(".remove")) { removeClips([id]); return; }
  if (e.target.closest(".handle")) return;
  const ids = state.clips.map((c) => c.id);
  if (e.shiftKey && state.anchor != null) {
    const [a, b] = [ids.indexOf(state.anchor), ids.indexOf(id)].sort((x, y) => x - y);
    state.selected = new Set(ids.slice(a, b + 1));
  } else if (e.ctrlKey || e.metaKey) {
    state.selected.has(id) ? state.selected.delete(id) : state.selected.add(id);
    state.anchor = id;
  } else {
    state.selected = state.selected.size === 1 && state.selected.has(id) ? new Set() : new Set([id]);
    state.anchor = id;
  }
  renderList(); schedulePreview();
});
list.addEventListener("dblclick", (e) => {
  const li = e.target.closest(".clip");
  const clip = li && state.clips.find((c) => c.id === Number(li.dataset.id));
  if (clip && !clip.pending && !e.target.closest(".remove, .handle")) api.invoke("open_path", { path: clip.info.path }).catch(() => {});
});

function removeClips(ids) {
  const set = new Set(ids);
  for (const c of state.clips) if (set.has(c.id) && c.pending?.job) api.invoke("yt_cancel", { job: c.pending.job });
  flip(() => {
    state.clips = state.clips.filter((c) => !set.has(c.id));
    ids.forEach((id) => state.selected.delete(id));
    renderList();
  });
  render();
}

// ─── Drag to reorder (pointer events; HTML5 DnD is owned by the OS file drop) ──
const wrap = $("#listWrap");
let drag = null;
list.addEventListener("pointerdown", (e) => {
  const handle = e.target.closest(".handle");
  if (!handle || e.button !== 0) return;
  e.preventDefault();
  const el = handle.closest(".clip");
  drag = { id: Number(el.dataset.id), el, startY: e.clientY, y: e.clientY, scroll: 0, raf: 0 };
  list.setPointerCapture(e.pointerId);
  el.classList.add("dragging");
  document.body.style.cursor = "grabbing";
  autoscroll();
});
list.addEventListener("pointermove", (e) => {
  if (!drag) return;
  drag.y = e.clientY;
  const r = wrap.getBoundingClientRect();
  const edge = 50;
  drag.scroll = e.clientY < r.top + edge ? -Math.ceil((r.top + edge - e.clientY) / 4)
    : e.clientY > r.bottom - edge ? Math.ceil((e.clientY - (r.bottom - edge)) / 4) : 0;
  dragStep();
});
function dragStep() {
  const { el } = drag;
  const from = state.clips.findIndex((c) => c.id === drag.id);
  let to = from;
  state.clips.forEach((c, i) => {
    const other = rows.get(c.id);
    if (other === el) return;
    const r = other.getBoundingClientRect();
    const mid = r.top + r.height / 2;
    if (i < from && drag.y < mid) to = Math.min(to, i);
    if (i > from && drag.y > mid) to = Math.max(to, i);
  });
  if (to !== from) {
    const before = el.getBoundingClientRect().top;
    flip(() => {
      const [c] = state.clips.splice(from, 1);
      state.clips.splice(to, 0, c);
      renderList();
    });
    // keep the dragged row under the pointer after its DOM slot moved
    drag.startY += el.getBoundingClientRect().top - before;
  }
  el.style.transform = `translateY(${drag.y - drag.startY}px)`;
}
function autoscroll() {
  if (!drag) return;
  if (drag.scroll) {
    const prev = wrap.scrollTop;
    wrap.scrollTop += drag.scroll;
    drag.startY -= wrap.scrollTop - prev;
    dragStep();
  }
  drag.raf = requestAnimationFrame(autoscroll);
}
function endDrag() {
  if (!drag) return;
  cancelAnimationFrame(drag.raf);
  const { el } = drag;
  el.style.transition = "transform .2s cubic-bezier(.2,.8,.2,1)";
  el.style.transform = "";
  setTimeout(() => { el.classList.remove("dragging"); el.style.transition = ""; }, 200);
  drag = null;
  document.body.style.cursor = "";
  render();
}
list.addEventListener("pointerup", endDrag);
list.addEventListener("pointercancel", endDrag);

// ─── More menu ─────────────────────────────────────────────────────────
const menu = $("#moreMenu");
$("#btnMore").addEventListener("click", (e) => { e.stopPropagation(); menu.hidden = !menu.hidden; });
document.addEventListener("click", () => (menu.hidden = true));
menu.addEventListener("click", (e) => {
  const b = e.target.closest("[data-act]");
  if (!b) return;
  const act = b.dataset.act;
  if (act === "folder") { addFolder(); return; }
  if (act === "link") { openLinkDialog(""); return; }
  if (act === "clear") { state.clips = []; state.selected.clear(); render(); return; }
  const coll = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });
  flip(() => {
    if (act === "name") state.clips.sort((a, b) => coll.compare(titleOf(a), titleOf(b)));
    if (act === "duration") state.clips.sort((a, b) => a.info.duration - b.info.duration);
    if (act === "reverse") state.clips.reverse();
    renderList();
  });
  render();
});

// ─── Output settings ───────────────────────────────────────────────────
function bindSeg(id, key, after) {
  const el = $(id);
  el.addEventListener("click", (e) => {
    const b = e.target.closest("button[data-v]");
    if (!b || b.disabled) return;
    settings[key] = b.dataset.v;
    save();
    syncSeg(el, key);
    after?.();
    renderDeck();
  });
  syncSeg(el, key);
}
function syncSeg(el, key) {
  $$("button[data-v]", el).forEach((b) => b.setAttribute("aria-checked", String(b.dataset.v === settings[key])));
}
function bindToggle(id, key, after) {
  const el = $(id);
  el.checked = !!settings[key];
  el.addEventListener("change", () => { settings[key] = el.checked; save(); after?.(); renderDeck(); });
}

$$(".format-tile").forEach((t) => t.addEventListener("click", () => {
  settings.format = t.dataset.format; save(); renderDeck(); schedulePreview(); scheduleFastCheck();
}));
bindSeg("#optRes", "res", schedulePreview);
bindSeg("#optFit", "fit");
bindSeg("#optFps", "fps");
bindSeg("#optBitrate", "bitrate");
bindSeg("#optRate", "rate");
bindSeg("#optQuality", "quality");
bindToggle("#optWave", "wave", schedulePreview);
bindToggle("#optSmart", "smart", scheduleFastCheck);
bindToggle("#optChapters", "chapters");

const tplList = $("#templateList");
tplList.innerHTML = TEMPLATES.map((t) => `<button class="tpl" role="radio" data-t="${t.id}"><canvas width="320" height="180"></canvas>${t.name}</button>`).join("");
tplList.addEventListener("click", (e) => {
  const b = e.target.closest(".tpl");
  if (!b) return;
  settings.template = b.dataset.t; save(); renderDeck(); schedulePreview();
});
$("#accentList").innerHTML = ACCENTS.map((c) => `<button class="swatch" role="radio" data-c="${c}" style="--c:${c}" aria-label="Accent ${c}"></button>`).join("");
$("#accentList").addEventListener("click", (e) => {
  const b = e.target.closest(".swatch");
  if (!b) return;
  settings.accent = b.dataset.c; save(); renderDeck(); schedulePreview();
});

function renderEncoders() {
  const labels = { libx264: "CPU", h264_nvenc: "NVIDIA", h264_qsv: "Intel", h264_amf: "AMD" };
  const opts = ["libx264", ...state.encoders];
  if (!opts.includes(settings.encoder)) settings.encoder = "libx264";
  $("#optEncoder").innerHTML = opts.map((o) => `<button data-v="${o}" title="${o}">${labels[o] || o}</button>`).join("");
  syncSeg($("#optEncoder"), "encoder");
}
$("#optEncoder").addEventListener("click", (e) => {
  const b = e.target.closest("button[data-v]");
  if (!b) return;
  settings.encoder = b.dataset.v; save(); syncSeg($("#optEncoder"), "encoder"); renderDeck();
});

function estimateBytes() {
  const ab = Number(settings.bitrate) * 1000 / 8;
  if (settings.smart && state.fast.possible) return state.clips.reduce((a, c) => a + c.info.size, 0);
  if (settings.format === "mp3") return totalDuration() * ab;
  const [w, h] = resWH();
  const px = (w * h) / (1920 * 1080);
  const fpsK = Math.sqrt(Number(settings.fps) / 30);
  const q = { high: 9e6, balanced: 5.5e6, small: 2.8e6 }[settings.quality];
  return state.clips.reduce((a, c) => {
    const vb = c.info.kind === "video" ? q * px * fpsK : (settings.wave ? 0.9e6 : 0.18e6) * px * fpsK;
    return a + c.info.duration * (vb / 8 + ab);
  }, 0);
}

function renderDeck() {
  document.body.dataset.format = settings.format;
  $$(".format-tile").forEach((t) => t.setAttribute("aria-checked", String(t.dataset.format === settings.format)));
  $$(".tpl", tplList).forEach((t) => t.setAttribute("aria-checked", String(t.dataset.t === settings.template)));
  $$(".swatch").forEach((s) => s.setAttribute("aria-checked", String(s.dataset.c === settings.accent)));
  $("#outExt").textContent = `.${settings.format}`;

  const n = state.clips.length;
  const downloading = state.clips.filter((c) => c.pending).length;
  $("#btnMerge").disabled = !n || downloading > 0 || state.merging || !state.toolsOk;
  $("#mergeLabel").textContent = downloading ? `Downloading ${downloading}…` : n ? `Merge ${n} item${n > 1 ? "s" : ""}` : "Merge";
  $("#mergeSub").textContent = n && !downloading ? `≈ ${fmtBytes(estimateBytes())}${settings.smart && state.fast.possible ? " · no re-encode" : ""}` : "";

  const dir = state.outDir || (state.clips[0] ? dirOf(state.clips[0].info.path) : "");
  // LRM marks keep the path's own direction inside the rtl (ellipsis-on-left) box
  $("#outDir").textContent = dir ? `‎${dir}${sepOf(dir)}‎` : "";
  $("#outDir").title = dir;
}

let fastTimer = 0;
function scheduleFastCheck() {
  clearTimeout(fastTimer);
  fastTimer = setTimeout(async () => {
    if (!state.clips.length || state.clips.some((c) => c.pending)) { state.fast = { possible: false }; renderDeck(); return; }
    try {
      state.fast = await api.invoke("check_fast_path", { infos: state.clips.map((c) => c.info), format: settings.format });
    } catch { state.fast = { possible: false }; }
    renderDeck();
  }, 120);
}

// ─── Title card preview ────────────────────────────────────────────────
const sample = { title: "Song title", artist: "Artist", album: "", duration: 214, index: 1, count: 12 };
function previewClip() {
  const sel = state.clips.find((c) => state.selected.has(c.id) && c.info.kind === "audio");
  return sel || state.clips.find((c) => c.info.kind === "audio") || null;
}
let previewRaf = 0;
function schedulePreview() {
  cancelAnimationFrame(previewRaf);
  previewRaf = requestAnimationFrame(async () => {
    if (settings.format !== "mp4") return;
    await ensureFonts();
    const clip = previewClip();
    const count = state.clips.length || sample.count;
    const track = clip ? trackOf(clip, state.clips.indexOf(clip) + 1, count) : sample;
    const img = clip?.img || null;
    const [W, H] = resWH();
    const k = 960 / Math.max(W, H);
    renderCard($("#cardPreview"), { template: settings.template, accent: settings.accent, W: Math.round(W * k), H: Math.round(H * k), track, cover: img, previewWave: settings.wave });
    for (const b of $$(".tpl", tplList)) {
      renderCard(b.querySelector("canvas"), { template: b.dataset.t, accent: settings.accent, W: 384, H: 216, track, cover: img, previewWave: settings.wave });
    }
  });
}

// ─── Output destination ────────────────────────────────────────────────
const outName = $("#outName");
const today = new Date();
outName.value = `Merged ${today.getFullYear()}-${String(today.getMonth() + 1).padStart(2, "0")}-${String(today.getDate()).padStart(2, "0")}`;
outName.addEventListener("input", () => { state.outConfirmed = false; });
$("#btnDest").addEventListener("click", chooseDest);
async function chooseDest() {
  const dir = state.outDir || (state.clips[0] ? dirOf(state.clips[0].info.path) : null);
  const picked = await api.invoke("pick_output", { defaultName: `${outName.value || "Merged"}.${settings.format}`, format: settings.format, directory: dir });
  if (!picked) return;
  state.outDir = dirOf(picked);
  outName.value = stem(picked.slice(state.outDir.length + 1));
  state.outConfirmed = true;
  renderDeck();
}
async function outputPath() {
  const dir = state.outDir || dirOf(state.clips[0].info.path);
  const name = (outName.value.trim() || "Merged").replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_");
  const path = `${dir}${sepOf(dir)}${name}.${settings.format}`;
  return state.outConfirmed ? path : api.invoke("unique_path", { path });
}

// ─── Merge ─────────────────────────────────────────────────────────────
const modal = $("#modal");
let runStart = 0, runTimer = 0, lastPct = 0;

function showState(which) {
  $("#stateLink").hidden = which !== "link";
  $("#stateRun").hidden = which !== "run";
  $("#stateDone").hidden = which !== "done";
  $("#stateError").hidden = which !== "error";
  modal.hidden = !which;
}

const STAGES = { cards: "Preparing", render: "Rendering", decode: "Reading", encode: "Encoding", join: "Joining", done: "Finishing" };
function setRun({ stage, detail, index, count, percent }) {
  $("#runStage").textContent = STAGES[stage] || stage;
  if (detail !== undefined) $("#runDetail").textContent = detail.replace(/^(Rendering|Reading) /, "");
  if (count) $("#runIndex").textContent = `${Math.min(index + 1, count)} / ${count}`;
  if (percent != null) {
    lastPct = Math.max(lastPct, percent);
    $("#runPercent").textContent = Math.floor(lastPct);
    $("#runFill").style.width = `${lastPct}%`;
  }
}

function tickClock() {
  const el = (performance.now() - runStart) / 1000;
  $("#runEta").textContent = lastPct > 2 && lastPct < 100 ? fmtTime((el * (100 - lastPct)) / lastPct) : "—";
}

async function merge() {
  if (!state.clips.length || state.merging || !state.toolsOk) return;
  if (state.clips.some((c) => c.pending)) { toast("Wait for downloads to finish"); return; }
  const output = await outputPath();
  if (!output) return;

  state.merging = true;
  renderDeck();
  lastPct = 0;
  runStart = performance.now();
  clearInterval(runTimer);
  runTimer = setInterval(tickClock, 500);
  tickClock();
  setRun({ stage: "cards", detail: "", index: 0, count: state.clips.length, percent: 0 });
  showState("run");

  try {
    const [W, H] = resWH();
    const fast = settings.smart && state.fast.possible;
    const canvas = document.createElement("canvas");
    await ensureFonts();
    const items = [];
    for (let i = 0; i < state.clips.length; i++) {
      const clip = state.clips[i];
      const item = { info: clip.info, card: null, wave: null };
      if (settings.format === "mp4" && clip.info.kind === "audio" && !fast) {
        setRun({ stage: "cards", detail: titleOf(clip), index: i, count: state.clips.length });
        if (!clip.img && clip.thumb) clip.img = await loadImage(clip.thumb);
        const rect = renderCard(canvas, { template: settings.template, accent: settings.accent, W, H, track: trackOf(clip, i + 1, state.clips.length), cover: clip.img, previewWave: false });
        item.card = canvas.toDataURL("image/jpeg", 0.93);
        if (settings.wave) item.wave = rect;
        await new Promise((r) => setTimeout(r, 0));
      }
      items.push(item);
    }

    const request = {
      items, format: settings.format, output,
      width: W, height: H, fps: Number(settings.fps), fit: settings.fit,
      audioBitrate: Number(settings.bitrate), sampleRate: Number(settings.rate),
      quality: settings.quality, encoder: settings.encoder,
      chapters: settings.chapters, smartCopy: settings.smart,
    };
    state.lastResult = await api.invoke("start_merge", { request });
    showDone(state.lastResult);
  } catch (e) {
    const msg = String(e?.message || e);
    if (msg.includes("cancelled")) { showState(null); toast("Cancelled"); }
    else { $("#errLog").textContent = msg; showState("error"); }
  } finally {
    state.merging = false;
    clearInterval(runTimer);
    renderDeck();
  }
}

function showDone(r) {
  $("#doneFile").textContent = r.output.split(/[\\/]/).pop();
  $("#doneFile").title = r.output;
  const took = r.elapsed < 60 ? `${r.elapsed.toFixed(1)}s` : fmtTime(r.elapsed);
  $("#doneStats").textContent = `${fmtTime(r.duration)} · ${fmtBytes(r.size)} · took ${took}`;
  showState("done");
}

api.listen("merge-progress", (e) => { if (state.merging) setRun(e.payload); });

$("#btnMerge").addEventListener("click", merge);
$("#btnCancel").addEventListener("click", () => { $("#runDetail").textContent = "Stopping…"; api.invoke("cancel_merge"); });
$("#btnDoneClose").addEventListener("click", () => showState(null));
$("#btnErrClose").addEventListener("click", () => showState(null));
$("#btnPlay").addEventListener("click", () => state.lastResult && api.invoke("open_path", { path: state.lastResult.output }).catch((e) => toast(String(e))));
$("#btnReveal").addEventListener("click", () => state.lastResult && api.invoke("reveal_path", { path: state.lastResult.output }).catch((e) => toast(String(e))));

// ─── Links (YouTube etc. via yt-dlp) ───────────────────────────────────
const URL_RE = /https?:\/\/[^\s<>"'`]+/g;
const findUrls = (text) => [...new Set((text || "").match(URL_RE) || [])];

let ytReady = null;
function ensureDownloader() {
  ytReady ??= api.invoke("yt_setup").catch((e) => { ytReady = null; throw e; });
  return ytReady;
}

function placeholder(url, mode, meta = {}) {
  return {
    id: nextId++,
    pending: { url, mode, ytId: meta.id || null, job: null, status: "Waiting…", percent: null },
    info: {
      path: "", name: meta.title || url.replace(/^https?:\/\/(www\.)?/, ""), title: meta.title || null,
      artist: meta.uploader || null, duration: meta.duration || 0, kind: mode === "video" ? "video" : "audio", size: 0,
    },
    thumb: null, img: null,
  };
}

function setPending(clip, status, percent = null) {
  if (!clip.pending) return;
  clip.pending.status = status;
  clip.pending.percent = percent;
  updateRow(clip);
}

function dropPending(clip, err) {
  const msg = String(err?.message || err || "");
  if (state.clips.includes(clip)) {
    state.clips = state.clips.filter((c) => c !== clip);
    state.selected.delete(clip.id);
    render();
  }
  if (!/cancel/i.test(msg)) toast(`Couldn't download “${clip.info.title || clip.info.name}”: ${msg}`);
}

/** Adds one placeholder row per link, expands playlists, then downloads. */
function addLinks(urls, mode = settings.linkMode) {
  for (const url of urls) {
    const ph = placeholder(url, mode);
    ph.pending.status = ytReady ? "Reading link…" : "Preparing downloader…";
    state.clips.push(ph);
    render();
    (async () => {
      try {
        await ensureDownloader();
        setPending(ph, "Reading link…");
        const entries = await api.invoke("yt_resolve", { url });
        if (!state.clips.includes(ph)) return; // removed meanwhile
        const items = entries.map((e) => placeholder(e.url, mode, e));
        flip(() => { state.clips.splice(state.clips.indexOf(ph), 1, ...items); renderList(); });
        render();
        if (items.length > 1) toast(`Adding ${items.length} videos from the playlist`);
        items.forEach((c) => dlQueue.push(c));
        pumpDownloads();
      } catch (e) { dropPending(ph, e); }
    })();
  }
}

const dlQueue = [];
let dlActive = 0;
function pumpDownloads() {
  // one at a time: parallel requests make YouTube rate-limit (HTTP 403)
  while (dlActive < 1 && dlQueue.length) {
    const clip = dlQueue.shift();
    if (!state.clips.includes(clip)) continue;
    dlActive++;
    const job = `j${clip.id}`;
    clip.pending.job = job;
    setPending(clip, "Starting…");
    // videos are fetched no larger than the output frame (smaller = faster)
    api.invoke("yt_download", { job, url: clip.pending.url, id: clip.pending.ytId, mode: clip.pending.mode, maxHeight: Math.min(...resWH()), dir: settings.dlDir })
      .then(async (path) => {
        const [info] = await api.invoke("probe_files", { paths: [path] });
        if (!info || info.error) throw new Error(info?.error || "unreadable file");
        if (!state.clips.includes(clip)) return;
        clip.info = info;
        delete clip.pending;
        render();
        queueThumbs([clip]);
      })
      .catch((e) => dropPending(clip, e))
      .finally(() => { dlActive--; pumpDownloads(); });
  }
}

api.listen("yt-progress", (e) => {
  const { job, percent, status, speed } = e.payload;
  const clip = state.clips.find((c) => c.pending?.job === job);
  const rate = speed ? ` · ${fmtBytes(speed)}/s` : "";
  if (clip) setPending(clip, percent != null && status === "Downloading" ? `Downloading ${Math.floor(percent)}%${rate}` : status, percent);
});
api.listen("yt-setup", (e) => {
  for (const c of state.clips) if (c.pending && !c.pending.job) setPending(c, e.payload);
});

// Ctrl+V anywhere (outside text fields) adds the pasted links right away.
document.addEventListener("paste", (e) => {
  if (e.target.matches?.("input") || !modal.hidden) return;
  const urls = findUrls(e.clipboardData?.getData("text"));
  if (urls.length) { e.preventDefault(); addLinks(urls); }
});

const linkInput = $("#linkInput");
const linkMode = $("#linkMode");
bindSeg("#linkMode", "linkMode");
async function openLinkDialog(text) {
  linkInput.value = text;
  syncSeg(linkMode, "linkMode");
  showState("link");
  setTimeout(() => linkInput.focus(), 30);
  await showDlFolder();
}
let dlFolder = "";
async function showDlFolder() {
  const info = await api.invoke("yt_info", { dir: settings.dlDir }).catch(() => null);
  dlFolder = info?.folder || "";
  // LRM marks keep the path readable inside the rtl (ellipsis-on-left) box
  $("#linkNote").textContent = dlFolder ? `‎Saved to ${dlFolder}‎` : "";
  $("#linkNote").title = dlFolder;
}
$("#btnDlChange").addEventListener("click", async () => {
  const picked = await api.invoke("pick_dir", { title: "Save downloads to", directory: dlFolder || null }).catch(() => null);
  if (!picked) return;
  settings.dlDir = picked;
  save();
  showDlFolder();
});
$("#btnDlOpen").addEventListener("click", () => dlFolder && api.invoke("open_path", { path: dlFolder }).catch((e) => toast(String(e))));
function submitLinks() {
  const urls = findUrls(linkInput.value);
  if (!urls.length) { linkInput.focus(); toast("Paste a link that starts with https://"); return; }
  showState(null);
  addLinks(urls);
}
$("#btnLinkAdd").addEventListener("click", submitLinks);
$("#btnLinkCancel").addEventListener("click", () => showState(null));
linkInput.addEventListener("keydown", (e) => { if (e.key === "Enter") submitLinks(); });

// ─── Buttons, drop, keys, window ───────────────────────────────────────
const addFiles = async () => addPaths(await api.invoke("pick_files"));
async function addFolder() { const p = await api.invoke("pick_folder"); if (p.length) addPaths(p); else toast("No media files in that folder"); }
$("#btnAddFiles").addEventListener("click", addFiles);
$("#emptyState").addEventListener("click", addFiles);

const dz = $("#dropzone");
api.onDrop((ev) => {
  const p = ev.payload;
  if (state.merging) return;
  if (p.type === "enter") dz.hidden = false;
  else if (p.type === "leave") dz.hidden = true;
  else if (p.type === "drop") { dz.hidden = true; addPaths(p.paths); }
});

document.addEventListener("keydown", (e) => {
  const inInput = e.target.matches("input");
  if (!modal.hidden) { if (e.key === "Escape" && !state.merging) showState(null); return; }
  const mod = e.ctrlKey || e.metaKey;
  if (mod && e.key.toLowerCase() === "o") { e.preventDefault(); addFiles(); }
  else if (mod && e.key === "Enter") { e.preventDefault(); merge(); }
  else if (inInput) return;
  else if (mod && e.key.toLowerCase() === "a") { e.preventDefault(); state.selected = new Set(state.clips.map((c) => c.id)); renderList(); }
  else if ((e.key === "Delete" || e.key === "Backspace") && state.selected.size) { e.preventDefault(); removeClips([...state.selected]); }
  else if (e.key === "Escape") { state.selected.clear(); renderList(); menu.hidden = true; }
  else if (mod && (e.key === "ArrowUp" || e.key === "ArrowDown") && state.selected.size === 1) {
    e.preventDefault();
    const id = [...state.selected][0];
    const i = state.clips.findIndex((c) => c.id === id);
    const j = e.key === "ArrowUp" ? i - 1 : i + 1;
    if (j < 0 || j >= state.clips.length) return;
    flip(() => { [state.clips[i], state.clips[j]] = [state.clips[j], state.clips[i]]; renderList(); });
    render();
  }
});

$("#wcMin").addEventListener("click", () => api.win.minimize());
$("#wcMax").addEventListener("click", () => api.win.toggleMaximize());
$("#wcClose").addEventListener("click", () => api.win.close());
$(".titlebar").addEventListener("dblclick", (e) => { if (!e.target.closest(".window-controls")) api.win.toggleMaximize(); });
document.addEventListener("contextmenu", (e) => { if (!e.target.matches("input")) e.preventDefault(); });

// ─── Boot ──────────────────────────────────────────────────────────────
renderEncoders();
render();
(async () => {
  const st = await api.invoke("tool_status").catch((e) => ({ ok: false, message: String(e) }));
  state.toolsOk = st.ok;
  if (!st.ok) {
    $("#toolStatus").hidden = false;
    $("#toolStatus").textContent = "FFmpeg not found";
    $("#toolStatus").title = st.message;
    toast(st.message);
  }
  renderDeck();
  if (st.ok) {
    state.encoders = await api.invoke("detect_encoders").catch(() => []);
    renderEncoders();
  }
})();
