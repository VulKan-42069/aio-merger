// Title-card templates for songs exported to MP4.
//
// renderCard() paints the static card. When the live waveform is on it returns
// the rectangle (in output pixels) where ffmpeg overlays the real waveform.

export const TEMPLATES = [
  { id: "ember", name: "Ember" },
  { id: "vinyl", name: "Vinyl" },
  { id: "paper", name: "Paper" },
  { id: "signal", name: "Signal" },
];

export const ACCENTS = ["#f4f4f5", "#ffb547", "#ff6a3d", "#3fd3c0", "#a89bff", "#ff5c8a"];

const F_DISPLAY = '"Geist"';
const F_BODY = '"Geist"';
const F_MONO = '"Geist Mono"';

let fontsReady = null;
export function ensureFonts() {
  fontsReady ??= Promise.all([
    document.fonts.load(`700 64px ${F_DISPLAY}`),
    document.fonts.load(`300 64px ${F_DISPLAY}`),
    document.fonts.load(`500 32px ${F_BODY}`),
    document.fonts.load(`600 32px ${F_MONO}`),
  ]).catch(() => {});
  return fontsReady;
}

// ─── helpers ───────────────────────────────────────────────────────────

const pad2 = (n) => String(n).padStart(2, "0");
export function fmtTime(sec) {
  sec = Math.max(0, Math.round(sec || 0));
  const h = Math.floor(sec / 3600), m = Math.floor((sec % 3600) / 60), s = sec % 60;
  return h ? `${h}:${pad2(m)}:${pad2(s)}` : `${m}:${pad2(s)}`;
}

function hash(str) {
  let h = 2166136261;
  for (let i = 0; i < str.length; i++) { h ^= str.charCodeAt(i); h = Math.imul(h, 16777619); }
  return h >>> 0;
}
function rng(seed) {
  let s = seed || 1;
  return () => ((s = Math.imul(s ^ (s >>> 15), 2246822507) ^ Math.imul(s ^ (s >>> 13), 3266489909)) >>> 0) / 4294967296;
}

function hexToRgb(hex) {
  const v = hex.replace("#", "");
  return [parseInt(v.slice(0, 2), 16), parseInt(v.slice(2, 4), 16), parseInt(v.slice(4, 6), 16)];
}
function rgba(hex, a) { const [r, g, b] = hexToRgb(hex); return `rgba(${r},${g},${b},${a})`; }
function luminance(hex) { const [r, g, b] = hexToRgb(hex); return (0.299 * r + 0.587 * g + 0.114 * b) / 255; }

function rr(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.roundRect ? ctx.roundRect(x, y, w, h, r) : ctx.rect(x, y, w, h);
}

function spacing(ctx, px) { if ("letterSpacing" in ctx) ctx.letterSpacing = `${px}px`; }

let noiseTile = null;
function grain(ctx, W, H, alpha) {
  if (!noiseTile) {
    noiseTile = document.createElement("canvas");
    noiseTile.width = noiseTile.height = 180;
    const c = noiseTile.getContext("2d");
    const img = c.createImageData(180, 180);
    const r = rng(7);
    for (let i = 0; i < img.data.length; i += 4) {
      const v = (r() * 255) | 0;
      img.data[i] = img.data[i + 1] = img.data[i + 2] = v;
      img.data[i + 3] = 255;
    }
    c.putImageData(img, 0, 0);
  }
  ctx.save();
  ctx.globalAlpha = alpha;
  ctx.globalCompositeOperation = "overlay";
  ctx.fillStyle = ctx.createPattern(noiseTile, "repeat");
  ctx.fillRect(0, 0, W, H);
  ctx.restore();
}

/** Word-wraps text into at most `maxLines`, shrinking the size until it fits. */
function fit(ctx, text, { weight, family, max, min, width, lines: maxLines }) {
  for (let size = max; size >= min; size -= Math.max(1, Math.round(max / 40))) {
    ctx.font = `${weight} ${size}px ${family}`;
    const lines = wrap(ctx, text, width);
    if (lines.length <= maxLines) return { size, lines };
  }
  ctx.font = `${weight} ${min}px ${family}`;
  const lines = wrap(ctx, text, width);
  if (lines.length > maxLines) {
    lines.length = maxLines;
    let last = lines[maxLines - 1];
    while (last.length > 1 && ctx.measureText(last + "…").width > width) last = last.slice(0, -1);
    lines[maxLines - 1] = last.trimEnd() + "…";
  }
  return { size: min, lines };
}
function wrap(ctx, text, width) {
  const words = text.split(/\s+/).filter(Boolean);
  const lines = [];
  let cur = "";
  for (let w of words) {
    while (ctx.measureText(w).width > width && w.length > 1) {
      // break very long words
      let i = w.length;
      while (i > 1 && ctx.measureText(w.slice(0, i)).width > width) i--;
      if (cur) { lines.push(cur); cur = ""; }
      lines.push(w.slice(0, i));
      w = w.slice(i);
    }
    const t = cur ? `${cur} ${w}` : w;
    if (ctx.measureText(t).width <= width || !cur) cur = t;
    else { lines.push(cur); cur = w; }
  }
  if (cur) lines.push(cur);
  return lines.length ? lines : [""];
}

