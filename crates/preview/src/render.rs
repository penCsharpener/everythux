//! Dispatch: picks the provider for a path and turns every failure into an `Unavailable` pane.

use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use crate::cancel::Cancel;
use crate::env::Env;
use crate::error::PreviewError;
use crate::kind::{Kind, classify};
use crate::{PreviewContent, PreviewLimits, textfile};

/// Folders with more entries than this report exactly this many.
const MAX_FOLDER_ENTRIES: u64 = 100_000;

/// Builds the preview of `path`.
///
/// `Err(Cancelled)` means the request was superseded; every other failure is folded into
/// [`PreviewContent::Unavailable`] with a hint for the user.
pub(crate) fn render(
    path: &Path,
    limits: PreviewLimits,
    env: &Env,
    cancel: &Cancel,
) -> Result<PreviewContent, PreviewError> {
    cancel.check()?;
    let meta = match fs::metadata(path) {
        Ok(meta) => meta,
        Err(e) => return Ok(unavailable(format!("Cannot read this file: {e}"))),
    };
    if meta.is_dir() {
        return folder(path, cancel);
    }
    if !meta.is_file() {
        // Never open FIFOs, sockets or devices: open/read could block the worker forever.
        return Ok(unavailable("Not a regular file"));
    }

    let too_large = meta.len() > limits.max_file_size;
    let kind = match classify(extension(path).as_deref()) {
        Kind::TypeScriptOrVideo => disambiguate_ts(path),
        kind => kind,
    };
    let built = build(kind, path, too_large, limits, env, cancel);
    match built {
        Ok(content) => Ok(content),
        Err(e) if e.is_cancelled() => Err(PreviewError::Cancelled),
        Err(e) => {
            tracing::debug!(path = %path.display(), error = %e, "preview provider failed");
            if let Some(content) = thumbnail_fallback(path, limits, env, cancel)? {
                return Ok(content);
            }
            Ok(unavailable(e.hint()))
        }
    }
}

fn build(
    kind: Kind,
    path: &Path,
    too_large: bool,
    limits: PreviewLimits,
    env: &Env,
    cancel: &Cancel,
) -> Result<PreviewContent, PreviewError> {
    let size_guard = || {
        if too_large {
            Err(PreviewError::TooLarge {
                limit: limits.max_file_size,
            })
        } else {
            Ok(())
        }
    };
    match kind {
        Kind::Image => {
            size_guard()?;
            image(path, limits, cancel)
        }
        Kind::Text => {
            size_guard()?;
            textfile::read_text(path, false, cancel)
        }
        Kind::Pdf => {
            size_guard()?;
            pdf(path, limits, env, cancel)
        }
        Kind::Audio => audio(path, limits, cancel),
        Kind::Video | Kind::TypeScriptOrVideo => video(path, !too_large, limits, env, cancel),
        Kind::Other => {
            // Whatever the file manager already thumbnailed (office documents, svg, raw photos …).
            if let Some(content) = thumbnail_fallback(path, limits, env, cancel)? {
                return Ok(content);
            }
            size_guard()?;
            textfile::read_text(path, true, cancel)
        }
    }
}

fn unavailable(hint: impl Into<String>) -> PreviewContent {
    PreviewContent::Unavailable { hint: hint.into() }
}

fn extension(path: &Path) -> Option<String> {
    everythux_core::text::extension(path.file_name()?.as_bytes()).map(std::borrow::Cow::into_owned)
}

/// Counts directory entries (capped), checking for cancellation now and then.
fn folder(path: &Path, cancel: &Cancel) -> Result<PreviewContent, PreviewError> {
    let Ok(entries) = fs::read_dir(path) else {
        return Ok(PreviewContent::Folder { children: None });
    };
    let mut count = 0u64;
    for entry in entries {
        if entry.is_err() {
            continue;
        }
        count += 1;
        if count >= MAX_FOLDER_ENTRIES {
            break;
        }
        if count.is_multiple_of(1024) {
            cancel.check()?;
        }
    }
    Ok(PreviewContent::Folder {
        children: Some(count),
    })
}

