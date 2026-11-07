//! The merge engine.
//!
//! Strategy
//! --------
//! * **Fast path**: when every input already matches the output format
//!   (all MP3 with one sample rate, or all H.264/AAC video at one size), the
//!   files are joined with the concat demuxer and `-c copy`. No re-encoding.
//! * **Normalise + join**: otherwise every input becomes an intermediate
//!   segment with identical parameters, and the segments are joined.
//!   - MP4: videos are scaled/padded (letterbox, blur-fill or crop) to the
//!     target size and frame rate; songs get the title card rendered by the
//!     UI, optionally with a live waveform, looped for the song's length.
//!   - MP3: every input is decoded to a lossless FLAC segment, then the
//!     whole sequence is encoded once, so the joins stay gapless.
//! * Chapter markers (one per source file) are written into the output.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Instant;

use base64::Engine;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::sync::watch;

use crate::ffmpeg::{self, Tools};
use crate::probe::{self, MediaInfo};

const CANCELLED: &str = "__cancelled__";

#[derive(Default)]
pub struct MergeState {
    cancel: Mutex<Option<watch::Sender<bool>>>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MergeItem {
    pub info: MediaInfo,
    /// JPEG data URL of the title card (songs only, MP4 output only).
    pub card: Option<String>,
    /// Where to overlay the live waveform on that card.
    pub wave: Option<Wave>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Wave {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    /// "rrggbb"
    pub color: String,
    pub opacity: f32,
    /// "cline" | "line" | "p2p"
    pub mode: String,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MergeRequest {
    pub items: Vec<MergeItem>,
    pub format: String,
    pub output: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub fit: String,
    pub audio_bitrate: u32,
    pub sample_rate: u32,
    pub quality: String,
    pub encoder: String,
    pub chapters: bool,
    pub smart_copy: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Progress {
    stage: String,
    detail: String,
    index: usize,
    count: usize,
    percent: f64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MergeResult {
    output: String,
    size: u64,
    duration: f64,
    elapsed: f64,
    fast: bool,
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn start_merge(app: AppHandle, state: State<'_, MergeState>, request: MergeRequest) -> Result<MergeResult, String> {
    let rx = {
        let mut slot = state.cancel.lock().map_err(|e| e.to_string())?;
        if slot.is_some() {
            return Err("A merge is already running.".into());
        }
        let (tx, rx) = watch::channel(false);
        *slot = Some(tx);
        rx
    };
    let result = run(&app, request, rx).await;
    if let Ok(mut slot) = state.cancel.lock() {
        *slot = None;
    }
    result
}

#[tauri::command]
pub fn cancel_merge(state: State<'_, MergeState>) {
    if let Ok(slot) = state.cancel.lock() {
        if let Some(tx) = slot.as_ref() {
            let _ = tx.send(true);
        }
    }
}

// ---------------------------------------------------------------------------
// Fast path detection
// ---------------------------------------------------------------------------

pub fn fast_path_reason(infos: &[MediaInfo], format: &str) -> Result<(), String> {
    let first = infos.first().ok_or("Add some files first.")?;
    if infos.iter().any(|i| i.error.is_some()) {
        return Err("Some files could not be read.".into());
    }
    let same_audio = |i: &MediaInfo| i.sample_rate == first.sample_rate && i.channels == first.channels;
    if format == "mp3" {
        for i in infos {
            if i.kind != "audio" {
                return Err("Videos need their audio extracted, so this merge re-encodes.".into());
            }
            if i.audio_codec.as_deref() != Some("mp3") {
                return Err(format!("“{}” is not an MP3, so this merge re-encodes.", i.name));
            }
            if !same_audio(i) {
                return Err("The MP3s use different sample rates or channel layouts, so this merge re-encodes.".into());
            }
        }
    } else {
        for i in infos {
            if i.kind != "video" {
                return Err("Songs get a rendered title card, so this merge re-encodes.".into());
            }
            if i.video_codec.as_deref() != Some("h264") || i.audio_codec.as_deref() != Some("aac") {
                return Err(format!("“{}” is not H.264 + AAC, so this merge re-encodes.", i.name));
            }
            let same_fps = match (i.fps, first.fps) {
                (Some(a), Some(b)) => (a - b).abs() < 0.01,
                _ => false,
            };
            if i.width != first.width || i.height != first.height || !same_fps || !same_audio(i) {
                return Err("The videos differ in size, frame rate or audio layout, so this merge re-encodes.".into());
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Reporter<'a> {
    app: &'a AppHandle,
    count: usize,
}
impl Reporter<'_> {
    fn send(&self, stage: &str, detail: &str, index: usize, percent: f64) {
        let _ = self.app.emit(
            "merge-progress",
            Progress {
                stage: stage.into(),
                detail: detail.into(),
                index,
                count: self.count,
                percent: (percent * 100.0).clamp(0.0, 100.0),
            },
        );
    }
}

fn display_title(info: &MediaInfo) -> String {
    let stem = Path::new(&info.name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| info.name.clone());
    match (&info.title, &info.artist) {
        (Some(t), Some(a)) => format!("{a} — {t}"),
        (Some(t), None) => t.clone(),
        _ => stem,
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(x), Ok(y)) => x == y,
        _ => {
            let (x, y) = (a.to_string_lossy(), b.to_string_lossy());
            if cfg!(windows) {
                x.eq_ignore_ascii_case(&y)
            } else {
                x == y
            }
        }
    }
}

async fn run(app: &AppHandle, req: MergeRequest, mut cancel: watch::Receiver<bool>) -> Result<MergeResult, String> {
    let started = Instant::now();
    let tools = ffmpeg::tools(app)?;

    if req.items.is_empty() {
        return Err("Add some files first.".into());
    }
    if let Some(bad) = req.items.iter().find(|i| i.info.error.is_some()) {
        return Err(format!("“{}” can't be read. Remove it and try again.", bad.info.name));
    }
    let output = PathBuf::from(&req.output);
    if req.items.iter().any(|i| same_path(Path::new(&i.info.path), &output)) {
        return Err("The output file would overwrite one of the source files. Choose another name.".into());
    }

    let parent = output.parent().filter(|p| !p.as_os_str().is_empty()).map(Path::to_path_buf).unwrap_or_else(std::env::temp_dir);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let name = format!(".aio-merger-{}-{stamp}", std::process::id());
    let tmp_path = if std::fs::create_dir_all(parent.join(&name)).is_ok() {
        parent.join(&name)
    } else {
        let p = std::env::temp_dir().join(&name);
        std::fs::create_dir_all(&p).map_err(|e| format!("could not create a temp folder: {e}"))?;
        p
    };
    let tmp = TempDir(tmp_path);
    #[cfg(windows)]
    hide(&tmp.0);

    let infos: Vec<MediaInfo> = req.items.iter().map(|i| i.info.clone()).collect();
    let fast = req.smart_copy && fast_path_reason(&infos, &req.format).is_ok();
    let report = Reporter { app, count: req.items.len() };
    let total: f64 = infos.iter().map(|i| i.duration.max(0.01)).sum();

    let result = if fast {
        merge_fast(&tools, &req, &tmp.0, &report, total, &mut cancel).await
    } else if req.format == "mp3" {
        merge_mp3(&tools, &req, &tmp.0, &report, total, &mut cancel).await
    } else {
        merge_mp4(&tools, &req, &tmp.0, &report, total, &mut cancel).await
    };

    if let Err(e) = result {
        let _ = std::fs::remove_file(&output);
        return Err(if e == CANCELLED { "Merge cancelled.".into() } else { e });
    }

    report.send("done", "Finished", req.items.len(), 1.0);
    let duration = probe::duration(&tools, &output).await.unwrap_or(total);
    Ok(MergeResult {
        output: output.to_string_lossy().into_owned(),
        size: std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0),
        duration,
        elapsed: started.elapsed().as_secs_f64(),
        fast,
    })
}

#[cfg(windows)]
fn hide(path: &Path) {
    use std::os::windows::process::CommandExt;
    let _ = std::process::Command::new("attrib")
        .arg("+h")
        .arg(path)
        .creation_flags(0x0800_0000)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// --- helpers ---------------------------------------------------------------

fn s<I: IntoIterator<Item = T>, T: Into<String>>(items: I) -> Vec<String> {
    items.into_iter().map(Into::into).collect()
}

/// Path suitable for an ffmpeg concat list.
fn concat_entry(p: &Path) -> String {
    let mut path = p.to_string_lossy().into_owned();
    if cfg!(windows) {
        path = path.replace('\\', "/");
    }
    format!("file '{}'\n", path.replace('\'', "'\\''"))
}

fn write_concat(tmp: &Path, files: &[PathBuf]) -> Result<PathBuf, String> {
    let list = tmp.join("list.txt");
    let body: String = files.iter().map(|f| concat_entry(f)).collect();
    std::fs::write(&list, body).map_err(|e| e.to_string())?;
    Ok(list)
}

fn meta_escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        if matches!(c, '=' | ';' | '#' | '\\' | '\n') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Writes an FFMETADATA file with one chapter per source.
fn write_chapters(tmp: &Path, marks: &[(String, f64)]) -> Result<PathBuf, String> {
    let mut body = String::from(";FFMETADATA1\ntitle=AIO Merger mix\nencoder=AIO Merger\n");
    let mut t = 0.0f64;
    for (title, dur) in marks {
        let (start, end) = ((t * 1000.0).round() as i64, ((t + dur) * 1000.0).round() as i64);
        body.push_str(&format!("\n[CHAPTER]\nTIMEBASE=1/1000\nSTART={start}\nEND={end}\ntitle={}\n", meta_escape(title)));
        t += dur;
    }
    let path = tmp.join("chapters.txt");
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    Ok(path)
}

fn concat_inputs(list: &Path, chapters: Option<&Path>) -> Vec<String> {
    let mut a = s(["-f", "concat", "-safe", "0", "-i"]);
    a.push(list.to_string_lossy().into_owned());
    if let Some(c) = chapters {
        a.push("-i".into());
        a.push(c.to_string_lossy().into_owned());
    }
    a
}

fn chapter_maps(chapters: bool) -> Vec<String> {
    if chapters {
        s(["-map_metadata", "1", "-map_chapters", "1"])
    } else {
        s(["-map_metadata", "-1", "-map_chapters", "-1"])
    }
}

fn even(v: u32) -> u32 {
    (v / 2 * 2).max(2)
}

fn video_codec_args(encoder: &str, quality: &str, fps: u32) -> Vec<String> {
    let q = match quality {
        "high" => 0,
        "small" => 2,
        _ => 1,
    };
    let mut a = match encoder {
        "h264_nvenc" => s(["-c:v", "h264_nvenc", "-preset", "p4", "-rc", "vbr", "-cq", ["19", "23", "28"][q], "-b:v", "0", "-pix_fmt", "yuv420p"]),
        "h264_qsv" => s(["-c:v", "h264_qsv", "-preset", "faster", "-global_quality", ["20", "24", "29"][q], "-pix_fmt", "nv12"]),
        "h264_amf" => s(["-c:v", "h264_amf", "-quality", "speed", "-rc", "cqp", "-qp_i", ["18", "22", "27"][q], "-qp_p", ["20", "24", "29"][q], "-pix_fmt", "nv12"]),
        _ => s(["-c:v", "libx264", "-preset", "veryfast", "-crf", ["18", "21", "26"][q], "-pix_fmt", "yuv420p"]),
    };
    a.extend(s(["-g".to_string(), (fps * 2).to_string()]));
    a
}

async fn run_ffmpeg(
    tools: &Tools,
    args: Vec<String>,
    expected: f64,
    cancel: &mut watch::Receiver<bool>,
    mut on_progress: impl FnMut(f64),
) -> Result<(), String> {
    if *cancel.borrow() {
        return Err(CANCELLED.into());
    }
    let mut full = s(["-hide_banner", "-nostdin", "-y", "-loglevel", "error", "-progress", "pipe:1", "-nostats"]);
    full.extend(args);

    let mut child = ffmpeg::command(&tools.ffmpeg)
        .args(&full)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start ffmpeg: {e}"))?;

    let stdout = child.stdout.take().ok_or("no stdout")?;
    let mut stderr = child.stderr.take().ok_or("no stderr")?;
    let err_task = tauri::async_runtime::spawn(async move {
        let mut buf = String::new();
        let _ = stderr.read_to_string(&mut buf).await;
        buf
    });

    let mut lines = BufReader::new(stdout).lines();
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(l)) => {
                    if let Some(v) = l.strip_prefix("out_time_us=") {
                        if let Ok(us) = v.trim().parse::<f64>() {
                            if expected > 0.0 {
                                on_progress((us / 1e6 / expected).clamp(0.0, 1.0));
                            }
                        }
                    }
                }
                _ => break,
            },
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    let _ = child.kill().await;
                    return Err(CANCELLED.into());
                }
            }
        }
    }

    let status = child.wait().await.map_err(|e| e.to_string())?;
    let log = err_task.await.unwrap_or_default();
    if *cancel.borrow() {
        return Err(CANCELLED.into());
    }
    if status.success() {
        on_progress(1.0);
        Ok(())
    } else {
        let t = ffmpeg::tail(&log, 6);
        Err(if t.is_empty() { format!("ffmpeg failed ({status})") } else { t })
    }
}

// --- fast path -------------------------------------------------------------

async fn merge_fast(
    tools: &Tools,
    req: &MergeRequest,
    tmp: &Path,
    report: &Reporter<'_>,
    total: f64,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    let files: Vec<PathBuf> = req.items.iter().map(|i| PathBuf::from(&i.info.path)).collect();
    let list = write_concat(tmp, &files)?;
    let marks: Vec<(String, f64)> = req.items.iter().map(|i| (display_title(&i.info), i.info.duration)).collect();
    let chapters = if req.chapters { Some(write_chapters(tmp, &marks)?) } else { None };

    let mut args = concat_inputs(&list, chapters.as_deref());
    if req.format == "mp3" {
        args.extend(s(["-map", "0:a:0", "-c", "copy", "-id3v2_version", "3"]));
    } else {
        args.extend(s(["-map", "0:v:0", "-map", "0:a:0", "-c", "copy", "-movflags", "+faststart"]));
    }
    args.extend(chapter_maps(req.chapters));
    args.push(req.output.clone());

    report.send("join", "Joining without re-encoding", 0, 0.0);
    run_ffmpeg(tools, args, total, cancel, |f| {
        report.send("join", "Joining without re-encoding", ((f * report.count as f64) as usize).min(report.count), f)
    })
    .await
}

// --- MP3 -------------------------------------------------------------------

async fn merge_mp3(
    tools: &Tools,
    req: &MergeRequest,
    tmp: &Path,
    report: &Reporter<'_>,
    total: f64,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    const DECODE: f64 = 0.3;
    let sr = req.sample_rate.to_string();
    let mut segments = Vec::new();
    let mut marks = Vec::new();
    let mut done = 0.0;

    for (idx, item) in req.items.iter().enumerate() {
        let info = &item.info;
        let title = display_title(info);
        let seg = tmp.join(format!("seg_{idx:04}.flac"));
        let mut args = Vec::new();
        if info.has_audio {
            args.extend(s(["-i".to_string(), info.path.clone(), "-map".into(), "0:a:0".into(), "-vn".into()]));
            args.extend(s(["-af".to_string(), format!("aresample={sr},aformat=sample_fmts=s16:channel_layouts=stereo")]));
        } else {
            args.extend(s(["-f".to_string(), "lavfi".into(), "-t".into(), format!("{:.3}", info.duration), "-i".into(), format!("anullsrc=r={sr}:cl=stereo")]));
            args.extend(s(["-sample_fmt", "s16"]));
        }
        args.extend(s(["-c:a", "flac", "-compression_level", "0"]));
        args.push(seg.to_string_lossy().into_owned());

        let detail = format!("Reading {title}");
        let dur = info.duration.max(0.01);
        run_ffmpeg(tools, args, dur, cancel, |f| {
            report.send("decode", &detail, idx, (done + f * dur) / total * DECODE)
        })
        .await
        .map_err(|e| if e == CANCELLED { e } else { format!("{}: {e}", info.name) })?;

        let real = probe::duration(tools, &seg).await.unwrap_or(info.duration);
        marks.push((title, real));
        segments.push(seg);
        done += dur;
    }

    let list = write_concat(tmp, &segments)?;
    let chapters = if req.chapters { Some(write_chapters(tmp, &marks)?) } else { None };
    let mut args = concat_inputs(&list, chapters.as_deref());
    args.extend(s(["-map".to_string(), "0:a:0".into(), "-c:a".into(), "libmp3lame".into(), "-b:a".into(), format!("{}k", req.audio_bitrate)]));
    args.extend(s(["-ar".to_string(), sr.clone(), "-id3v2_version".into(), "3".into()]));
    args.extend(chapter_maps(req.chapters));
    args.push(req.output.clone());

    let seq: f64 = marks.iter().map(|m| m.1).sum();
    run_ffmpeg(tools, args, seq, cancel, |f| {
        report.send("encode", "Encoding MP3", report.count, DECODE + f * (1.0 - DECODE))
    })
    .await
}

// --- MP4 -------------------------------------------------------------------

fn fit_filter(fit: &str, w: u32, h: u32, fps: u32) -> String {
    let tail = format!("setsar=1,fps={fps},format=yuv420p[v]");
    match fit {
        "crop" => format!("[0:v:0]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},{tail}"),
        "blur" => {
            let (bw, bh) = (even(w / 6), even(h / 6));
            format!(
                "[0:v:0]fps={fps},split=2[bgsrc][fgsrc];\
                 [bgsrc]scale={bw}:{bh}:force_original_aspect_ratio=increase,crop={bw}:{bh},boxblur=8:2,eq=brightness=-0.06:saturation=1.15,scale={w}:{h}[bg];\
                 [fgsrc]scale={w}:{h}:force_original_aspect_ratio=decrease[fg];\
                 [bg][fg]overlay=(W-w)/2:(H-h)/2,{tail}"
            )
        }
        _ => format!("[0:v:0]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color=black,{tail}"),
    }
}

fn decode_card(card: &str, path: &Path) -> Result<(), String> {
    let b64 = card.split_once("base64,").map(|(_, d)| d).unwrap_or(card);
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.trim()).map_err(|e| format!("bad title card: {e}"))?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}