function drawImageCover(ctx, img, x, y, w, h) {
  const s = Math.max(w / img.width, h / img.height);
  const sw = w / s, sh = h / s;
  ctx.drawImage(img, (img.width - sw) / 2, (img.height - sh) / 2, sw, sh, x, y, w, h);
}

/** A generated cover for songs without artwork. */
function generatedCover(ctx, x, y, size, track, accent) {
  const r = rng(hash(track.title + track.index));
  ctx.save();
  rr(ctx, x, y, size, size, size * 0.05);
  ctx.clip();
  const g = ctx.createLinearGradient(x, y, x + size, y + size);
  g.addColorStop(0, rgba(accent, 1));
  g.addColorStop(1, "#1b1611");
  ctx.fillStyle = g;
  ctx.fillRect(x, y, size, size);
  ctx.globalCompositeOperation = "soft-light";
  for (let i = 0; i < 9; i++) {
    ctx.beginPath();
    ctx.arc(x + size * (0.2 + r() * 0.9), y + size * (0.1 + r() * 0.9), size * (0.15 + r() * 0.6), 0, Math.PI * 2);
    ctx.fillStyle = `rgba(${r() > 0.5 ? "255,255,255" : "0,0,0"},${0.15 + r() * 0.3})`;
    ctx.fill();
  }
  ctx.globalCompositeOperation = "source-over";
  ctx.strokeStyle = "rgba(10,9,7,.35)";
  ctx.lineWidth = size * 0.012;
  for (let i = 1; i < 7; i++) {
    ctx.beginPath();
    ctx.arc(x + size * 0.82, y + size * 0.82, size * 0.12 * i, 0, Math.PI * 2);
    ctx.stroke();
  }
  ctx.fillStyle = "rgba(10,9,7,.82)";
  ctx.font = `700 ${size * 0.34}px ${F_DISPLAY}`;
  ctx.textBaseline = "alphabetic";
  ctx.fillText((track.title.trim()[0] || "♪").toUpperCase(), x + size * 0.08, y + size * 0.42);
  ctx.restore();
}

function cover(ctx, img, x, y, size, radius, track, accent, shadow = true) {
  if (shadow) {
    ctx.save();
    ctx.shadowColor = "rgba(0,0,0,.55)";
    ctx.shadowBlur = size * 0.12;
    ctx.shadowOffsetY = size * 0.05;
    rr(ctx, x, y, size, size, radius);
    ctx.fillStyle = "#000";
    ctx.fill();
    ctx.restore();
  }
  if (img) {
    ctx.save();
    rr(ctx, x, y, size, size, radius);
    ctx.clip();
    drawImageCover(ctx, img, x, y, size, size);
    ctx.restore();
  } else {
    generatedCover(ctx, x, y, size, track, accent);
  }
}

/** Static stand-in for the live waveform, used only in the preview. */
function fakeWave(ctx, rect, color, opacity, seed) {
  const r = rng(seed);
  const n = Math.max(24, Math.floor(rect.w / 9));
  const bw = rect.w / n;
  ctx.save();
  ctx.fillStyle = rgba(color, opacity);
  const mid = rect.y + rect.h / 2;
  for (let i = 0; i < n; i++) {
    const env = 0.35 + 0.65 * Math.abs(Math.sin(i * 0.21 + seed));
    const h = Math.max(2, rect.h * env * (0.25 + r() * 0.75));
    ctx.fillRect(rect.x + i * bw, mid - h / 2, Math.max(1.5, bw * 0.45), h);
  }
  ctx.restore();
}

