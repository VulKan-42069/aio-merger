//! Downloading songs/videos from links (YouTube and anything else yt-dlp supports).
//!
//! yt-dlp and Deno (the JavaScript runtime yt-dlp needs for YouTube) are not
//! bundled. They are fetched into `<app local data>/tools/` the first time a link
//! is pasted, and yt-dlp is self-updated once per session after that.
//! Downloads are saved to `<Music>/AIO Merger/` and reused if pasted again.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{mpsc, watch};

use crate::ffmpeg;

#[derive(Default)]
pub struct YtState {
    jobs: Mutex<HashMap<String, watch::Sender<bool>>>,
    /// true once the tools were checked/updated in this session
    ready: tokio::sync::Mutex<bool>,
}

struct YtTools {
    ytdlp: PathBuf,
    deno: Option<PathBuf>,
}

fn exe(name: &str) -> String {
    if cfg!(windows) { format!("{name}.exe") } else { name.to_string() }
}

fn tools_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_local_data_dir().map(|d| d.join("tools")).map_err(|e| e.to_string())
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join(name)).find(|p| p.is_file())
}

fn find(app: &AppHandle) -> Option<YtTools> {
    let dir = tools_dir(app).ok();
    let local = |n: &str| dir.as_ref().map(|d| d.join(exe(n))).filter(|p| p.is_file());
    let ytdlp = std::env::var_os("AIO_MERGER_YTDLP").map(PathBuf::from).filter(|p| p.is_file())
        .or_else(|| local("yt-dlp"))
        .or_else(|| which(&exe("yt-dlp")))?;
    let deno = local("deno").or_else(|| which(&exe("deno")));
    Some(YtTools { ytdlp, deno })
}

/// Release asset names for this platform: (yt-dlp binary, deno zip).
fn assets() -> Option<(&'static str, &'static str)> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some(("yt-dlp.exe", "deno-x86_64-pc-windows-msvc.zip"))
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(("yt-dlp_linux", "deno-x86_64-unknown-linux-gnu.zip"))
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        Some(("yt-dlp_linux_aarch64", "deno-aarch64-unknown-linux-gnu.zip"))
    } else {
        None
    }
}

fn system_tool(name: &str) -> PathBuf {
    if cfg!(windows) {
        // Windows 10+ ships curl.exe and tar.exe (bsdtar, which also reads zip).
        let p = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32")
            .join(format!("{name}.exe"));
        if p.is_file() { return p; }
    }
    PathBuf::from(name)
}

/// Downloads `url` to `dest`, reporting "<label> · N MB" while it runs.
async fn fetch(app: &AppHandle, label: &str, url: &str, dest: &Path) -> Result<(), String> {
    let mut tmp = dest.as_os_str().to_owned();
    tmp.push(".part");
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    // Slow or flaky connections often drop near the end; resume (-C -) instead of restarting.
    let mut last_err = String::new();
    for attempt in 0..4 {
        match curl_once(app, label, url, &tmp, attempt > 0).await {
            Ok(()) => {
                place(&tmp, dest).await?;
                make_exec(dest);
                return Ok(());
            }
            Err(e) => {
                log(app, &format!("fetch {url} attempt {attempt}: {e}"));
                last_err = e;
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
    }
    // Keep what we have if it looks complete; callers validate (e.g. unzip fails on a truncated zip).
    if std::fs::metadata(&tmp).map(|m| m.len() > 1_000_000).unwrap_or(false) {
        place(&tmp, dest).await?;
        make_exec(dest);
        return Err(format!("partial:{last_err}"));
    }
    let _ = std::fs::remove_file(&tmp);
    Err(last_err)
}

/// Appends a line to `<tools>/setup.log` (for diagnosing download problems).
fn log(app: &AppHandle, line: &str) {
    if let Ok(dir) = tools_dir(app) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("setup.log")) {
            let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
            let _ = writeln!(f, "[{ts}] {line}");
        }
    }
}

