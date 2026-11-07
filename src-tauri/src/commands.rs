//! Small IPC commands: dialogs, probing, thumbnails, encoder detection.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use serde::Serialize;
use tauri::AppHandle;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::{oneshot, Semaphore};

use crate::ffmpeg;
use crate::merge;
use crate::probe::{self, MediaInfo, MEDIA_EXTENSIONS};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    ok: bool,
    message: String,
    version: Option<String>,
}

#[tauri::command]
pub async fn tool_status(app: AppHandle) -> ToolStatus {
    match ffmpeg::tools(&app) {
        Ok(t) => {
            let version = ffmpeg::output(&t.ffmpeg, &["-version".into()])
                .await
                .ok()
                .and_then(|o| String::from_utf8_lossy(&o).lines().next().map(str::to_string))
                .map(|l| l.split_whitespace().nth(2).unwrap_or("").to_string());
            ToolStatus { ok: true, message: t.ffmpeg.display().to_string(), version }
        }
        Err(e) => ToolStatus { ok: false, message: e, version: None },
    }
}

/// Returns the hardware H.264 encoders that actually work on this machine.
#[tauri::command]
pub async fn detect_encoders(app: AppHandle) -> Vec<String> {
    let Ok(t) = ffmpeg::tools(&app) else { return vec![] };
    let candidates: &[&str] = if cfg!(windows) {
        &["h264_nvenc", "h264_qsv", "h264_amf"]
    } else {
        &["h264_nvenc"]
    };
    let mut handles = Vec::new();
    for enc in candidates {
        let t = t.clone();
        let enc = enc.to_string();
        handles.push(tauri::async_runtime::spawn(async move {
            let args: Vec<String> = [
                "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i",
                "color=c=black:s=320x240:r=30:d=0.3", "-pix_fmt", "yuv420p", "-c:v", &enc, "-f", "null", "-",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect();
            let ok = tokio::time::timeout(std::time::Duration::from_secs(8), ffmpeg::output(&t.ffmpeg, &args))
                .await
                .map(|r| r.is_ok())
                .unwrap_or(false);
            ok.then_some(enc)
        }));
    }
    let mut found = Vec::new();
    for h in handles {
        if let Ok(Some(e)) = h.await {
            found.push(e);
        }
    }
    found
}

#[tauri::command]
pub async fn pick_files(app: AppHandle) -> Vec<String> {
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .set_title("Add songs or videos")
        .add_filter("Audio & video", MEDIA_EXTENSIONS)
        .add_filter("All files", &["*"])
        .pick_files(move |files| {
            let _ = tx.send(files);
        });
    rx.await
        .ok()
        .flatten()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|f| f.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> Vec<String> {
    let (tx, rx) = oneshot::channel();
    app.dialog().file().set_title("Add a folder of media").pick_folder(move |f| {
        let _ = tx.send(f);
    });
    match rx.await.ok().flatten().and_then(|f| f.into_path().ok()) {
        Some(dir) => expand(vec![dir.to_string_lossy().into_owned()]),
        None => vec![],
    }
}

/// A plain folder picker (no media scanning).
#[tauri::command]
pub async fn pick_dir(app: AppHandle, title: String, directory: Option<String>) -> Option<String> {
    let (tx, rx) = oneshot::channel();
    let mut dlg = app.dialog().file().set_title(&title);
    if let Some(d) = directory.filter(|d| Path::new(d).is_dir()) {
        dlg = dlg.set_directory(d);
    }
    dlg.pick_folder(move |f| {
        let _ = tx.send(f);
    });
    rx.await.ok().flatten()?.into_path().ok().map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
pub async fn pick_output(app: AppHandle, default_name: String, format: String, directory: Option<String>) -> Option<String> {
    let (tx, rx) = oneshot::channel();
    let (label, ext) = if format == "mp3" { ("MP3 audio", "mp3") } else { ("MP4 video", "mp4") };
    let mut dlg = app
        .dialog()
        .file()
        .set_title("Save merged file")
        .set_file_name(&default_name)
        .add_filter(label, &[ext]);
    if let Some(dir) = directory.filter(|d| Path::new(d).is_dir()) {
        dlg = dlg.set_directory(dir);
    }
    dlg.save_file(move |f| {
        let _ = tx.send(f);
    });
    let path = rx.await.ok().flatten()?.into_path().ok()?;
    let path = if path.extension().map(|e| e.eq_ignore_ascii_case(ext)).unwrap_or(false) {
        path
    } else {
        let mut s = path.into_os_string();
        s.push(format!(".{ext}"));
        PathBuf::from(s)
    };
    Some(path.to_string_lossy().into_owned())
}

/// Expands folders into the media files they contain (recursively, natural order).
fn expand(paths: Vec<String>) -> Vec<String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>, depth: u8) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort_by(|a, b| natural_cmp(&a.to_string_lossy(), &b.to_string_lossy()));
        for p in entries {
            if p.is_dir() {
                if depth < 6 {
                    walk(&p, out, depth + 1);
                }
            } else if probe::is_media(&p) {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    for p in paths {
        let pb = PathBuf::from(&p);
        if pb.is_dir() {
            walk(&pb, &mut out, 0);
        } else {
            out.push(pb);
        }
    }
    out.into_iter().map(|p| p.to_string_lossy().into_owned()).collect()
}

/// "track 2" < "track 10"
pub fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (a.peek().copied(), b.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, _) => return std::cmp::Ordering::Less,
            (_, None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na: String = std::iter::from_fn(|| a.next_if(|c| c.is_ascii_digit())).collect();
                let nb: String = std::iter::from_fn(|| b.next_if(|c| c.is_ascii_digit())).collect();
                let (ta, tb) = (na.trim_start_matches('0'), nb.trim_start_matches('0'));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
            }
            (Some(x), Some(y)) => {
                let ord = x.to_lowercase().cmp(y.to_lowercase());
                if ord != std::cmp::Ordering::Equal {
                    return ord;
                }
                a.next();
                b.next();
            }
        }
    }
}

