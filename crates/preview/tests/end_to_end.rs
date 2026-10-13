//! End-to-end checks through the public API: real worker thread, real fixture files, real helper
//! programs where installed (those cases skip when the tool is missing).

use std::fs;
#[cfg(feature = "image")]
use std::io::Cursor;
use std::path::Path;
#[cfg(any(feature = "pdf", feature = "video"))]
use std::process::Command;
use std::time::Duration;

use crossbeam_channel::Receiver;
use everythux_preview::{PreviewContent, PreviewLimits, PreviewResult, PreviewService};
#[cfg(feature = "image")]
use image::{ImageFormat, Rgb, RgbImage};

/// Only bounds how long a failing test waits; never used to wait for correctness.
const WAIT: Duration = Duration::from_secs(30);

#[cfg(any(feature = "pdf", feature = "video"))]
fn tool_available(name: &str) -> bool {
    Command::new(name)
        .arg("-version")
        .output()
        .or_else(|_| Command::new(name).arg("-v").output())
        .is_ok()
}

fn preview(service: &PreviewService, results: &Receiver<PreviewResult>, path: &Path) -> PreviewContent {
    let id = service.request(path.to_path_buf());
    let result = results.recv_timeout(WAIT).expect("a result");
    assert_eq!(result.request, id);
    assert_eq!(result.path, path);
    result.content
}

#[cfg(feature = "image")]
fn png(w: u32, h: u32) -> Vec<u8> {
    let img = RgbImage::from_fn(w, h, |x, _| Rgb([(x % 256) as u8, 40, 200]));
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png).expect("encode png");
    out.into_inner()
}

#[cfg(feature = "image")]
#[test]
fn images_are_decoded_and_downscaled() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (service, results) = PreviewService::start(PreviewLimits::default());

    let img = tmp.path().join("photo.png");
    fs::write(&img, png(2000, 1000)).expect("write");
    let PreviewContent::Image { image, original_size } = preview(&service, &results, &img) else {
        panic!("expected an image");
    };
    assert_eq!((image.width, image.height), (768, 384));
    assert_eq!(original_size, Some((2000, 1000)));

    let jpg = tmp.path().join("photo.jpeg");
    let rgb = RgbImage::from_pixel(64, 48, Rgb([10, 200, 30]));
    rgb.save_with_format(&jpg, ImageFormat::Jpeg).expect("save jpeg");
    assert!(matches!(
        preview(&service, &results, &jpg),
        PreviewContent::Image { .. }
    ));
}

#[test]
fn text_folder_and_binary() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (service, results) = PreviewService::start(PreviewLimits::default());

    let txt = tmp.path().join("notes.md");
    fs::write(&txt, "# Title\nbody\n").expect("write");
    assert_eq!(
        preview(&service, &results, &txt),
        PreviewContent::Text {
            text: "# Title\nbody\n".to_owned(),
            truncated: false
        }
    );

    let big = tmp.path().join("big.log");
    fs::write(&big, vec![b'x'; 200_000]).expect("write");
    let PreviewContent::Text { text, truncated } = preview(&service, &results, &big) else {
        panic!("expected text");
    };
    assert!(truncated);
    assert_eq!(text.len(), 64 * 1024);

    let blob = tmp.path().join("data.bin");
    fs::write(&blob, [0u8, 159, 146, 150, 0, 1, 2]).expect("write");
    assert!(matches!(
        preview(&service, &results, &blob),
        PreviewContent::Unavailable { .. }
    ));

    assert_eq!(
        preview(&service, &results, tmp.path()),
        PreviewContent::Folder { children: Some(3) }
    );
}

#[cfg(feature = "image")]
#[test]
fn set_limits_applies_to_later_requests() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let img = tmp.path().join("p.png");
    fs::write(&img, png(400, 400)).expect("write");
    let (service, results) = PreviewService::start(PreviewLimits::default());
    let PreviewContent::Image { image, .. } = preview(&service, &results, &img) else {
        panic!("expected an image");
    };
    assert_eq!(image.width, 400);

    service.set_limits(PreviewLimits {
        max_dim: 100,
        ..PreviewLimits::default()
    });
    let PreviewContent::Image { image, .. } = preview(&service, &results, &img) else {
        panic!("expected an image");
    };
    assert_eq!(
        image.width, 100,
        "same file, new limits: re-rendered, not served stale"
    );

    service.set_limits(PreviewLimits {
        max_file_size: 10,
        ..PreviewLimits::default()
    });
    assert!(matches!(
        preview(&service, &results, &img),
        PreviewContent::Unavailable { hint } if hint.contains("larger than the preview limit")
    ));
}