function blurredBackdrop(ctx, img, W, H, fallback) {
  if (!img) {
    const g = ctx.createRadialGradient(W * 0.3, H * 0.4, 0, W * 0.3, H * 0.4, Math.max(W, H));
    g.addColorStop(0, rgba(fallback, 0.55));
    g.addColorStop(1, "#0d0b09");
    ctx.fillStyle = g;
    ctx.fillRect(0, 0, W, H);
    return;
  }
  // Blur by heavy downscale + smooth upscale; works on every webview.
  const small = document.createElement("canvas");
  small.width = 24; small.height = Math.max(8, Math.round(24 * H / W));
  const sc = small.getContext("2d");
  drawImageCover(sc, img, 0, 0, small.width, small.height);
  const mid = document.createElement("canvas");
  mid.width = 96; mid.height = Math.round(96 * H / W);
  const mc = mid.getContext("2d");
  mc.imageSmoothingQuality = "high";
  mc.drawImage(small, 0, 0, mid.width, mid.height);
  ctx.imageSmoothingQuality = "high";
  ctx.drawImage(mid, -W * 0.05, -H * 0.05, W * 1.1, H * 1.1);
}

function tapeDots(ctx, x, y, w, track, color, dim) {
  const n = Math.min(track.count, 40);
  if (n < 2) return;
  const gap = w / n;
  const cur = Math.min(n - 1, Math.floor((track.index - 1) * n / track.count));
  for (let i = 0; i < n; i++) {
    ctx.fillStyle = i === cur ? color : dim;
    const bh = i === cur ? gap * 0.18 : gap * 0.1;
    rr(ctx, x + i * gap, y - bh / 2, Math.max(2, gap * 0.62), bh, bh / 2);
    ctx.fill();
  }
}

// ─── templates ─────────────────────────────────────────────────────────

function layout(W, H) {
  const ratio = W / H;
  return { u: Math.min(W, H) / 1080, wide: ratio >= 1.3, tall: ratio < 0.8 };
}

function eyebrow(track) {
  return `TRACK ${pad2(track.index)} / ${pad2(track.count)}`;
}

function ember(ctx, W, H, t, img, accent) {
  const { u, wide } = layout(W, H);
  ctx.fillStyle = "#0f0d0a";
  ctx.fillRect(0, 0, W, H);
  let g = ctx.createRadialGradient(W * 0.12, H * 1.05, 0, W * 0.12, H * 1.05, Math.max(W, H) * 0.8);
  g.addColorStop(0, rgba(accent, 0.42));
  g.addColorStop(0.5, rgba(accent, 0.08));
  g.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, W, H);
  g = ctx.createRadialGradient(W * 0.95, -H * 0.1, 0, W * 0.95, -H * 0.1, Math.max(W, H) * 0.6);
  g.addColorStop(0, "rgba(243,231,211,.07)");
  g.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, W, H);

  let wave;
  ctx.textBaseline = "alphabetic";
  if (wide) {
    const size = Math.round(H * 0.5);
    const cx = Math.round(W * 0.085), cy = Math.round((H - size) / 2 - 30 * u);
    cover(ctx, img, cx, cy, size, 26 * u, t, accent);
    const tx = cx + size + 90 * u, tw = W - tx - W * 0.085;
    let y = cy + 40 * u;
    ctx.fillStyle = accent; ctx.font = `600 ${26 * u}px ${F_MONO}`; spacing(ctx, 6 * u);
    ctx.fillText(eyebrow(t), tx, y); spacing(ctx, 0);
    y += 30 * u;
    const ft = fit(ctx, t.title, { weight: 700, family: F_DISPLAY, max: 104 * u, min: 52 * u, width: tw, lines: 2 });
    ctx.fillStyle = "#f3e7d3";
    for (const line of ft.lines) { y += ft.size * 1.1; ctx.fillText(line, tx, y); }
    if (t.artist) {
      y += 64 * u;
      ctx.fillStyle = "#cdbfa8"; ctx.font = `500 ${44 * u}px ${F_BODY}`;
      ctx.fillText(fit(ctx, t.artist, { weight: 500, family: F_BODY, max: 44 * u, min: 30 * u, width: tw, lines: 1 }).lines[0], tx, y);
    }
    wave = { x: tx, y: cy + size - 150 * u, w: tw, h: 150 * u };
    ctx.fillStyle = "rgba(243,231,211,.35)"; ctx.font = `500 ${24 * u}px ${F_MONO}`;
    ctx.fillText([t.album, fmtTime(t.duration)].filter(Boolean).join("  ·  "), cx, H - 70 * u);
    tapeDots(ctx, W * 0.55, H - 78 * u, W * 0.365, t, accent, "rgba(243,231,211,.14)");
  } else {
    const size = Math.round(W * (layout(W, H).tall ? 0.72 : 0.44));
    const cx = Math.round((W - size) / 2), cy = Math.round(H * (layout(W, H).tall ? 0.13 : 0.1));
    cover(ctx, img, cx, cy, size, 26 * u, t, accent);
    const tw = W * 0.84;
    ctx.textAlign = "center";
    let y = cy + size + 90 * u;
    ctx.fillStyle = accent; ctx.font = `600 ${26 * u}px ${F_MONO}`; spacing(ctx, 6 * u);
    ctx.fillText(eyebrow(t), W / 2, y); spacing(ctx, 0);
    const ft = fit(ctx, t.title, { weight: 700, family: F_DISPLAY, max: 84 * u, min: 44 * u, width: tw, lines: 2 });
    y += 16 * u;
    ctx.fillStyle = "#f3e7d3";
    for (const line of ft.lines) { y += ft.size * 1.12; ctx.fillText(line, W / 2, y); }
    if (t.artist) { y += 60 * u; ctx.fillStyle = "#cdbfa8"; ctx.font = `500 ${40 * u}px ${F_BODY}`; ctx.fillText(t.artist, W / 2, y); }
    ctx.textAlign = "left";
    const wh = 140 * u;
    wave = { x: W * 0.08, y: Math.min(H - wh - 90 * u, y + 50 * u), w: W * 0.84, h: wh };
    tapeDots(ctx, W * 0.08, H - 60 * u, W * 0.84, t, accent, "rgba(243,231,211,.14)");
  }
  grain(ctx, W, H, 0.09);
  return { ...wave, color: accent, opacity: 0.9, mode: "cline" };
}

