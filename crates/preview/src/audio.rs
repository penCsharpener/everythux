//! Audio tags, properties and cover art via `lofty`. Only headers and tags are read, so this is
//! cheap even for huge files.

use std::path::Path;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::picture::{Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::{Accessor, Tag};

use crate::cancel::Cancel;
use crate::error::PreviewError;
use crate::{MediaMeta, PreviewContent, images};

/// Reads tags and properties; decodes the best embedded picture, downscaled to `max_dim`.
pub(crate) fn read(path: &Path, max_dim: u32, cancel: &Cancel) -> Result<PreviewContent, PreviewError> {
    cancel.check()?;
    let tagged = Probe::open(path)
        .and_then(|probe| {
            // Content-based detection, so a wrong extension does not matter.
            probe
                .options(ParseOptions::new().read_cover_art(true))
                .guess_file_type()
                .map_err(Into::into)
        })
        .and_then(Probe::read)
        .map_err(|e| PreviewError::Decode(e.to_string()))?;
    cancel.check()?;

    let meta = meta_of(&tagged);
    let cover = cover_of(&tagged).and_then(|picture| {
        images::load_bytes(picture.data(), max_dim, cancel)
            .map(|(image, _)| image)
            .inspect_err(|e| tracing::debug!(path = %path.display(), error = %e, "cover art not decodable"))
            .ok()
    });
    cancel.check()?;
    Ok(PreviewContent::Audio { meta, cover })
}

fn meta_of(tagged: &TaggedFile) -> MediaMeta {
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let non_empty =
        |s: Option<std::borrow::Cow<'_, str>>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    let props = tagged.properties();

    let mut extra = vec![("Format".to_owned(), format!("{:?}", tagged.file_type()))];
    if let Some(kbps) = props.audio_bitrate().or_else(|| props.overall_bitrate()) {
        extra.push(("Bitrate".to_owned(), format!("{kbps} kbps")));
    }
    if let Some(hz) = props.sample_rate() {
        extra.push(("Sample rate".to_owned(), format!("{hz} Hz")));
    }
    if let Some(ch) = props.channels() {
        extra.push(("Channels".to_owned(), ch.to_string()));
    }
    if let Some(bits) = props.bit_depth() {
        extra.push(("Bit depth".to_owned(), format!("{bits}-bit")));
    }
    let duration = props.duration();
    MediaMeta {
        title: tag.and_then(|t| non_empty(t.title())),
        artist: tag.and_then(|t| non_empty(t.artist())),
        album: tag.and_then(|t| non_empty(t.album())),
        duration: (!duration.is_zero()).then_some(duration),
        extra,
    }
}

/// Front cover if present, otherwise the first picture of any tag.
fn cover_of(tagged: &TaggedFile) -> Option<&Picture> {
    let pictures = || tagged.tags().iter().flat_map(Tag::pictures);
    pictures()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pictures().next())
}

#[cfg(test)]
pub(crate) mod testutil {
    /// 16-bit mono PCM WAV: 44-byte header followed by silence.
    pub(crate) fn wav(sample_rate: u32, seconds: u32) -> Vec<u8> {
        let channels: u16 = 1;
        let bits: u16 = 16;
        let samples = sample_rate * seconds;
        let data_len = samples * u32::from(channels) * u32::from(bits / 8);
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend(b"RIFF");
        out.extend((36 + data_len).to_le_bytes());
        out.extend(b"WAVEfmt ");
        out.extend(16u32.to_le_bytes());
        out.extend(1u16.to_le_bytes()); // PCM
        out.extend(channels.to_le_bytes());
        out.extend(sample_rate.to_le_bytes());
        out.extend((sample_rate * u32::from(channels) * u32::from(bits / 8)).to_le_bytes());
        out.extend((channels * bits / 8).to_le_bytes());
        out.extend(bits.to_le_bytes());
        out.extend(b"data");
        out.extend(data_len.to_le_bytes());
        out.resize(44 + data_len as usize, 0);
        out
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::*;

    #[test]
    fn wav_properties_are_reported() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("tone.wav");
        fs::write(&file, testutil::wav(8000, 2)).expect("write");
        let PreviewContent::Audio { meta, cover } = read(&file, 256, &Cancel::never()).expect("read") else {
            panic!("expected audio");
        };
        assert!(cover.is_none());
        assert_eq!(meta.duration, Some(Duration::from_secs(2)));
        assert!(meta.title.is_none());
        let get = |key: &str| meta.extra.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
        assert_eq!(get("Sample rate"), Some("8000 Hz"));
        assert_eq!(get("Channels"), Some("1"));
        assert_eq!(get("Bit depth"), Some("16-bit"));
        assert_eq!(get("Bitrate"), Some("128 kbps"));
        assert_eq!(get("Format"), Some("Wav"));
    }

    #[test]
    fn tags_and_cover_art_are_read() {
        use lofty::config::WriteOptions;
        use lofty::picture::MimeType;
        use lofty::tag::TagType;

        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("song.wav");
        fs::write(&file, testutil::wav(8000, 1)).expect("write");

        let mut tagged = lofty::read_from_path(&file).expect("read wav");
        let mut tag = Tag::new(TagType::Id3v2);
        tag.set_title("Title ".to_owned());
        tag.set_artist("Artist".to_owned());
        tag.set_album("Album".to_owned());
        let cover = images::testutil::encode(&images::testutil::two_tone(64, 32), image::ImageFormat::Png);
        tag.push_picture(
            Picture::unchecked(cover)
                .pic_type(PictureType::CoverFront)
                .mime_type(MimeType::Png)
                .build(),
        );
        tagged.insert_tag(tag);
        tagged
            .save_to_path(&file, WriteOptions::default())
            .expect("save tags");

        let PreviewContent::Audio { meta, cover } = read(&file, 16, &Cancel::never()).expect("read") else {
            panic!("expected audio");
        };
        assert_eq!(meta.title.as_deref(), Some("Title"), "trimmed");
        assert_eq!(meta.artist.as_deref(), Some("Artist"));
        assert_eq!(meta.album.as_deref(), Some("Album"));
        assert_eq!(meta.duration, Some(Duration::from_secs(1)));
        let cover = cover.expect("embedded cover");
        assert_eq!((cover.width, cover.height), (16, 8), "downscaled to max_dim");
    }

    #[test]
    fn wrong_extension_is_detected_by_content() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("tone.mp3");
        fs::write(&file, testutil::wav(8000, 1)).expect("write");
        let PreviewContent::Audio { meta, .. } = read(&file, 256, &Cancel::never()).expect("read") else {
            panic!("expected audio");
        };
        assert_eq!(meta.duration, Some(Duration::from_secs(1)));
    }

    #[test]
    fn garbage_is_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("noise.flac");
        fs::write(&file, vec![0x55u8; 4096]).expect("write");
        assert!(read(&file, 256, &Cancel::never()).is_err());
    }
}
