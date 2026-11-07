// Browser-only stand-in for the Tauri backend, used to preview the UI with
// `npx serve src` (or any static server). Never loaded inside the app.

const SAMPLE = [
  { name: "01 - Midnight Static.mp3", kind: "audio", title: "Midnight Static", artist: "Neon Harbor", album: "Low Tide", duration: 214.3, audioCodec: "mp3", sampleRate: 44100, bitrate: 320000, size: 8.6e6 },
  { name: "Holiday trip - beach.mp4", kind: "video", duration: 96.1, videoCodec: "h264", width: 1920, height: 1080, fps: 29.97, audioCodec: "aac", sampleRate: 48000, size: 61e6 },
  { name: "02 - Paper Lanterns.flac", kind: "audio", title: "Paper Lanterns Over the Quiet River", artist: "Mira Sol", duration: 263.8, audioCodec: "flac", sampleRate: 48000, bitrate: 980000, size: 31e6 },
  { name: "phone clip vertical.mov", kind: "video", duration: 41.5, videoCodec: "hevc", width: 1080, height: 1920, fps: 30, audioCodec: "aac", sampleRate: 44100, size: 44e6 },
  { name: "03 - Glasshouse.m4a", kind: "audio", title: "Glasshouse", artist: "The Amber Lines", duration: 188, audioCodec: "aac", sampleRate: 44100, bitrate: 256000, size: 6.1e6 },
];

export function createMock() {
  const listeners = {};
  let n = 0;
  const handlers = {
    tool_status: () => ({ ok: true, message: "mock", version: "8.1" }),
    detect_encoders: () => new Promise((r) => setTimeout(() => r(["h264_nvenc"]), 600)),
    pick_files: () => SAMPLE.map((s) => `C:\\Users\\me\\Music\\${s.name}`),
    pick_folder: () => SAMPLE.map((s) => `C:\\Users\\me\\Music\\${s.name}`),
    pick_output: ({ defaultName }) => `C:\\Users\\me\\Music\\${defaultName}`,
    unique_path: ({ path }) => path,
    probe_files: ({ paths }) => new Promise((r) => setTimeout(() => r(paths.map((p) => {
      const name = p.split("\\").pop();
      const s = SAMPLE.find((x) => x.name === name) || SAMPLE[n++ % SAMPLE.length];
      return { path: p, container: "", hasAudio: true, hasCover: false, channels: 2, error: null, ...s };
    })), 350)),
    thumbnail: () => null,
    check_fast_path: () => ({ possible: false, reason: "Songs get a rendered title card, so this merge re-encodes." }),
    yt_info: ({ dir }) => ({ ready: false, folder: dir || "D:\\Apps\\AIO Merger\\Downloads" }),
    pick_dir: () => "E:\\Media\\YouTube",
    yt_setup: () => new Promise((r) => { setTimeout(() => (listeners["yt-setup"] || []).forEach((cb) => cb({ payload: "Getting the downloader (one-time)…" })), 300); setTimeout(r, 1500); }),
    yt_resolve: ({ url }) => new Promise((r) => setTimeout(() => r(url.includes("list=")
      ? [1, 2, 3].map((i) => ({ id: `pl${i}`, url: `${url}&i=${i}`, title: `Playlist song ${i}`, uploader: "Some Channel", duration: 180 + i * 7 }))
      : [{ id: "abc", url, title: "Lofi Rain — Study Session", uploader: "Chill Channel", duration: 245 }]), 700)),
    yt_download: ({ job }) => new Promise((resolve, reject) => {
      let p = 0;
      handlers._yt ||= {};
      const t = setInterval(() => {
        if (handlers._yt[job]) { clearInterval(t); reject("cancelled"); return; }
        p += 4;
        (listeners["yt-progress"] || []).forEach((cb) => cb({ payload: { job, percent: Math.min(p, 100), status: p >= 100 ? "Finishing…" : "Downloading" } }));
        if (p >= 104) { clearInterval(t); resolve(`C:\\Users\\me\\Music\\AIO Merger\\${SAMPLE[0].name}`); }
      }, 90);
    }),
    yt_cancel: ({ job }) => { (handlers._yt ||= {})[job] = true; },
    open_path: () => {}, reveal_path: () => {}, cancel_merge: () => { handlers._cancel = true; },
    start_merge: ({ request }) => new Promise((resolve, reject) => {
      handlers._cancel = false;
      let p = 0;
      const t = setInterval(() => {
        if (handlers._cancel) { clearInterval(t); reject("Merge cancelled."); return; }
        p += 1.5;
        const idx = Math.min(request.items.length - 1, Math.floor((p / 100) * request.items.length));
        (listeners["merge-progress"] || []).forEach((cb) => cb({ payload: { stage: "render", detail: `Rendering ${request.items[idx].info.title || request.items[idx].info.name}`, index: idx, count: request.items.length, percent: p } }));
        if (p >= 100) { clearInterval(t); resolve({ output: request.output, size: 312e6, duration: 803.7, elapsed: 41.2, fast: false }); }
      }, 80);
    }),
  };
  return {
    invoke: async (cmd, args) => { const h = handlers[cmd]; if (!h) throw `mock: no ${cmd}`; return h(args || {}); },
    listen: async (ev, cb) => { (listeners[ev] ||= []).push(cb); return () => {}; },
    win: { minimize() {}, toggleMaximize() {}, close() {} },
    onDrop: () => {},
  };
}