function vinyl(ctx, W, H, t, img, accent) {
  const { u, wide } = layout(W, H);
  blurredBackdrop(ctx, img, W, H, accent);
  ctx.fillStyle = "rgba(10,9,7,.58)";
  ctx.fillRect(0, 0, W, H);
  const v = ctx.createRadialGradient(W / 2, H / 2, Math.min(W, H) * 0.3, W / 2, H / 2, Math.max(W, H) * 0.75);
  v.addColorStop(0, "rgba(0,0,0,0)");
  v.addColorStop(1, "rgba(0,0,0,.6)");
  ctx.fillStyle = v;
  ctx.fillRect(0, 0, W, H);

  const disc = (x, y, r) => {
    ctx.save();
    ctx.shadowColor = "rgba(0,0,0,.6)"; ctx.shadowBlur = r * 0.12; ctx.shadowOffsetY = r * 0.04;
    ctx.beginPath(); ctx.arc(x, y, r, 0, Math.PI * 2); ctx.fillStyle = "#0b0a09"; ctx.fill();
    ctx.restore();
    ctx.save();
    ctx.strokeStyle = "rgba(255,255,255,.045)"; ctx.lineWidth = Math.max(1, r * 0.004);
    for (let k = r * 0.38; k < r * 0.97; k += r * 0.018) { ctx.beginPath(); ctx.arc(x, y, k, 0, Math.PI * 2); ctx.stroke(); }
    const sh = ctx.createLinearGradient(x - r, y - r, x + r, y + r);
    sh.addColorStop(0.3, "rgba(255,255,255,0)"); sh.addColorStop(0.48, "rgba(255,255,255,.10)");
    sh.addColorStop(0.52, "rgba(255,255,255,.10)"); sh.addColorStop(0.7, "rgba(255,255,255,0)");
    ctx.beginPath(); ctx.arc(x, y, r * 0.97, 0, Math.PI * 2); ctx.fillStyle = sh; ctx.fill();
    // label
    const lr = r * 0.34;
    ctx.beginPath(); ctx.arc(x, y, lr, 0, Math.PI * 2); ctx.clip();
    if (img) drawImageCover(ctx, img, x - lr, y - lr, lr * 2, lr * 2);
    else { ctx.fillStyle = accent; ctx.fill(); }
    ctx.restore();
    ctx.beginPath(); ctx.arc(x, y, r * 0.035, 0, Math.PI * 2); ctx.fillStyle = "#0b0a09"; ctx.fill();
  };

  let wave;
  ctx.textBaseline = "alphabetic";
  if (wide) {
    const size = Math.round(H * 0.6);
    const sx = Math.round(W * 0.07), sy = Math.round((H - size) / 2);
    disc(sx + size * 0.78, sy + size / 2, size * 0.47);
    cover(ctx, img, sx, sy, size, 8 * u, t, accent);
    const tx = sx + size * 1.38, tw = W - tx - W * 0.07;
    let y = sy + 70 * u;
    ctx.fillStyle = accent; ctx.font = `600 ${24 * u}px ${F_MONO}`; spacing(ctx, 6 * u);
    ctx.fillText(eyebrow(t), tx, y); spacing(ctx, 0);
    if (t.artist) { y += 70 * u; ctx.fillStyle = "#f3e7d3"; ctx.font = `500 ${40 * u}px ${F_BODY}`; ctx.fillText(fit(ctx, t.artist, { weight: 500, family: F_BODY, max: 40 * u, min: 28 * u, width: tw, lines: 1 }).lines[0], tx, y); }
    const ft = fit(ctx, t.title, { weight: 300, family: F_DISPLAY, max: 92 * u, min: 46 * u, width: tw, lines: 3 });
    y += 16 * u;
    ctx.fillStyle = "#fff";
    for (const line of ft.lines) { y += ft.size * 1.12; ctx.fillText(line, tx, y); }
    wave = { x: tx, y: sy + size - 120 * u, w: tw, h: 120 * u };
  } else {
    const r = W * (layout(W, H).tall ? 0.4 : 0.27);
    const cy = H * (layout(W, H).tall ? 0.3 : 0.32);
    disc(W / 2, cy, r);
    ctx.textAlign = "center";
    let y = cy + r + 100 * u;
    ctx.fillStyle = accent; ctx.font = `600 ${24 * u}px ${F_MONO}`; spacing(ctx, 6 * u);
    ctx.fillText(eyebrow(t), W / 2, y); spacing(ctx, 0);
    const ft = fit(ctx, t.title, { weight: 300, family: F_DISPLAY, max: 80 * u, min: 42 * u, width: W * 0.84, lines: 2 });
    y += 10 * u; ctx.fillStyle = "#fff";
    for (const line of ft.lines) { y += ft.size * 1.12; ctx.fillText(line, W / 2, y); }
    if (t.artist) { y += 58 * u; ctx.fillStyle = "#f3e7d3"; ctx.font = `500 ${38 * u}px ${F_BODY}`; ctx.fillText(t.artist, W / 2, y); }
    ctx.textAlign = "left";
    wave = { x: W * 0.1, y: Math.min(H - 200 * u, y + 50 * u), w: W * 0.8, h: 120 * u };
  }
  grain(ctx, W, H, 0.07);
  return { ...wave, color: "#f3e7d3", opacity: 0.85, mode: "cline" };
}

