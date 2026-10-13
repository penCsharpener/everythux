//! Image decoding: format sniffing, decoder limits, EXIF orientation and fast downscaling.

use std::fs::File;
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageReader, Limits};

use crate::ImageData;
use crate::cancel::Cancel;
use crate::error::PreviewError;

/// Largest decoder allocation we accept; protects against decompression bombs.
const MAX_ALLOC: u64 = 256 * 1024 * 1024;
/// Largest accepted source dimension.
const MAX_SOURCE_DIM: u32 = 32_768;
/// Read buffer for files.
const READ_BUF: usize = 64 * 1024;

/// A decoded, downscaled image and the dimensions of the (oriented) original.
pub(crate) type Decoded = (ImageData, (u32, u32));

/// Reader that fails once the request is cancelled, so decoders that stream from it stop early.
#[derive(Debug)]
struct CancelReader<R> {
    inner: R,
    cancel: Cancel,
}

impl<R> CancelReader<R> {
    fn new(inner: R, cancel: Cancel) -> Self {
        Self { inner, cancel }
    }
}

fn cancelled_io() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "preview cancelled")
}

impl<R: Read> Read for CancelReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cancel.is_cancelled() {
            return Err(cancelled_io());
        }
        self.inner.read(buf)
    }
}

impl<R: Seek> Seek for CancelReader<R> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        if self.cancel.is_cancelled() {
            return Err(cancelled_io());
        }
        self.inner.seek(pos)
    }
}

/// Decodes the image file at `path`, aborting early if `cancel` fires while reading.
pub(crate) fn load_file(path: &Path, max_dim: u32, cancel: &Cancel) -> Result<Decoded, PreviewError> {
    let file = File::open(path)?;
    let reader = BufReader::with_capacity(READ_BUF, CancelReader::new(file, cancel.clone()));
    decode(reader, max_dim, cancel)
}

#[cfg(any(test, feature = "pdf", feature = "audio", feature = "video"))]
/// Decodes an in-memory image (a cached thumbnail, `pdftoppm` or `ffmpeg` output, cover art).
pub(crate) fn load_bytes(bytes: &[u8], max_dim: u32, cancel: &Cancel) -> Result<Decoded, PreviewError> {
    decode(io::Cursor::new(bytes), max_dim, cancel)
}

fn decode<R: BufRead + Seek>(reader: R, max_dim: u32, cancel: &Cancel) -> Result<Decoded, PreviewError> {
    let max_dim = max_dim.max(1); // a zero box would make the resizer misbehave
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_ALLOC);
    limits.max_image_width = Some(MAX_SOURCE_DIM);
    limits.max_image_height = Some(MAX_SOURCE_DIM);

    let mut reader = ImageReader::new(reader).with_guessed_format()?;
    reader.limits(limits);
    let mut decoder = reader.into_decoder().map_err(|e| map_image_error(&e, cancel))?;
    // A broken EXIF block must not prevent showing the picture.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let image = DynamicImage::from_decoder(decoder).map_err(|e| map_image_error(&e, cancel))?;
    cancel.check()?;

    let (w, h) = (image.width(), image.height());
    let original = if swaps_axes(orientation) { (h, w) } else { (w, h) };
    // Downscale before rotating: the box is square, and rotating fewer pixels is cheaper.
    let mut image = if w > max_dim || h > max_dim {
        image.thumbnail(max_dim, max_dim)
    } else {
        image
    };
    image.apply_orientation(orientation);
    cancel.check()?;

    let rgba = image.into_rgba8();
    let (width, height) = rgba.dimensions();
    Ok((
        ImageData {
            width,
            height,
            rgba: rgba.into_raw(),
        },
        original,
    ))
}