async fn curl_once(app: &AppHandle, label: &str, url: &str, tmp: &Path, resume: bool) -> Result<(), String> {
    let mut args: Vec<String> = ["-L", "-f", "-s", "-S", "--retry", "3", "--connect-timeout", "20", "--speed-time", "60", "--speed-limit", "1000"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    if resume && tmp.is_file() {
        args.extend(["-C".into(), "-".into()]);
    }
    args.extend(["-o".into(), tmp.to_string_lossy().into_owned(), url.to_string()]);
    let mut child = ffmpeg::command(&system_tool("curl"))
        .args(&args)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start curl: {e}"))?;
    let mut stderr = child.stderr.take();
    let err_task = tauri::async_runtime::spawn(async move {
        let mut s = String::new();
        if let Some(e) = stderr.as_mut() {
            use tokio::io::AsyncReadExt;
            let _ = e.read_to_string(&mut s).await;
        }
        s
    });
    let _ = app.emit("yt-setup", label.to_string());
    let status = loop {
        tokio::select! {
            st = child.wait() => break st.map_err(|e| e.to_string())?,
            _ = tokio::time::sleep(Duration::from_millis(700)) => {
                let mb = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0) as f64 / 1_048_576.0;
                if mb > 0.0 {
                    let _ = app.emit("yt-setup", format!("{label} · {mb:.0} MB"));
                }
            }
        }
    };
    let errlog = err_task.await.unwrap_or_default();
    if !status.success() {
        return Err(format!("download failed ({status}): {}", ffmpeg::tail(&errlog, 3)));
    }
    Ok(())
}