function paper(ctx, W, H, t, img, accent) {
  const { u, wide } = layout(W, H);
  const ink = "#16130f";
  ctx.fillStyle = "#efe6d5";
  ctx.fillRect(0, 0, W, H);
  grain(ctx, W, H, 0.14);
  const m = (wide ? 110 : 80) * u;
  const dark = luminance(accent) > 0.85 ? "#16130f" : accent;

  ctx.textBaseline = "alphabetic";
  // header rule
  ctx.fillStyle = ink;
  ctx.fillRect(m, m + 60 * u, W - m * 2, 3 * u);
  ctx.font = `600 ${24 * u}px ${F_MONO}`; spacing(ctx, 5 * u);
  ctx.fillText(eyebrow(t), m, m + 34 * u);
  ctx.textAlign = "right";
  ctx.fillText(fmtTime(t.duration), W - m, m + 34 * u);
  ctx.textAlign = "left"; spacing(ctx, 0);

  // big index numeral
  ctx.fillStyle = dark;
  ctx.font = `700 ${(wide ? 300 : 240) * u}px ${F_DISPLAY}`;
  ctx.fillText(pad2(t.index), m - 12 * u, m + 60 * u + (wide ? 300 : 250) * u);

  const csize = (wide ? 300 : 260) * u;
  if (img) cover(ctx, img, W - m - csize, m + 110 * u, csize, 4 * u, t, accent, false);

  const tw = W - m * 2;
  const ft = fit(ctx, t.title, { weight: 700, family: F_DISPLAY, max: (wide ? 120 : 96) * u, min: 50 * u, width: tw, lines: wide ? 2 : 3 });
  const waveH = 110 * u;
  const bottom = H - m - waveH - 40 * u - (t.artist ? 70 * u : 0);
  let y = bottom - (ft.lines.length - 1) * ft.size * 1.05;
  ctx.fillStyle = ink;
  for (const line of ft.lines) { ctx.fillText(line, m - 4 * u, y); y += ft.size * 1.05; }
  if (t.artist) { ctx.font = `500 ${42 * u}px ${F_BODY}`; ctx.fillStyle = "#4a4237"; ctx.fillText(t.artist, m, bottom + 70 * u); }
  return { x: m, y: H - m - waveH, w: W - m * 2, h: waveH, color: "#16130f", opacity: 0.88, mode: "p2p" };
}

