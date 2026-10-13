//! Video: metadata from `ffprobe`; thumbnail from the freedesktop cache, `ffmpegthumbnailer` or `ffmpeg`.

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use serde_json::Value;

use crate::cancel::Cancel;
use crate::env::Env;
use crate::error::PreviewError;
use crate::tools::run_capture;
use crate::{ImageData, MediaMeta, PreviewContent, images};

const HINT: &str = "Install ffmpeg for video previews";
const TIMEOUT: Duration = Duration::from_secs(5);

/// Builds the video preview.
///
/// `allow_frame` is false for files over the size limit: they still get cached thumbnails and
/// `ffprobe` metadata (headers only), but no frame extraction.
pub(crate) fn render(
    path: &Path,
    allow_frame: bool,
    max_dim: u32,
    env: &Env,
    cancel: &Cancel,
) -> Result<PreviewContent, PreviewError> {
    let abs = std::path::absolute(path)?;
    let ffprobe = env.tools.locate("ffprobe");
    let ffmpeg = env.tools.locate("ffmpeg");
    let thumbnailer = env.tools.locate("ffmpegthumbnailer");

    let mut meta = match &ffprobe {
        Some(bin) => probe(bin, &abs, cancel).or_else(|e| soft(e, path))?,
        None => None,
    };
    let duration = meta.as_ref().and_then(|m| m.duration);

    let mut thumbnail = match env.cached_thumbnail(path) {
        Some(cached) => images::load_file(&cached, max_dim, cancel)
            .map(|(img, _)| Some(img))
            .or_else(|e| soft(e, path))?,
        None => None,
    };
    if thumbnail.is_none() && allow_frame {
        thumbnail = if let Some(bin) = &thumbnailer {
            thumbnailer_frame(bin, &abs, max_dim, cancel).or_else(|e| soft(e, path))?
        } else if let Some(bin) = &ffmpeg {
            ffmpeg_frame(bin, &abs, duration, max_dim, cancel).or_else(|e| soft(e, path))?
        } else {
            None
        };
    }

    if ffprobe.is_none() && ffmpeg.is_none() && thumbnailer.is_none() && thumbnail.is_none() {
        return Err(PreviewError::ToolMissing {
            tool: "ffprobe",
            hint: HINT,
        });
    }
    let mut meta = meta.take().unwrap_or_default();
    if thumbnail.is_none() && ffmpeg.is_none() && thumbnailer.is_none() {
        meta.extra.push((
            "Hint".to_owned(),
            "Install ffmpeg for video thumbnails".to_owned(),
        ));
    }
    Ok(PreviewContent::Video { meta, thumbnail })
}

/// Helper failures other than cancellation degrade the preview instead of failing it.
fn soft<T>(err: PreviewError, path: &Path) -> Result<Option<T>, PreviewError> {
    if err.is_cancelled() {
        return Err(err);
    }
    tracing::debug!(path = %path.display(), error = %err, "video helper failed");
    Ok(None)
}

fn probe(bin: &Path, path: &Path, cancel: &Cancel) -> Result<Option<MediaMeta>, PreviewError> {
    let args = [
        "-v",
        "quiet",
        "-print_format",
        "json",
        "-show_format",
        "-show_streams",
    ]
    .map(OsString::from);
    let mut args = args.to_vec();
    args.push(path.as_os_str().to_owned());
    let json = run_capture("ffprobe", bin, &args, TIMEOUT, cancel)?;
    parse_probe(&json).map(Some)
}

