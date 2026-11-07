//! Reading media metadata with ffprobe.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::ffmpeg::{self, Tools};

pub const MEDIA_EXTENSIONS: &[&str] = &[
    // audio
    "mp3", "wav", "flac", "m4a", "aac", "ogg", "oga", "opus", "wma", "aif", "aiff", "alac", "ape", "wv", "mka",
    // video
    "mp4", "m4v", "mkv", "mov", "avi", "webm", "wmv", "flv", "mpg", "mpeg", "ts", "mts", "m2ts", "3gp", "ogv",
];

pub fn is_media(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| MEDIA_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub path: String,
    pub name: String,
    pub size: u64,
    pub duration: f64,
    /// "audio" or "video"
    pub kind: String,
    pub container: String,
    pub video_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<f64>,
    pub audio_codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub bitrate: Option<u64>,
    pub has_audio: bool,
    pub has_cover: bool,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub error: Option<String>,
}

fn num<T: std::str::FromStr>(v: &Value) -> Option<T> {
    match v {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}

fn rate(v: &Value) -> Option<f64> {
    let s = v.as_str()?;
    let (a, b) = s.split_once('/')?;
    let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
    (b > 0.0 && a > 0.0).then(|| a / b)
}

fn tag(tags: &Value, key: &str) -> Option<String> {
    tags.as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))
        .and_then(|(_, v)| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

const IMAGE_CODECS: &[&str] = &["mjpeg", "png", "bmp", "gif", "webp", "tiff"];

pub async fn probe(tools: &Tools, path: &str) -> MediaInfo {
    let p = Path::new(path);
    let mut info = MediaInfo {
        path: path.to_string(),
        name: p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.to_string()),
        size: std::fs::metadata(p).map(|m| m.len()).unwrap_or(0),
        kind: "audio".into(),
        ..Default::default()
    };

    let args: Vec<String> = ["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]
        .iter()
        .map(|s| s.to_string())
        .chain(std::iter::once(path.to_string()))
        .collect();

    let json: Value = match ffmpeg::output(&tools.ffprobe, &args).await {
        Ok(out) => match serde_json::from_slice(&out) {
            Ok(v) => v,
            Err(e) => {
                info.error = Some(format!("unreadable probe output: {e}"));
                return info;
            }
        },
        Err(e) => {
            info.error = Some(if e.is_empty() { "not a playable media file".into() } else { e });
            return info;
        }
    };

    let format = &json["format"];
    info.container = format["format_name"].as_str().unwrap_or("").to_string();
    info.duration = num::<f64>(&format["duration"]).unwrap_or(0.0);
    info.bitrate = num(&format["bit_rate"]);
    let audio_container = ["mp3", "flac", "ogg", "wav", "aiff", "ape", "wv"]
        .iter()
        .any(|c| info.container.split(',').any(|f| f == *c));

    let empty = Vec::new();
    let streams = json["streams"].as_array().unwrap_or(&empty);
    let mut stream_tags = Value::Null;
    for s in streams {
        let codec = s["codec_name"].as_str().unwrap_or("").to_string();
        match s["codec_type"].as_str() {
            Some("video") => {
                let attached = s["disposition"]["attached_pic"].as_i64() == Some(1);
                let still = IMAGE_CODECS.contains(&codec.as_str()) && audio_container;
                if attached || still {
                    info.has_cover = true;
                } else if info.video_codec.is_none() {
                    info.kind = "video".into();
                    info.video_codec = Some(codec);
                    info.width = num(&s["width"]);
                    info.height = num(&s["height"]);
                    info.fps = rate(&s["avg_frame_rate"]).or_else(|| rate(&s["r_frame_rate"]));
                    if info.duration <= 0.0 {
                        info.duration = num(&s["duration"]).unwrap_or(0.0);
                    }
                }
            }
            Some("audio") if !info.has_audio => {
                info.has_audio = true;
                info.audio_codec = Some(codec);
                info.sample_rate = num(&s["sample_rate"]);
                info.channels = num(&s["channels"]);
                if info.duration <= 0.0 {
                    info.duration = num(&s["duration"]).unwrap_or(0.0);
                }
                stream_tags = s["tags"].clone();
            }
            _ => {}
        }
    }

    let tags = &format["tags"];
    info.title = tag(tags, "title").or_else(|| tag(&stream_tags, "title"));
    info.artist = tag(tags, "artist")
        .or_else(|| tag(tags, "album_artist"))
        .or_else(|| tag(&stream_tags, "artist"));
    info.album = tag(tags, "album");

    if !info.has_audio && info.kind == "audio" {
        info.error = Some("no audio or video stream found".into());
    } else if info.duration <= 0.0 {
        info.error = Some("could not determine the duration".into());
    }
    info
}

/// Precise duration of a file we produced ourselves (used for chapter marks).
pub async fn duration(tools: &Tools, path: &Path) -> Option<f64> {
    let args = vec![
        "-v".into(),
        "error".into(),
        "-show_entries".into(),
        "format=duration".into(),
        "-of".into(),
        "default=nw=1:nk=1".into(),
        path.to_string_lossy().into_owned(),
    ];
    let out = ffmpeg::output(&tools.ffprobe, &args).await.ok()?;
    String::from_utf8_lossy(&out).trim().parse().ok()
}