function signal(ctx, W, H, t, img, accent) {
  const { u, wide } = layout(W, H);
  const ink = luminance(accent) > 0.55 ? "#120f0b" : "#fbf4e8";
  ctx.fillStyle = accent;
  ctx.fillRect(0, 0, W, H);
  // halftone dots
  ctx.fillStyle = luminance(accent) > 0.55 ? "rgba(0,0,0,.07)" : "rgba(255,255,255,.08)";
  const step = 26 * u;
  for (let yy = 0; yy < H; yy += step) for (let xx = (yy / step) % 2 ? step / 2 : 0; xx < W; xx += step) {
    const d = Math.hypot(xx - W, yy) / Math.hypot(W, H);
    ctx.beginPath(); ctx.arc(xx, yy, step * 0.32 * (1 - d), 0, Math.PI * 2); ctx.fill();
  }
  // giant outlined number
  ctx.save();
  ctx.strokeStyle = ink; ctx.globalAlpha = 0.3; ctx.lineWidth = 4 * u;
  const tall = layout(W, H).tall;
  ctx.font = `800 ${(wide ? 760 : tall ? 560 : 420) * u}px ${F_DISPLAY}`;
  ctx.textAlign = "right"; ctx.textBaseline = "alphabetic";
  ctx.strokeText(pad2(t.index), W + 30 * u, wide ? H * 0.86 : tall ? H * 0.6 : H * 0.62);
  ctx.restore();

  const m = (wide ? 100 : 72) * u;
  const csize = (wide ? 260 : 300) * u;
  ctx.fillStyle = ink;
  ctx.fillRect(m - 8 * u, m - 8 * u, csize + 16 * u, csize + 16 * u);
  cover(ctx, img, m, m, csize, 0, t, accent, false);

  ctx.textBaseline = "alphabetic";
  ctx.fillStyle = ink;
  ctx.font = `700 ${24 * u}px ${F_MONO}`; spacing(ctx, 6 * u);
  ctx.fillText(eyebrow(t), m, m + csize + 70 * u); spacing(ctx, 0);

  const tw = wide ? W * 0.62 : W - m * 2;
  const ft = fit(ctx, t.title.toUpperCase(), { weight: 800, family: F_DISPLAY, max: (wide ? 110 : 92) * u, min: 46 * u, width: tw, lines: 3 });
  let y = H - m - (t.artist ? 70 * u : 0) - (ft.lines.length - 1) * ft.size * 1.02;
  if (!wide) y -= 200 * u;
  for (const line of ft.lines) { ctx.fillText(line, m - 4 * u, y); y += ft.size * 1.02; }
  if (t.artist) { ctx.font = `600 ${40 * u}px ${F_BODY}`; ctx.fillText(t.artist, m, y - ft.size * 1.02 + 70 * u); }

  const wave = wide
    ? { x: m + csize + 70 * u, y: m + csize / 2 - 70 * u, w: W - (m + csize + 70 * u) - m, h: 140 * u }
    : { x: m, y: H - m - 150 * u, w: W - m * 2, h: 130 * u };
  return { ...wave, color: ink.replace("#", "#"), opacity: 0.85, mode: "cline" };
}

const PAINTERS = { ember, vinyl, paper, signal };

/**
 * Paints a title card. Returns the waveform rectangle (rounded to ints).
 * @param {HTMLCanvasElement} canvas
 */
export function renderCard(canvas, { template, accent, W, H, track, cover: img, previewWave }) {
  canvas.width = W;
  canvas.height = H;
  const ctx = canvas.getContext("2d");
  ctx.textAlign = "left";
  const painter = PAINTERS[template] || ember;
  const w = painter(ctx, W, H, track, img, accent);
  const rect = {
    x: Math.max(0, Math.round(w.x)), y: Math.max(0, Math.round(w.y)),
    w: Math.round(Math.min(w.w, W)), h: Math.round(Math.min(w.h, H)),
    color: w.color.replace("#", ""), opacity: w.opacity, mode: w.mode,
  };
  if (previewWave) fakeWave(ctx, rect, w.color, w.opacity, hash(track.title) % 97);
  return rect;
}