/// Turns `ffprobe -print_format json -show_format -show_streams` output into [`MediaMeta`].
pub(crate) fn parse_probe(json: &[u8]) -> Result<MediaMeta, PreviewError> {
    let root: Value =
        serde_json::from_slice(json).map_err(|e| PreviewError::Decode(format!("ffprobe: {e}")))?;
    let format = &root["format"];
    let streams = root["streams"].as_array().map_or(&[][..], Vec::as_slice);
    let stream_of = |kind: &str, real_video: bool| {
        streams.iter().find(|s| {
            s["codec_type"] == kind && (!real_video || s["disposition"]["attached_pic"].as_i64() != Some(1))
        })
    };
    let video = stream_of("video", true);
    let audio = stream_of("audio", false);

    let num = |v: &Value| {
        v.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or_else(|| v.as_f64())
    };
    let duration = num(&format["duration"])
        .or_else(|| video.and_then(|v| num(&v["duration"])))
        .filter(|s| s.is_finite() && *s > 0.0)
        .and_then(|s| Duration::try_from_secs_f64(s).ok());

    let tag = |key: &str| {
        format["tags"].as_object().and_then(|tags| {
            tags.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .and_then(|(_, v)| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
    };

    let mut extra = Vec::new();
    let mut push = |key: &str, value: Option<String>| {
        if let Some(value) = value {
            extra.push((key.to_owned(), value));
        }
    };
    push(
        "Container",
        format["format_long_name"].as_str().map(str::to_owned),
    );
    if let Some(v) = video {
        push("Video codec", v["codec_name"].as_str().map(str::to_owned));
        if let (Some(w), Some(h)) = (v["width"].as_u64(), v["height"].as_u64()) {
            push("Resolution", Some(format!("{w}×{h}")));
        }
        push("Frame rate", v["avg_frame_rate"].as_str().and_then(frame_rate));
    }
    if let Some(a) = audio {
        push("Audio codec", a["codec_name"].as_str().map(str::to_owned));
        push("Channels", a["channels"].as_u64().map(|c| c.to_string()));
        push(
            "Sample rate",
            num(&a["sample_rate"]).map(|hz| format!("{hz:.0} Hz")),
        );
    }
    push(
        "Bitrate",
        num(&format["bit_rate"]).map(|bps| format!("{:.0} kbps", bps / 1000.0)),
    );

    Ok(MediaMeta {
        title: tag("title"),
        artist: tag("artist"),
        album: tag("album"),
        duration,
        extra,
    })
}

/// `"30000/1001"` -> `"29.97 fps"`, `"25/1"` -> `"25 fps"`, `"0/0"` -> `None`.
fn frame_rate(ratio: &str) -> Option<String> {
    let (n, d) = ratio.split_once('/')?;
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    if d == 0.0 || n <= 0.0 {
        return None;
    }
    let fps = n / d;
    Some(if (fps - fps.round()).abs() < 0.005 {
        format!("{fps:.0} fps")
    } else {
        format!("{fps:.2} fps")
    })
}

/// A frame at 10 % of the duration (or the start when unknown) through `ffmpeg`.
fn ffmpeg_frame(
    bin: &Path,
    path: &Path,
    duration: Option<Duration>,
    max_dim: u32,
    cancel: &Cancel,
) -> Result<Option<ImageData>, PreviewError> {
    let seek = duration.map_or(0.0, |d| d.as_secs_f64() * 0.1);
    for seek in [seek, 0.0] {
        let png = run_capture("ffmpeg", bin, &ffmpeg_args(path, seek, max_dim), TIMEOUT, cancel)?;
        if !png.is_empty() {
            return images::load_bytes(&png, max_dim, cancel).map(|(img, _)| Some(img));
        }
        if seek == 0.0 {
            break;
        }
    }
    Ok(None)
}

fn ffmpeg_args(path: &Path, seek_secs: f64, max_dim: u32) -> Vec<OsString> {
    // Fit inside max_dim × max_dim without ever upscaling; quotes keep the commas inside the filter.
    let filter =
        format!("scale=w='min(iw,{max_dim})':h='min(ih,{max_dim})':force_original_aspect_ratio=decrease");
    let mut args: Vec<OsString> = ["-nostdin", "-hide_banner", "-loglevel", "error", "-ss"]
        .map(OsString::from)
        .into();
    args.push(format!("{seek_secs:.3}").into());
    args.push("-i".into());
    args.push(path.as_os_str().to_owned());
    args.extend(
        ["-frames:v", "1", "-vf"]
            .map(OsString::from)
            .into_iter()
            .chain([OsString::from(filter)])
            .chain(["-f", "image2pipe", "-vcodec", "png", "-"].map(OsString::from)),
    );
    args
}

/// A frame at 10 % through `ffmpegthumbnailer`, which writes to a file.
fn thumbnailer_frame(
    bin: &Path,
    path: &Path,
    max_dim: u32,
    cancel: &Cancel,
) -> Result<Option<ImageData>, PreviewError> {
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("frame.png");
    let args: Vec<OsString> = vec![
        "-i".into(),
        path.as_os_str().to_owned(),
        "-o".into(),
        out.clone().into_os_string(),
        "-s".into(),
        max_dim.to_string().into(),
        "-t".into(),
        "10%".into(),
        "-c".into(),
        "png".into(),
    ];
    run_capture("ffmpegthumbnailer", bin, &args, TIMEOUT, cancel)?;
    let png = std::fs::read(&out)?;
    images::load_bytes(&png, max_dim, cancel).map(|(img, _)| Some(img))
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::tools::Tools;

    const SAMPLE_PROBE: &str = r#"{
      "streams": [
        {"codec_type":"video","codec_name":"h264","width":1920,"height":1080,"avg_frame_rate":"30000/1001","duration":"61.5"},
        {"codec_type":"audio","codec_name":"aac","channels":2,"sample_rate":"48000"}
      ],
      "format": {"format_long_name":"Matroska / WebM","duration":"61.250000","bit_rate":"1500000",
                 "tags":{"TITLE":" Demo ","artist":"Someone"}}
    }"#;

    #[test]
    fn parses_ffprobe_json() {
        let meta = parse_probe(SAMPLE_PROBE.as_bytes()).expect("parse");
        assert_eq!(meta.title.as_deref(), Some("Demo"));
        assert_eq!(meta.artist.as_deref(), Some("Someone"));
        assert_eq!(meta.album, None);
        assert_eq!(meta.duration, Some(Duration::from_secs_f64(61.25)));
        let extra: Vec<(&str, &str)> = meta.extra.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(
            extra,
            [
                ("Container", "Matroska / WebM"),
                ("Video codec", "h264"),
                ("Resolution", "1920×1080"),
                ("Frame rate", "29.97 fps"),
                ("Audio codec", "aac"),
                ("Channels", "2"),
                ("Sample rate", "48000 Hz"),
                ("Bitrate", "1500 kbps"),
            ]
        );
    }

    #[test]
    fn cover_art_streams_are_not_the_video_stream() {
        let json = r#"{"streams":[
            {"codec_type":"video","codec_name":"mjpeg","width":10,"height":10,"disposition":{"attached_pic":1}},
            {"codec_type":"video","codec_name":"vp9","width":640,"height":360,"avg_frame_rate":"25/1"}],
            "format":{}}"#;
        let meta = parse_probe(json.as_bytes()).expect("parse");
        assert!(meta.extra.contains(&("Video codec".to_owned(), "vp9".to_owned())));
        assert!(
            meta.extra
                .contains(&("Frame rate".to_owned(), "25 fps".to_owned()))
        );
        assert_eq!(meta.duration, None);
    }

    #[test]
    fn invalid_json_is_an_error() {
        assert!(parse_probe(b"not json").is_err());
    }

    #[test]
    fn frame_rates() {
        assert_eq!(frame_rate("25/1").as_deref(), Some("25 fps"));
        assert_eq!(frame_rate("0/0"), None);
        assert_eq!(frame_rate("garbage"), None);
    }

    /// Generates a 1 s 320×240 test clip, or `None` when `ffmpeg` is unavailable.
    fn make_clip(dir: &Path, name: &str) -> Option<std::path::PathBuf> {
        let ffmpeg = Tools::from_env().locate("ffmpeg")?;
        let out = dir.join(name);
        let status = Command::new(ffmpeg)
            .args([
                "-v",
                "error",
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=320x240:rate=10",
            ])
            .args(["-pix_fmt", "yuv420p"])
            .arg(&out)
            .status()
            .ok()?;
        status.success().then_some(out)
    }

    fn env_with(path: &str, thumbs: Option<&Path>) -> Env {
        Env {
            tools: Tools::with_path(path),
            thumbnail_dir: thumbs.map(Path::to_path_buf),
        }
    }

    #[test]
    fn real_clip_gets_metadata_and_frame() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let Some(clip) = make_clip(tmp.path(), "clip.mp4") else {
            eprintln!("skipped: ffmpeg not installed");
            return;
        };
        let env = Env {
            tools: Tools::from_env(),
            thumbnail_dir: None,
        };
        let PreviewContent::Video { meta, thumbnail } =
            render(&clip, true, 200, &env, &Cancel::never()).expect("render")
        else {
            panic!("expected video");
        };
        let secs = meta.duration.expect("duration").as_secs_f64();
        assert!((0.9..1.2).contains(&secs), "duration {secs}");
        assert!(
            meta.extra
                .contains(&("Resolution".to_owned(), "320×240".to_owned())),
            "{:?}",
            meta.extra
        );
        assert!(
            meta.extra
                .contains(&("Video codec".to_owned(), "h264".to_owned()))
        );
        let thumb = thumbnail.expect("thumbnail from ffmpeg");
        assert_eq!((thumb.width, thumb.height), (200, 150), "fitted into 200x200");
        assert_eq!(thumb.rgba.len(), 200 * 150 * 4);
    }

    #[test]
    fn small_video_is_not_upscaled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let Some(clip) = make_clip(tmp.path(), "clip.mp4") else {
            return;
        };
        let env = Env {
            tools: Tools::from_env(),
            thumbnail_dir: None,
        };
        let PreviewContent::Video { thumbnail, .. } =
            render(&clip, true, 768, &env, &Cancel::never()).expect("render")
        else {
            panic!("expected video");
        };
        let thumb = thumbnail.expect("thumbnail");
        assert_eq!((thumb.width, thumb.height), (320, 240));
    }

    #[test]
    fn over_limit_video_gets_metadata_but_no_frame() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let Some(clip) = make_clip(tmp.path(), "clip.mkv") else {
            return;
        };
        let env = Env {
            tools: Tools::from_env(),
            thumbnail_dir: None,
        };
        let PreviewContent::Video { meta, thumbnail } =
            render(&clip, false, 200, &env, &Cancel::never()).expect("render")
        else {
            panic!("expected video");
        };
        assert!(meta.duration.is_some());
        assert!(thumbnail.is_none());
    }

    #[test]
    fn cached_thumbnail_is_preferred_over_extraction() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let clip = tmp.path().join("movie.mp4");
        std::fs::write(&clip, b"not really a video").expect("write");
        let thumbs = tmp.path().join("thumbs");
        let name = format!("{}.png", crate::thumbnails::thumbnail_name(&clip));
        std::fs::create_dir_all(thumbs.join("large")).expect("mkdir");
        let png = images::testutil::encode(&images::testutil::two_tone(64, 32), image::ImageFormat::Png);
        std::fs::write(thumbs.join("large").join(name), png).expect("write thumb");

        // No helper programs at all: the cached thumbnail alone makes a preview.
        let env = env_with("/nonexistent-dir", Some(&thumbs));
        let PreviewContent::Video { meta, thumbnail } =
            render(&clip, true, 768, &env, &Cancel::never()).expect("render")
        else {
            panic!("expected video");
        };
        let thumb = thumbnail.expect("cached thumbnail");
        assert_eq!((thumb.width, thumb.height), (64, 32));
        assert!(
            meta.extra.iter().all(|(k, _)| k != "Hint"),
            "a thumbnail exists, so there is nothing to install"
        );
    }

    #[test]
    fn without_any_tool_or_thumbnail_the_hint_is_an_install_hint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let clip = tmp.path().join("movie.mp4");
        std::fs::write(&clip, b"x").expect("write");
        let env = env_with("/nonexistent-dir", None);
        let err = render(&clip, true, 768, &env, &Cancel::never()).expect_err("nothing available");
        assert_eq!(err.hint(), HINT);
    }

    #[test]
    fn broken_video_degrades_instead_of_failing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let clip = tmp.path().join("broken.mp4");
        std::fs::write(&clip, b"this is not an mp4").expect("write");
        let env = Env {
            tools: Tools::from_env(),
            thumbnail_dir: None,
        };
        if env.tools.locate("ffprobe").is_none() {
            return;
        }
        let content = render(&clip, true, 200, &env, &Cancel::never()).expect("degrades");
        assert!(
            matches!(content, PreviewContent::Video { thumbnail: None, .. }),
            "{content:?}"
        );
    }
}