#[tauri::command]
pub async fn probe_files(app: AppHandle, paths: Vec<String>) -> Result<Vec<MediaInfo>, String> {
    let tools = ffmpeg::tools(&app)?;
    let files = expand(paths);
    let limit = Arc::new(Semaphore::new(6));
    let mut handles = Vec::with_capacity(files.len());
    for f in files {
        let tools = tools.clone();
        let limit = limit.clone();
        handles.push(tauri::async_runtime::spawn(async move {
            let _permit = limit.acquire_owned().await;
            probe::probe(&tools, &f).await
        }));
    }
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(info) = h.await {
            out.push(info);
        }
    }
    Ok(out)
}

/// A JPEG data URL: a frame for videos, the embedded cover for songs.
#[tauri::command]
pub async fn thumbnail(app: AppHandle, path: String, kind: String, duration: f64) -> Option<String> {
    let tools = ffmpeg::tools(&app).ok()?;
    let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into()];
    if kind == "video" {
        let at = (duration * 0.15).clamp(0.0, 8.0);
        args.extend(["-ss".into(), format!("{at:.2}"), "-i".into(), path]);
        args.extend(["-map".into(), "0:v:0".into(), "-frames:v".into(), "1".into()]);
        args.extend(["-vf".into(), "scale=480:-2".into()]);
    } else {
        args.extend(["-i".into(), path, "-an".into(), "-map".into(), "0:v:0".into(), "-frames:v".into(), "1".into()]);
        args.extend(["-vf".into(), "scale=720:720:force_original_aspect_ratio=decrease".into()]);
    }
    args.extend(["-f", "image2pipe", "-c:v", "mjpeg", "-q:v", "3", "pipe:1"].map(String::from));
    let jpg = tokio::time::timeout(std::time::Duration::from_secs(15), ffmpeg::output(&tools.ffmpeg, &args))
        .await
        .ok()?
        .ok()?;
    if jpg.is_empty() {
        return None;
    }
    Some(format!("data:image/jpeg;base64,{}", base64::engine::general_purpose::STANDARD.encode(jpg)))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FastPath {
    possible: bool,
    reason: String,
}

#[tauri::command]
pub fn check_fast_path(infos: Vec<MediaInfo>, format: String) -> FastPath {
    match merge::fast_path_reason(&infos, &format) {
        Ok(()) => FastPath { possible: true, reason: "All files share the same format. They can be joined without re-encoding.".into() },
        Err(reason) => FastPath { possible: false, reason },
    }
}

/// `path`, or `path (2)`, `path (3)`… whichever does not exist yet.
#[tauri::command]
pub fn unique_path(path: String) -> String {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return path;
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    (2..10_000)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|c| !c.exists())
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or(path)
}

#[tauri::command]
pub fn reveal_path(app: AppHandle, path: String) -> Result<(), String> {
    app.opener().reveal_item_in_dir(path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_path(app: AppHandle, path: String) -> Result<(), String> {
    app.opener().open_path(path, None::<&str>).map_err(|e| e.to_string())
}