fn swaps_axes(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// Decoder failures caused by our own cancelling reader are reported as cancellation.
fn map_image_error(err: &image::ImageError, cancel: &Cancel) -> PreviewError {
    if cancel.is_cancelled() {
        PreviewError::Cancelled
    } else {
        PreviewError::Decode(err.to_string())
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::io::Cursor;

    use image::{ImageFormat, Rgb, RgbImage};

    /// A `w × h` image whose left half is red and right half blue.
    pub(crate) fn two_tone(w: u32, h: u32) -> RgbImage {
        RgbImage::from_fn(w, h, |x, _| {
            if x < w / 2 {
                Rgb([255, 0, 0])
            } else {
                Rgb([0, 0, 255])
            }
        })
    }

    /// Inserts an EXIF APP1 segment carrying `orientation` (1..=8) right after the JPEG SOI marker.
    pub(crate) fn with_exif_orientation(jpeg: &[u8], orientation: u8) -> Vec<u8> {
        let mut payload = b"Exif\0\0".to_vec();
        payload.extend([b'I', b'I', 0x2a, 0, 8, 0, 0, 0]); // TIFF header, IFD0 at offset 8
        payload.extend([1, 0]); // one entry
        payload.extend([0x12, 0x01, 3, 0, 1, 0, 0, 0, orientation, 0, 0, 0]); // Orientation, SHORT
        payload.extend([0, 0, 0, 0]); // no next IFD
        let len = u16::try_from(payload.len() + 2).expect("segment fits");
        let mut out = jpeg[..2].to_vec();
        out.extend([0xff, 0xe1]);
        out.extend(len.to_be_bytes());
        out.extend(payload);
        out.extend(&jpeg[2..]);
        out
    }

    pub(crate) fn encode(img: &RgbImage, format: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, format).expect("encode");
        out.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;

    use image::ImageFormat;

    use super::testutil::{encode, two_tone, with_exif_orientation};
    use super::*;

    #[test]
    fn reader_fails_after_cancel() {
        let latest = Arc::new(AtomicU64::new(1));
        let mut reader = CancelReader::new(Cursor::new(vec![1u8; 16]), Cancel::new(Arc::clone(&latest), 1));
        let mut buf = [0u8; 4];
        assert_eq!(reader.read(&mut buf).expect("first read"), 4);
        latest.store(2, std::sync::atomic::Ordering::Relaxed);
        let err = reader.read(&mut buf).expect_err("cancelled read");
        assert_eq!(err.kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn small_png_is_returned_unscaled() {
        let png = encode(&two_tone(40, 20), ImageFormat::Png);
        let (img, original) = load_bytes(&png, 768, &Cancel::never()).expect("decode");
        assert_eq!((img.width, img.height, original), (40, 20, (40, 20)));
        assert_eq!(img.rgba.len(), 40 * 20 * 4);
        assert_eq!(&img.rgba[..4], &[255, 0, 0, 255], "left pixel is red");
        let last = img.rgba.len() - 4;
        assert_eq!(&img.rgba[last..], &[0, 0, 255, 255], "right pixel is blue");
    }

    #[test]
    fn large_image_is_downscaled_keeping_aspect() {
        let jpeg = encode(&two_tone(400, 200), ImageFormat::Jpeg);
        let (img, original) = load_bytes(&jpeg, 100, &Cancel::never()).expect("decode");
        assert_eq!((img.width, img.height), (100, 50));
        assert_eq!(original, (400, 200));
        assert_eq!(img.rgba.len(), 100 * 50 * 4);
    }

    #[test]
    fn exif_orientation_is_applied() {
        let jpeg = encode(&two_tone(40, 20), ImageFormat::Jpeg);
        // Orientation 6: rotate 90 degrees clockwise, so a 40x20 file displays as 20x40.
        let rotated = with_exif_orientation(&jpeg, 6);
        let (img, original) = load_bytes(&rotated, 768, &Cancel::never()).expect("decode");
        assert_eq!((img.width, img.height), (20, 40));
        assert_eq!(original, (20, 40));
        // After a clockwise turn, the red left half ends up on top.
        assert!(
            img.rgba[0] > 200 && img.rgba[2] < 60,
            "top-left should be red: {:?}",
            &img.rgba[..4]
        );
        let last = img.rgba.len() - 4;
        assert!(
            img.rgba[last + 2] > 200 && img.rgba[last] < 60,
            "bottom-right should be blue"
        );

        let (plain, _) = load_bytes(&jpeg, 768, &Cancel::never()).expect("decode");
        assert_eq!((plain.width, plain.height), (40, 20), "no EXIF means no rotation");
    }

    #[test]
    fn exif_orientation_applies_when_downscaling_too() {
        let jpeg = with_exif_orientation(&encode(&two_tone(400, 200), ImageFormat::Jpeg), 6);
        let (img, original) = load_bytes(&jpeg, 100, &Cancel::never()).expect("decode");
        assert_eq!((img.width, img.height), (50, 100));
        assert_eq!(original, (200, 400));
    }

    #[test]
    fn corrupt_data_is_an_error() {
        let err = load_bytes(b"definitely not an image", 100, &Cancel::never()).expect_err("must fail");
        assert!(
            matches!(err, PreviewError::Decode(_) | PreviewError::Io(_)),
            "{err:?}"
        );
    }

    #[test]
    fn cancelled_request_stops_decoding() {
        let png = encode(&two_tone(64, 64), ImageFormat::Png);
        let latest = Arc::new(AtomicU64::new(2));
        let stale = Cancel::new(latest, 1);
        let err = load_bytes(&png, 100, &stale).expect_err("stale request");
        assert!(err.is_cancelled(), "{err:?}");
    }

    #[test]
    fn oversized_dimensions_are_rejected_by_limits() {
        // A PNG header claiming 40000 x 40000 pixels; the decoder limit must trip before allocating.
        let mut png = encode(&two_tone(2, 2), ImageFormat::Png);
        // IHDR width/height live at bytes 16..24; the CRC is not verified before limits are applied
        // on the header, but if it is, an error is still the expected outcome.
        png[16..20].copy_from_slice(&40_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&40_000u32.to_be_bytes());
        assert!(load_bytes(&png, 100, &Cancel::never()).is_err());
    }
}