/// Moves a freshly downloaded file into place. Antivirus scanners often hold
/// new executables open for a few seconds, so retry and fall back to copying.
async fn place(tmp: &Path, dest: &Path) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..60 {
        let _ = std::fs::remove_file(dest);
        match std::fs::rename(tmp, dest) {
            Ok(()) => return Ok(()),
            Err(e) => last = e.to_string(),
        }
        if std::fs::copy(tmp, dest).is_ok() {
            let _ = std::fs::remove_file(tmp);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let _ = std::fs::remove_file(tmp);
    Err(format!("couldn't save {}: {last}", dest.display()))
}

/// Removes half-finished files left by an interrupted setup (older versions used `.download`).
fn clean_leftovers(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for p in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
            let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if ext == "download" || ext == "part" || p.file_name().is_some_and(|n| n == "deno.zip") {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

async fn unzip(zip: &Path, dir: &Path) -> Result<(), String> {
    let (z, d) = (zip.to_string_lossy().into_owned(), dir.to_string_lossy().into_owned());
    if cfg!(windows) {
        return ffmpeg::output(&system_tool("tar"), &["-xf".into(), z, "-C".into(), d]).await.map(|_| ());
    }
    if ffmpeg::output(Path::new("unzip"), &["-o".into(), "-q".into(), z.clone(), "-d".into(), d.clone()]).await.is_ok() {
        return Ok(());
    }
    ffmpeg::output(Path::new("python3"), &["-m".into(), "zipfile".into(), "-e".into(), z, d]).await.map(|_| ())
}

#[cfg(unix)]
fn make_exec(p: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(m) = std::fs::metadata(p) {
        let mut perm = m.permissions();
        perm.set_mode(0o755);
        let _ = std::fs::set_permissions(p, perm);
    }
}
#[cfg(not(unix))]
fn make_exec(_: &Path) {}

/// Where downloads go: the user's chosen folder, else `Downloads` next to the
/// app (follows the install drive), else `Music/AIO Merger` if that isn't writable.
pub fn download_dir(custom: Option<&str>) -> PathBuf {
    if let Some(c) = custom.map(str::trim).filter(|c| !c.is_empty()) {
        return PathBuf::from(c);
    }
    if let Some(d) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("Downloads"))) {
        if writable(&d) {
            return d;
        }
    }
    dirs::audio_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Music")))
        .unwrap_or_else(std::env::temp_dir)
        .join("AIO Merger")
}

fn writable(d: &Path) -> bool {
    if std::fs::create_dir_all(d).is_err() {
        return false;
    }
    let probe = d.join(".aio-write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

fn base_args(t: &YtTools) -> Vec<String> {
    let mut a: Vec<String> = vec!["--ignore-config".into(), "--no-warnings".into()];
    if let Some(deno) = &t.deno {
        a.push("--js-runtimes".into());
        a.push(format!("deno:{}", deno.display()));
    }
    a
}

// ─── Commands ──────────────────────────────────────────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct YtInfo {
    ready: bool,
    folder: String,
}

#[tauri::command]
pub fn yt_info(app: AppHandle, dir: Option<String>) -> YtInfo {
    YtInfo { ready: find(&app).is_some(), folder: download_dir(dir.as_deref()).to_string_lossy().into_owned() }
}

/// Makes sure yt-dlp (and Deno) are present; downloads them on first use and
/// self-updates yt-dlp once per session.
#[tauri::command]
pub async fn yt_setup(app: AppHandle, state: State<'_, YtState>) -> Result<(), String> {
    let mut ready = state.ready.lock().await;
    if *ready && find(&app).is_some() {
        return Ok(());
    }
    let dir = tools_dir(&app)?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    clean_leftovers(&dir);
    let assets = assets();

    let ytdlp = dir.join(exe("yt-dlp"));
    if ytdlp.is_file() {
        // YouTube changes often; keep yt-dlp current (ignore failures, e.g. offline).
        let _ = app.emit("yt-setup", "Checking for downloader updates…");
        let _ = tokio::time::timeout(Duration::from_secs(45), ffmpeg::output(&ytdlp, &["-U".into()])).await;
    } else if let Some((asset, _)) = assets {
        if let Err(e) = fetch(&app, "Getting the downloader (one-time, ~23 MB)", &format!("https://github.com/yt-dlp/yt-dlp/releases/latest/download/{asset}"), &ytdlp).await {
            // never keep a possibly truncated executable
            let _ = std::fs::remove_file(&ytdlp);
            log(&app, &format!("yt-dlp setup failed: {e}"));
            return Err(format!("Couldn't get the downloader: {}", e.trim_start_matches("partial:")));
        }
    } else if which(&exe("yt-dlp")).is_none() {
        return Err("yt-dlp isn't available for this system. Install it with your package manager.".into());
    }

    let deno = dir.join(exe("deno"));
    if !deno.is_file() && which(&exe("deno")).is_none() {
        if let Some((_, asset)) = assets {
            let zip = dir.join("deno.zip");
            let got = fetch(&app, "Getting the YouTube helper (one-time, ~43 MB)", &format!("https://github.com/denoland/deno/releases/latest/download/{asset}"), &zip).await;
            let _ = app.emit("yt-setup", "Unpacking the YouTube helper…");
            // a "partial" zip is kept only if it unzips cleanly
            let unzipped = match got {
                Ok(()) => unzip(&zip, &dir).await,
                Err(e) if e.starts_with("partial:") => unzip(&zip, &dir).await.map_err(|_| e),
                Err(e) => Err(e),
            };
            if let Err(e) = &unzipped {
                log(&app, &format!("deno setup failed: {e}"));
            }
            let _ = std::fs::remove_file(&zip);
            make_exec(&deno);
            if let Err(e) = unzipped.and_then(|_| if deno.is_file() { Ok(()) } else { Err("deno.exe missing after unzip".into()) }) {
                // Without a JS runtime YouTube refuses most downloads (HTTP 403).
                return Err(format!("Couldn't get the YouTube helper (Deno): {e}"));
            }
        }
    }

    *ready = true;
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkEntry {
    id: String,
    url: String,
    title: String,
    uploader: Option<String>,
    duration: f64,
}

fn entry(v: &Value) -> Option<LinkEntry> {
    let id = v["id"].as_str().unwrap_or_default().to_string();
    let url = v["webpage_url"].as_str()
        .or_else(|| v["url"].as_str())
        .map(str::to_string)
        .or_else(|| (!id.is_empty()).then(|| format!("https://www.youtube.com/watch?v={id}")))?;
    Some(LinkEntry {
        title: v["title"].as_str().unwrap_or(&url).to_string(),
        uploader: v["uploader"].as_str().or_else(|| v["channel"].as_str()).map(str::to_string),
        duration: v["duration"].as_f64().unwrap_or(0.0),
        id,
        url,
    })
}

/// Expands a link into one entry per video (playlists → many).
#[tauri::command]
pub async fn yt_resolve(app: AppHandle, url: String) -> Result<Vec<LinkEntry>, String> {
    let t = find(&app).ok_or("The downloader isn't set up yet.")?;
    let mut args = base_args(&t);
    // --no-playlist: a "watch?v=…&list=…" link means that one video, not the whole mix
    args.extend(["--flat-playlist", "-J", "--no-playlist", "--playlist-end", "300"].map(String::from));
    args.push(url.clone());
    let out = tokio::time::timeout(Duration::from_secs(120), ffmpeg::output(&t.ytdlp, &args))
        .await
        .map_err(|_| "Timed out reading the link.".to_string())?
        .map_err(|e| clean_error(&e))?;
    let json: Value = serde_json::from_slice(&out).map_err(|_| "Couldn't read that link.".to_string())?;
    let list: Vec<LinkEntry> = if json["_type"].as_str() == Some("playlist") {
        json["entries"].as_array().map(|a| a.iter().filter_map(entry).collect()).unwrap_or_default()
    } else {
        entry(&json).into_iter().collect()
    };
    if list.is_empty() {
        return Err("No videos found at that link.".into());
    }
    Ok(list)
}

fn clean_error(log: &str) -> String {
    let line = log.lines().rev().find(|l| l.contains("ERROR")).or_else(|| log.lines().rev().find(|l| !l.trim().is_empty()));
    let msg = line.unwrap_or("download failed").trim();
    let msg = msg.trim_start_matches("ERROR:").trim();
    // "[youtube] abc123: Video unavailable" → "Video unavailable"
    let msg = match msg.strip_prefix('[') {
        Some(rest) => rest.split_once("]: ").or_else(|| rest.split_once("] ")).map(|(_, m)| m).unwrap_or(msg),
        None => msg,
    };
    let msg = msg.split_once(": ").filter(|(id, _)| !id.contains(' ') && id.len() <= 20).map(|(_, m)| m).unwrap_or(msg);
    msg.chars().take(200).collect()
}

/// A previously downloaded file for this video id, if any.
fn existing(dir: &Path, id: &str, video: bool) -> Option<String> {
    if id.is_empty() {
        return None;
    }
    let tag = format!("[{id}]");
    std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        name.contains(&tag)
            && name.contains("(video)") == video
            && !["part", "ytdl", "jpg", "webp", "png", "temp", "download"].contains(&ext.as_str())
    }).map(|p| p.to_string_lossy().into_owned())
}

/// Errors that another attempt won't fix.
fn is_permanent(e: &str) -> bool {
    let e = e.to_ascii_lowercase();
    ["unavailable", "private video", "unsupported url", "removed", "copyright", "does not exist", "members-only", "not available in your country"]
        .iter()
        .any(|k| e.contains(k))
}

/// Deletes thumbnails / partial files a failed attempt left behind for this video.
fn clean_partial(dir: &Path, id: &str) {
    if id.is_empty() {
        return;
    }
    let tag = format!("[{id}]");
    if let Ok(rd) = std::fs::read_dir(dir) {
        for p in rd.filter_map(|e| e.ok()).map(|e| e.path()) {
            let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            if name.contains(&tag) && ["part", "ytdl", "jpg", "webp", "png", "temp"].contains(&ext.as_str()) || name.contains(&tag) && name.contains(".part") {
                let _ = std::fs::remove_file(&p);
            }
        }
    }
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct YtProgress {
    job: String,
    percent: Option<f64>,
    status: String,
    speed: Option<f64>,
}

#[tauri::command]
pub async fn yt_download(
    app: AppHandle,
    state: State<'_, YtState>,
    job: String,
    url: String,
    id: Option<String>,
    mode: String,
    max_height: Option<u32>,
    dir: Option<String>,
) -> Result<String, String> {
    let t = find(&app).ok_or("The downloader isn't set up yet.")?;
    let ff = ffmpeg::tools(&app)?;
    let video = mode == "video";
    let dir = download_dir(dir.as_deref());
    std::fs::create_dir_all(&dir).map_err(|e| format!("can't create {}: {e}", dir.display()))?;
    if let Some(found) = id.as_deref().and_then(|i| existing(&dir, i, video)) {
        return Ok(found);
    }

    let result_file = std::env::temp_dir().join(format!("aio-yt-{}-{job}.txt", std::process::id()));
    let _ = std::fs::remove_file(&result_file);
    let ff_dir = ff.ffmpeg.parent().map(Path::to_path_buf).unwrap_or_default();

    let mut common = base_args(&t);
    // Downloads log verbosely (with warnings) to <tools>/last-download.log for diagnosis.
    common.retain(|a| a != "--no-warnings");
    common.push("-v".into());
    if let Ok(dir) = tools_dir(&app) {
        let _ = std::fs::write(dir.join("last-download.log"), format!("url: {url}
mode: {mode}
"));
    }
    common.extend(["--no-playlist", "--newline", "--progress", "--no-mtime", "--embed-metadata", "--embed-thumbnail", "--convert-thumbnails", "jpg"].map(String::from));
    common.extend(["--retries", "5", "--fragment-retries", "5", "--extractor-retries", "3", "--concurrent-fragments", "4"].map(String::from));
    common.extend(["--progress-template".into(), "download:[aio] %(progress.downloaded_bytes)s %(progress.total_bytes)s %(progress.total_bytes_estimate)s %(progress.speed)s".into()]);
    common.extend(["--ffmpeg-location".into(), ff_dir.to_string_lossy().into_owned()]);
    common.extend(["-P".into(), dir.to_string_lossy().into_owned()]);
    common.extend(["--print-to-file".into(), "after_move:filepath".into(), result_file.to_string_lossy().into_owned()]);
    let out_tpl = if video { "%(title).120B [%(id)s] (video).%(ext)s" } else { "%(title).120B [%(id)s].%(ext)s" };

    // Each attempt is more permissive than the last. YouTube regularly blocks
    // particular player clients/formats with HTTP 403; another client usually works.
    // Videos are fetched no larger than the output frame (smaller = much faster).
    let h = max_height.unwrap_or(1080).clamp(360, 2160);
    let v1 = format!("bv*[vcodec^=avc1][height<={h}]+ba[ext=m4a]/bv*[height<={h}]+ba/b[height<={h}]/b");
    let v2 = format!("bv*[height<={h}]+ba/b[height<={h}]/b");
    // Each attempt is more permissive than the last. YouTube regularly blocks
    // particular player clients/formats with HTTP 403; another client usually works.
    let attempts: [(&str, Vec<&str>); 3] = if video {
        [
            ("", vec!["-f", &v1, "--merge-output-format", "mp4"]),
            ("youtube:player_client=default,tv,web_safari,mweb", vec!["-f", &v2, "--merge-output-format", "mp4"]),
            ("youtube:player_client=tv,mweb,android_vr", vec!["-f", "18/b", "--merge-output-format", "mp4"]),
        ]
    } else {
        [
            ("", vec!["-f", "bestaudio[ext=m4a]/bestaudio/best"]),
            ("youtube:player_client=default,tv,web_safari,mweb", vec!["-f", "bestaudio/best", "-x", "--audio-format", "m4a", "--audio-quality", "0"]),
            ("youtube:player_client=tv,mweb,android_vr", vec!["-f", "bestaudio/18/best", "-x", "--audio-format", "m4a", "--audio-quality", "0"]),
        ]
    };

    let (tx, mut cancel) = watch::channel(false);
    state.jobs.lock().map_err(|e| e.to_string())?.insert(job.clone(), tx);
    let mut outcome = Err(String::new());
    for (i, (client, fmt)) in attempts.iter().enumerate() {
        if i > 0 {
            let _ = app.emit("yt-progress", YtProgress { job: job.clone(), percent: None, status: format!("Retrying ({}/{})…", i + 1, attempts.len()), speed: None });
            clean_partial(&dir, id.as_deref().unwrap_or_default());
        }
        let mut args = common.clone();
        if !client.is_empty() {
            args.extend(["--extractor-args".into(), client.to_string()]);
        }
        args.extend(fmt.iter().map(|s| s.to_string()));
        args.extend(["-o".into(), out_tpl.into(), url.clone()]);
        outcome = run(&app, &t.ytdlp, args, &job, &mut cancel).await;
        match &outcome {
            Ok(()) => break,
            Err(e) if e == "cancelled" || is_permanent(e) => break,
            Err(_) => {}
        }
    }
    if let Ok(mut jobs) = state.jobs.lock() {
        jobs.remove(&job);
    }
    if outcome.is_err() {
        clean_partial(&dir, id.as_deref().unwrap_or_default());
    }
    outcome?;

    let path = std::fs::read_to_string(&result_file).unwrap_or_default();
    let _ = std::fs::remove_file(&result_file);
    let path = path.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or_default().to_string();
    if path.is_empty() || !Path::new(&path).is_file() {
        return Err("The download finished but the file wasn't found.".into());
    }
    Ok(path)
}

async fn run(app: &AppHandle, ytdlp: &Path, args: Vec<String>, job: &str, cancel: &mut watch::Receiver<bool>) -> Result<(), String> {
    let mut child = ffmpeg::command(ytdlp)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("could not start yt-dlp: {e}"))?;

    // merge stdout + stderr into one line stream
    let (ltx, mut lines) = mpsc::unbounded_channel::<String>();
    if let Some(out) = child.stdout.take() {
        let ltx = ltx.clone();
        tauri::async_runtime::spawn(async move {
            let mut r = BufReader::new(out).lines();
            while let Ok(Some(l)) = r.next_line().await { let _ = ltx.send(l); }
        });
    }
    if let Some(err) = child.stderr.take() {
        let ltx = ltx.clone();
        tauri::async_runtime::spawn(async move {
            let mut r = BufReader::new(err).lines();
            while let Ok(Some(l)) = r.next_line().await { let _ = ltx.send(l); }
        });
    }
    drop(ltx);

    let send = |percent: Option<f64>, status: &str, speed: Option<f64>| {
        let _ = app.emit("yt-progress", YtProgress { job: job.to_string(), percent, status: status.to_string(), speed });
    };
    send(None, "Starting…", None);
    let mut log = String::new();
    let mut samples = 0u32;
    loop {
        tokio::select! {
            line = lines.recv() => match line {
                Some(l) => {
                    if let Some(rest) = l.strip_prefix("[aio] ") {
                        let n: Vec<Option<f64>> = rest.split_whitespace().map(|x| x.parse().ok()).collect();
                        let done = n.first().copied().flatten();
                        let total = n.get(1).copied().flatten().or(n.get(2).copied().flatten());
                        let speed = n.get(3).copied().flatten();
                        if let (Some(d), Some(tot)) = (done, total) {
                            if tot > 0.0 { send(Some((d / tot * 100.0).min(100.0)), "Downloading", speed); }
                        }
                        samples += 1;
                        if samples % 25 == 1 {
                            log.push_str(&format!("[aio-progress] {rest}
"));
                        }
                    } else {
                        if ["[Merger]", "[FixupM4a]", "[Metadata]", "[EmbedThumbnail]", "[ExtractAudio]", "[VideoConvertor]"].iter().any(|p| l.starts_with(p)) {
                            send(Some(100.0), "Finishing…", None);
                        }
                        log.push_str(&l);
                        log.push('\n');
                    }
                }
                None => break,
            },
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    let _ = child.kill().await;
                    return Err("cancelled".into());
                }
            }
        }
    }
    let status = child.wait().await.map_err(|e| e.to_string())?;
    if let Ok(dir) = tools_dir(app) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("last-download.log")) {
            let _ = writeln!(f, "---- attempt (exit {status})
{log}");
        }
    }
    if status.success() { Ok(()) } else { Err(clean_error(&log)) }
}

#[tauri::command]
pub fn yt_cancel(state: State<'_, YtState>, job: String) {
    if let Ok(jobs) = state.jobs.lock() {
        if let Some(tx) = jobs.get(&job) {
            let _ = tx.send(true);
        }
    }
}
