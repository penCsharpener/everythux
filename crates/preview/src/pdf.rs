//! PDF first page via poppler's `pdftoppm` (page count from `pdfinfo`).

use std::ffi::OsString;
use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::cancel::Cancel;
use crate::error::PreviewError;
use crate::tools::{Tools, run_capture};
use crate::{PreviewContent, images};

const HINT: &str = "Install poppler-utils for PDF previews";
const TIMEOUT: Duration = Duration::from_secs(5);

/// Renders page 1 of `path` to fit `max_dim × max_dim`.
pub(crate) fn render(
    path: &Path,
    max_dim: u32,
    tools: &Tools,
    cancel: &Cancel,
) -> Result<PreviewContent, PreviewError> {
    let Some(pdftoppm) = tools.locate("pdftoppm") else {
        return Err(PreviewError::ToolMissing {
            tool: "pdftoppm",
            hint: HINT,
        });
    };
    let pdfinfo = tools.locate("pdfinfo");
    let abs = std::path::absolute(path)?;

    // Without an output prefix, pdftoppm writes the single requested page to stdout.
    let render_args = [
        OsString::from("-png"),
        OsString::from("-singlefile"),
        OsString::from("-f"),
        OsString::from("1"),
        OsString::from("-l"),
        OsString::from("1"),
        OsString::from("-scale-to"),
        OsString::from(max_dim.to_string()),
        abs.clone().into_os_string(),
    ];
    let info_args = [abs.into_os_string()];

    // `pdfinfo` is only a nicety; run it next to the render so it adds no latency.
    let (png, info) = thread::scope(|scope| {
        let info = pdfinfo
            .as_deref()
            .map(|bin| scope.spawn(|| run_capture("pdfinfo", bin, &info_args, TIMEOUT, cancel)));
        let png = run_capture("pdftoppm", &pdftoppm, &render_args, TIMEOUT, cancel);
        let info = info.and_then(|handle| handle.join().ok()).and_then(Result::ok);
        (png, info)
    });
    let png = png?;
    cancel.check()?;
    if png.is_empty() {
        return Err(PreviewError::Decode("pdftoppm produced no image".to_owned()));
    }
    let (page, _) = images::load_bytes(&png, max_dim, cancel)?;
    Ok(PreviewContent::Pdf {
        page,
        pages: info.as_deref().and_then(parse_page_count),
    })
}

/// The `Pages:` line of `pdfinfo` output.
pub(crate) fn parse_page_count(output: &[u8]) -> Option<u32> {
    String::from_utf8_lossy(output)
        .lines()
        .find_map(|line| line.strip_prefix("Pages:"))
        .and_then(|rest| rest.trim().parse().ok())
}

#[cfg(test)]
pub(crate) mod testutil {
    /// A one-page PDF (200 × 100 pt) saying "Hello". `pdftoppm` repairs the missing xref table.
    pub(crate) const MINIMAL_PDF: &str = "%PDF-1.4
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pdfinfo_pages_line() {
        let out = b"Title:  x\nPages:           12\nEncrypted:       no\n";
        assert_eq!(parse_page_count(out), Some(12));
        assert_eq!(parse_page_count(b"Title: x\n"), None);
        assert_eq!(parse_page_count(b"Pages: many\n"), None);
    }

    #[test]
    fn missing_pdftoppm_reports_install_hint() {
        let tools = Tools::with_path("/nonexistent-dir");
        let err = render(Path::new("/x.pdf"), 100, &tools, &Cancel::never()).expect_err("no tool");
        assert_eq!(err.hint(), HINT);
    }

    #[test]
    fn renders_first_page_and_counts_pages() {
        let tools = Tools::from_env();
        if tools.locate("pdftoppm").is_none() {
            eprintln!("skipped: pdftoppm not installed");
            return;
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("hello.pdf");
        std::fs::write(&file, testutil::MINIMAL_PDF).expect("write");
        let PreviewContent::Pdf { page, pages } =
            render(&file, 300, &tools, &Cancel::never()).expect("render")
        else {
            panic!("expected a PDF preview");
        };
        assert_eq!(page.width.max(page.height), 300, "fits the box");
        assert!(page.width > page.height, "landscape page");
        assert_eq!(page.rgba.len(), (page.width * page.height * 4) as usize);
        if tools.locate("pdfinfo").is_some() {
            assert_eq!(pages, Some(1));
        }
        assert!(
            page.rgba.chunks_exact(4).any(|px| px[0] < 128),
            "the text must leave dark pixels on the white page"
        );
    }

    #[test]
    fn garbage_pdf_is_an_error_not_a_panic() {
        let tools = Tools::from_env();
        if tools.locate("pdftoppm").is_none() {
            return;
        }
        let tmp = tempfile::tempdir().expect("tempdir");
        let file = tmp.path().join("bad.pdf");
        std::fs::write(&file, b"not a pdf").expect("write");
        assert!(render(&file, 100, &tools, &Cancel::never()).is_err());
    }
}