async fn merge_mp4(
    tools: &Tools,
    req: &MergeRequest,
    tmp: &Path,
    report: &Reporter<'_>,
    total: f64,
    cancel: &mut watch::Receiver<bool>,
) -> Result<(), String> {
    const RENDER: f64 = 0.965;
    let (w, h, fps) = (even(req.width), even(req.height), req.fps.clamp(10, 60));
    let sr = req.sample_rate.to_string();
    let audio_chain = format!("aresample={sr}:async=1,aformat=sample_fmts=fltp:channel_layouts=stereo");
    let venc = video_codec_args(&req.encoder, &req.quality, fps);
    let mut segments = Vec::new();
    let mut marks = Vec::new();
    let mut done = 0.0;

    for (idx, item) in req.items.iter().enumerate() {
        let info = &item.info;
        let title = display_title(info);
        let seg = tmp.join(format!("seg_{idx:04}.mp4"));
        let dur = info.duration.max(0.01);
        let mut args: Vec<String> = Vec::new();
        let filter: String;

        if info.kind == "video" {
            args.extend(s(["-i".to_string(), info.path.clone()]));
            let a_in = if info.has_audio {
                "[0:a:0]"
            } else {
                args.extend(s(["-f".to_string(), "lavfi".into(), "-t".into(), format!("{dur:.3}"), "-i".into(), format!("anullsrc=r={sr}:cl=stereo")]));
                "[1:a:0]"
            };
            filter = format!("{};{a_in}{audio_chain}[a]", fit_filter(&req.fit, w, h, fps));
        } else {
            // Song: loop the title card for the length of the audio.
            match &item.card {
                Some(card) => {
                    let jpg = tmp.join(format!("card_{idx:04}.jpg"));
                    decode_card(card, &jpg)?;
                    args.extend(s(["-loop".to_string(), "1".into(), "-framerate".into(), fps.to_string(), "-i".into(), jpg.to_string_lossy().into_owned()]));
                }
                None => {
                    args.extend(s(["-f".to_string(), "lavfi".into(), "-i".into(), format!("color=c=0x15130f:s={w}x{h}:r={fps}")]));
                }
            }
            args.extend(s(["-i".to_string(), info.path.clone()]));
            let bg = format!("[0:v]scale={w}:{h},setsar=1,fps={fps},format=yuv420p");
            filter = match &item.wave {
                Some(wv) => {
                    let (ww, wh) = (even(wv.w.min(w)), even(wv.h.min(h)));
                    let color = wv.color.trim_start_matches('#');
                    let mode = match wv.mode.as_str() {
                        "line" | "p2p" | "point" => wv.mode.as_str(),
                        _ => "cline",
                    };
                    format!(
                        "{bg}[bg];[1:a:0]{audio_chain},asplit=2[a][wa];\
                         [wa]showwaves=s={ww}x{wh}:mode={mode}:rate={fps}:colors=0x{color}@{op:.2}:scale=sqrt:draw=full,format=rgba[wave];\
                         [bg][wave]overlay={x}:{y}:shortest=1,format=yuv420p[v]",
                        op = wv.opacity.clamp(0.05, 1.0),
                        x = wv.x.min(w - ww),
                        y = wv.y.min(h - wh),
                    )
                }
                None => format!("{bg}[v];[1:a:0]{audio_chain}[a]"),
            };
            args.extend(s(["-shortest".to_string(), "-t".into(), format!("{dur:.3}")]));
        }

        args.extend(s(["-filter_complex".to_string(), filter, "-map".into(), "[v]".into(), "-map".into(), "[a]".into()]));
        args.extend(venc.clone());
        args.extend(s(["-r".to_string(), fps.to_string(), "-fps_mode".into(), "cfr".into()]));
        args.extend(s(["-c:a".to_string(), "aac".into(), "-b:a".into(), format!("{}k", req.audio_bitrate), "-ar".into(), sr.clone(), "-ac".into(), "2".into()]));
        args.extend(s(["-video_track_timescale", "90000", "-map_metadata", "-1", "-map_chapters", "-1"]));
        args.push(seg.to_string_lossy().into_owned());

        let detail = format!("Rendering {title}");
        report.send("render", &detail, idx, done / total * RENDER);
        run_ffmpeg(tools, args, dur, cancel, |f| {
            report.send("render", &detail, idx, (done + f * dur) / total * RENDER)
        })
        .await
        .map_err(|e| if e == CANCELLED { e } else { format!("{}: {e}", info.name) })?;

        let real = probe::duration(tools, &seg).await.unwrap_or(dur);
        marks.push((title, real));
        segments.push(seg);
        done += dur;
    }

    let list = write_concat(tmp, &segments)?;
    let chapters = if req.chapters { Some(write_chapters(tmp, &marks)?) } else { None };
    let mut args = concat_inputs(&list, chapters.as_deref());
    args.extend(s(["-map", "0:v:0", "-map", "0:a:0", "-c", "copy", "-movflags", "+faststart"]));
    args.extend(chapter_maps(req.chapters));
    args.push(req.output.clone());

    let seq: f64 = marks.iter().map(|m| m.1).sum();
    run_ffmpeg(tools, args, seq, cancel, |f| {
        report.send("join", "Joining segments", report.count, RENDER + f * (1.0 - RENDER))
    })
    .await
}
