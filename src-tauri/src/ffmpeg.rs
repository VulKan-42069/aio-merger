//! Locating and spawning the ffmpeg / ffprobe binaries.
//!
//! Lookup order:
//!   1. `AIO_MERGER_FFMPEG_DIR` environment variable
//!   2. `<resources>/ffmpeg/` (bundled by the Windows installer)
//!   3. `<exe dir>/ffmpeg/` and `<exe dir>/`
//!   4. every directory on `PATH` (how Linux packages find the system ffmpeg)

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

use tauri::{AppHandle, Manager};
use tokio::process::Command;

#[derive(Clone, Debug)]
pub struct Tools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

static TOOLS: OnceLock<Tools> = OnceLock::new();

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn find_in(dir: &Path) -> Option<Tools> {
    let ffmpeg = dir.join(exe("ffmpeg"));
    let ffprobe = dir.join(exe("ffprobe"));
    (ffmpeg.is_file() && ffprobe.is_file()).then_some(Tools { ffmpeg, ffprobe })
}

pub fn tools(app: &AppHandle) -> Result<Tools, String> {
    if let Some(t) = TOOLS.get() {
        return Ok(t.clone());
    }

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("AIO_MERGER_FFMPEG_DIR") {
        dirs.push(PathBuf::from(d));
    }
    if let Ok(res) = app.path().resource_dir() {
        dirs.push(res.join("ffmpeg"));
    }
    if let Some(exe_dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        dirs.push(exe_dir.join("ffmpeg"));
        dirs.push(exe_dir);
    }
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    if cfg!(unix) {
        dirs.extend(["/usr/bin", "/usr/local/bin", "/opt/homebrew/bin", "/snap/bin"].map(PathBuf::from));
    }

    let found = dirs
        .iter()
        .find_map(|d| find_in(d))
        .ok_or_else(|| {
            if cfg!(windows) {
                "FFmpeg was not found. Reinstall AIO Merger, or place ffmpeg.exe and ffprobe.exe next to the app.".to_string()
            } else {
                "FFmpeg was not found. Install it with your package manager, e.g. `sudo apt install ffmpeg`.".to_string()
            }
        })?;
    let _ = TOOLS.set(found.clone());
    Ok(found)
}

/// A tokio `Command` that never flashes a console window on Windows.
pub fn command(program: &Path) -> Command {
    let mut cmd = Command::new(program);
    cmd.stdin(Stdio::null()).kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Runs a command to completion and returns stdout, or stderr as the error.
pub async fn output(program: &Path, args: &[String]) -> Result<Vec<u8>, String> {
    let out = command(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| format!("could not start {}: {e}", program.display()))?;
    if out.status.success() {
        Ok(out.stdout)
    } else {
        Err(tail(&String::from_utf8_lossy(&out.stderr), 8))
    }
}

/// Last `n` non-empty lines of a log.
pub fn tail(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}