#[cfg(feature = "audio")]
#[test]
fn wav_metadata() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut wav = Vec::new();
    let (rate, secs) = (8000u32, 3u32);
    let data = rate * secs * 2;
    wav.extend(b"RIFF");
    wav.extend((36 + data).to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend([1, 0, 1, 0]); // PCM, mono
    wav.extend(rate.to_le_bytes());
    wav.extend((rate * 2).to_le_bytes());
    wav.extend([2, 0, 16, 0]);
    wav.extend(b"data");
    wav.extend(data.to_le_bytes());
    wav.resize(44 + data as usize, 0);
    let path = tmp.path().join("silence.wav");
    fs::write(&path, wav).expect("write");

    let (service, results) = PreviewService::start(PreviewLimits::default());
    let PreviewContent::Audio { meta, cover } = preview(&service, &results, &path) else {
        panic!("expected audio");
    };
    assert_eq!(meta.duration, Some(Duration::from_secs(3)));
    assert!(cover.is_none());
    assert!(
        meta.extra
            .contains(&("Sample rate".to_owned(), "8000 Hz".to_owned()))
    );
}

#[cfg(feature = "pdf")]
const PDF: &str = "%PDF-1.4
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 100]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj
4 0 obj<</Length 36>>stream
BT /F1 24 Tf 20 40 Td (Hello) Tj ET
endstream
endobj
5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj
trailer<</Root 1 0 R/Size 6>>
";

#[cfg(feature = "pdf")]
#[test]
fn pdf_first_page() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("doc.pdf");
    fs::write(&path, PDF).expect("write");
    let (service, results) = PreviewService::start(PreviewLimits::default());
    let content = preview(&service, &results, &path);
    if !tool_available("pdftoppm") {
        let PreviewContent::Unavailable { hint } = content else {
            panic!("expected a hint, got {content:?}");
        };
        assert_eq!(hint, "Install poppler-utils for PDF previews");
        return;
    }
    let PreviewContent::Pdf { page, pages } = content else {
        panic!("expected a PDF page, got {content:?}");
    };
    assert_eq!(page.width.max(page.height), 768);
    assert_eq!(pages, Some(1));
}

#[cfg(feature = "video")]
#[test]
fn video_metadata_and_thumbnail() {
    if !tool_available("ffmpeg") {
        eprintln!("skipped: ffmpeg not installed");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("clip.mkv");
    let status = Command::new("ffmpeg")
        .args([
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=320x240:rate=10",
        ])
        .arg(&path)
        .status()
        .expect("run ffmpeg");
    assert!(status.success());

    let (service, results) = PreviewService::start(PreviewLimits::default());
    let PreviewContent::Video { meta, thumbnail } = preview(&service, &results, &path) else {
        panic!("expected video");
    };
    assert!(meta.duration.is_some_and(|d| d >= Duration::from_millis(900)));
    assert!(
        meta.extra
            .contains(&("Resolution".to_owned(), "320×240".to_owned()))
    );
    let thumb = thumbnail.expect("thumbnail frame");
    assert_eq!((thumb.width, thumb.height), (320, 240));
}

#[test]
fn only_the_latest_of_many_rapid_requests_is_answered() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (service, results) = PreviewService::start(PreviewLimits::default());
    let mut last = 0;
    for i in 0..20 {
        let path = tmp.path().join(format!("f{i}.txt"));
        fs::write(&path, format!("file {i}")).expect("write");
        last = service.request(path);
    }
    // Results may exist for earlier requests that finished before they were superseded, but the
    // newest request always answers, and nothing is ever delivered after it.
    let mut seen = Vec::new();
    loop {
        let result = results.recv_timeout(WAIT).expect("the newest request answers");
        seen.push(result.request);
        if result.request == last {
            break;
        }
    }
    assert!(
        seen.windows(2).all(|w| w[0] < w[1]),
        "ids arrive in order: {seen:?}"
    );
    drop(service);
    assert!(results.recv().is_err(), "nothing after the newest result");
}