/// `.ts` files are MPEG transport streams when they carry the 0x47 sync byte every 188 bytes.
fn disambiguate_ts(path: &Path) -> Kind {
    let mut head = [0u8; 189];
    let is_transport_stream = File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok_and(|()| head[0] == 0x47 && head[188] == 0x47);
    if is_transport_stream {
        Kind::Video
    } else {
        Kind::Text
    }
}

// ---- feature-gated providers ---------------------------------------------------------------

#[cfg(feature = "image")]
fn image(path: &Path, limits: PreviewLimits, cancel: &Cancel) -> Result<PreviewContent, PreviewError> {
    let (image, original) = crate::images::load_file(path, limits.max_dim, cancel)?;
    Ok(PreviewContent::Image {
        image,
        original_size: Some(original),
    })
}

#[cfg(not(feature = "image"))]
fn image(_: &Path, _: PreviewLimits, _: &Cancel) -> Result<PreviewContent, PreviewError> {
    Err(PreviewError::FeatureDisabled("image"))
}

#[cfg(feature = "pdf")]
fn pdf(
    path: &Path,
    limits: PreviewLimits,
    env: &Env,
    cancel: &Cancel,
) -> Result<PreviewContent, PreviewError> {
    crate::pdf::render(path, limits.max_dim, &env.tools, cancel)
}

#[cfg(not(feature = "pdf"))]
fn pdf(_: &Path, _: PreviewLimits, _: &Env, _: &Cancel) -> Result<PreviewContent, PreviewError> {
    Err(PreviewError::FeatureDisabled("PDF"))
}

#[cfg(feature = "audio")]
fn audio(path: &Path, limits: PreviewLimits, cancel: &Cancel) -> Result<PreviewContent, PreviewError> {
    crate::audio::read(path, limits.max_dim, cancel)
}

#[cfg(not(feature = "audio"))]
fn audio(_: &Path, _: PreviewLimits, _: &Cancel) -> Result<PreviewContent, PreviewError> {
    Err(PreviewError::FeatureDisabled("audio"))
}

#[cfg(feature = "video")]
fn video(
    path: &Path,
    allow_frame: bool,
    limits: PreviewLimits,
    env: &Env,
    cancel: &Cancel,
) -> Result<PreviewContent, PreviewError> {
    crate::video::render(path, allow_frame, limits.max_dim, env, cancel)
}

#[cfg(not(feature = "video"))]
fn video(_: &Path, _: bool, _: PreviewLimits, _: &Env, _: &Cancel) -> Result<PreviewContent, PreviewError> {
    Err(PreviewError::FeatureDisabled("video"))
}

/// Cached freedesktop thumbnail as an image preview. Errors other than cancellation mean "none".
#[cfg(feature = "image")]
fn thumbnail_fallback(
    path: &Path,
    limits: PreviewLimits,
    env: &Env,
    cancel: &Cancel,
) -> Result<Option<PreviewContent>, PreviewError> {
    let Some(thumb) = env.cached_thumbnail(path) else {
        return Ok(None);
    };
    match crate::images::load_file(&thumb, limits.max_dim, cancel) {
        Ok((image, _)) => Ok(Some(PreviewContent::Image {
            image,
            original_size: None,
        })),
        Err(e) if e.is_cancelled() => Err(e),
        Err(e) => {
            tracing::debug!(thumbnail = %thumb.display(), error = %e, "cached thumbnail not decodable");
            Ok(None)
        }
    }
}

#[cfg(not(feature = "image"))]
#[expect(
    clippy::unnecessary_wraps,
    reason = "same signature as the image-enabled version"
)]
fn thumbnail_fallback(
    _: &Path,
    _: PreviewLimits,
    _: &Env,
    _: &Cancel,
) -> Result<Option<PreviewContent>, PreviewError> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::process::Command;

    use super::*;

    /// Environment with no helper programs and (optionally) a fake thumbnail cache.
    fn env(thumbnail_dir: Option<PathBuf>) -> Env {
        Env {
            #[cfg(any(feature = "pdf", feature = "video"))]
            tools: crate::tools::Tools::with_path("/nonexistent-dir"),
            thumbnail_dir,
        }
    }

    fn run(path: &Path, limits: PreviewLimits, env: &Env) -> PreviewContent {
        render(path, limits, env, &Cancel::never()).expect("not cancelled")
    }

    fn hint(content: &PreviewContent) -> &str {
        match content {
            PreviewContent::Unavailable { hint } => hint,
            other => panic!("expected Unavailable, got {other:?}"),
        }
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).expect("write");
        path
    }

    /// Puts `png` into a fake cache as the `large` thumbnail of `file`.
    #[cfg(feature = "image")]
    fn fake_thumbnail(cache: &Path, file: &Path, png: &[u8]) {
        use crate::thumbnails::thumbnail_name;

        let dir = cache.join("large");
        fs::create_dir_all(&dir).expect("mkdir");
        fs::write(dir.join(format!("{}.png", thumbnail_name(file))), png).expect("write thumb");
    }

    #[cfg(feature = "image")]
    fn small_png() -> Vec<u8> {
        crate::images::testutil::encode(
            &crate::images::testutil::two_tone(32, 16),
            image::ImageFormat::Png,
        )
    }

    #[test]
    fn missing_file_and_special_files_are_unavailable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let limits = PreviewLimits::default();
        assert!(hint(&run(&tmp.path().join("gone.txt"), limits, &env(None))).starts_with("Cannot read"));

        let fifo = tmp.path().join("pipe.txt");
        if Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .is_ok_and(|s| s.success())
        {
            // Must return immediately: opening a FIFO without a writer would block forever.
            assert_eq!(hint(&run(&fifo, limits, &env(None))), "Not a regular file");
        }
    }

    #[test]
    fn folders_report_their_child_count() {
        let tmp = tempfile::tempdir().expect("tempdir");
        for name in ["a", "b", ".hidden"] {
            write(tmp.path(), name, b"x");
        }
        fs::create_dir(tmp.path().join("sub")).expect("mkdir");
        assert_eq!(
            run(tmp.path(), PreviewLimits::default(), &env(None)),
            PreviewContent::Folder { children: Some(4) }
        );
    }

    #[test]
    fn text_by_extension_and_by_sniffing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let limits = PreviewLimits::default();
        let rs = write(tmp.path(), "main.RS", b"fn main() {}\n");
        assert!(
            matches!(run(&rs, limits, &env(None)), PreviewContent::Text { text, .. } if text.starts_with("fn"))
        );
        let makefile = write(tmp.path(), "Makefile", b"all:\n\ttrue\n");
        assert!(matches!(
            run(&makefile, limits, &env(None)),
            PreviewContent::Text { .. }
        ));
        let blob = write(tmp.path(), "blob.bin", &[0, 1, 2, 3, 0xff, 0xfe]);
        assert_eq!(
            hint(&run(&blob, limits, &env(None))),
            "No preview available for this file type"
        );
    }

    #[test]
    fn files_over_the_limit_are_unavailable_with_a_hint() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = write(tmp.path(), "big.txt", &[b'a'; 100]);
        let limits = PreviewLimits {
            max_file_size: 99,
            ..PreviewLimits::default()
        };
        assert!(hint(&run(&file, limits, &env(None))).contains("larger than the preview limit"));
        let at_limit = PreviewLimits {
            max_file_size: 100,
            ..PreviewLimits::default()
        };
        assert!(matches!(
            run(&file, at_limit, &env(None)),
            PreviewContent::Text { .. }
        ));
    }

    #[cfg(feature = "image")]
    #[test]
    fn images_are_decoded_scaled_and_report_original_size() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let big = crate::images::testutil::encode(
            &crate::images::testutil::two_tone(300, 100),
            image::ImageFormat::Png,
        );
        let file = write(tmp.path(), "wide.PNG", &big);
        let limits = PreviewLimits {
            max_dim: 150,
            ..PreviewLimits::default()
        };
        let PreviewContent::Image { image, original_size } = run(&file, limits, &env(None)) else {
            panic!("expected image");
        };
        assert_eq!((image.width, image.height), (150, 50));
        assert_eq!(original_size, Some((300, 100)));
    }

    #[cfg(feature = "image")]
    #[test]
    fn undecodable_image_falls_back_to_cached_thumbnail_then_to_unavailable() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cache = tmp.path().join("thumbs");
        let file = write(tmp.path(), "broken.jpg", b"not a jpeg at all");
        let limits = PreviewLimits::default();
        assert!(hint(&run(&file, limits, &env(Some(cache.clone())))).contains("cannot read this file"));

        fake_thumbnail(&cache, &file, &small_png());
        let PreviewContent::Image { image, original_size } = run(&file, limits, &env(Some(cache))) else {
            panic!("expected the cached thumbnail");
        };
        assert_eq!((image.width, image.height, original_size), (32, 16, None));
    }

    #[cfg(feature = "image")]
    #[test]
    fn unknown_formats_use_the_thumbnail_cache_before_sniffing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cache = tmp.path().join("thumbs");
        // Looks like text, but the file manager has a thumbnail, which is what the user expects.
        let doc = write(tmp.path(), "report.docx", b"PK\x03\x04 pretend zip");
        fake_thumbnail(&cache, &doc, &small_png());
        assert!(matches!(
            run(&doc, PreviewLimits::default(), &env(Some(cache))),
            PreviewContent::Image { .. }
        ));
        assert_eq!(
            hint(&run(&doc, PreviewLimits::default(), &env(None))),
            "No preview available for this file type"
        );
    }

    #[cfg(feature = "image")]
    #[test]
    fn oversized_file_still_uses_its_cached_thumbnail() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cache = tmp.path().join("thumbs");
        let file = write(tmp.path(), "huge.png", &small_png());
        fake_thumbnail(&cache, &file, &small_png());
        let limits = PreviewLimits {
            max_file_size: 1,
            ..PreviewLimits::default()
        };
        assert!(matches!(
            run(&file, limits, &env(Some(cache))),
            PreviewContent::Image { .. }
        ));
    }

    #[cfg(feature = "pdf")]
    #[test]
    fn pdf_without_poppler_says_what_to_install() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = write(tmp.path(), "a.pdf", crate::pdf::testutil::MINIMAL_PDF.as_bytes());
        assert_eq!(
            hint(&run(&file, PreviewLimits::default(), &env(None))),
            "Install poppler-utils for PDF previews"
        );
    }

    #[cfg(feature = "audio")]
    #[test]
    fn audio_metadata_is_read_even_over_the_size_limit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = write(tmp.path(), "tone.wav", &crate::audio::testutil::wav(8000, 1));
        let limits = PreviewLimits {
            max_file_size: 10,
            ..PreviewLimits::default()
        };
        assert!(matches!(
            run(&file, limits, &env(None)),
            PreviewContent::Audio { .. }
        ));
    }

    #[cfg(feature = "video")]
    #[test]
    fn typescript_and_transport_streams_are_told_apart() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let ts_source = write(tmp.path(), "app.ts", b"export const x: number = 1;\n");
        assert_eq!(disambiguate_ts(&ts_source), Kind::Text);
        let short = write(tmp.path(), "short.ts", b"G");
        assert_eq!(
            disambiguate_ts(&short),
            Kind::Text,
            "too short for two sync bytes"
        );

        let mut stream = vec![0u8; 188 * 3];
        for packet in stream.as_chunks_mut::<188>().0 {
            packet[0] = 0x47;
        }
        let mpegts = write(tmp.path(), "clip.ts", &stream);
        assert_eq!(disambiguate_ts(&mpegts), Kind::Video);
        // Without ffmpeg the video provider has nothing to offer, so the pane explains what to install.
        assert_eq!(
            hint(&run(&mpegts, PreviewLimits::default(), &env(None))),
            "Install ffmpeg for video previews"
        );
        assert!(matches!(
            run(&ts_source, PreviewLimits::default(), &env(None)),
            PreviewContent::Text { .. }
        ));
    }

    #[test]
    fn cancelled_request_yields_cancelled() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = write(tmp.path(), "a.txt", b"x");
        let latest = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(2));
        let stale = Cancel::new(latest, 1);
        let err = render(&file, PreviewLimits::default(), &env(None), &stale).expect_err("stale");
        assert!(err.is_cancelled());
    }
}
