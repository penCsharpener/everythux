//! Text preview latency through the public service (ROADMAP §9 Phase 8: a preview within
//! 150 ms): the first 64 KiB of a 5 MB log in each encoding, and one 256 KiB "Show more" chunk.

use std::fmt::Write as _;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use crossbeam_channel::Receiver;
use everythux_preview::{
    PreviewContent, PreviewLimits, PreviewResult, PreviewService, TEXT_MORE_BYTES, TextEncoding,
};

const WAIT: Duration = Duration::from_secs(30);

fn log_text(bytes: usize) -> String {
    let mut text = String::with_capacity(bytes + 100);
    let mut i = 0u32;
    while text.len() < bytes {
        writeln!(
            text,
            "[{i:08}] worker-{} finished job “{}” in {} ms — ok",
            i % 16,
            i % 97,
            i % 997
        )
        .expect("writing to a String cannot fail");
        i += 1;
    }
    text
}

fn fixtures(dir: &Path) -> Vec<(&'static str, PathBuf)> {
    let text = log_text(5 * 1024 * 1024);
    let utf8 = dir.join("big.log");
    std::fs::write(&utf8, &text).expect("write");
    let utf16 = dir.join("big-utf16.txt");
    let mut bytes = vec![0xFF, 0xFE];
    for unit in text.encode_utf16() {
        bytes.extend(unit.to_le_bytes());
    }
    std::fs::write(&utf16, bytes).expect("write");
    let latin = dir.join("big-1252.txt");
    let bytes: Vec<u8> = text
        .chars()
        .map(|c| match c {
            '“' => 0x93,
            '”' => 0x94,
            '—' => 0x97,
            c => c as u8,
        })
        .collect();
    std::fs::write(&latin, bytes).expect("write");
    vec![("utf8", utf8), ("utf16", utf16), ("windows1252", latin)]
}

fn answer(results: &Receiver<PreviewResult>, id: u64) -> PreviewContent {
    loop {
        let result = results.recv_timeout(WAIT).expect("result");
        if result.request == id {
            return result.content;
        }
    }
}

fn text(c: &mut Criterion) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let files = fixtures(tmp.path());
    // The service caches finished previews by path; a fresh limit per iteration defeats that so
    // every iteration reads and decodes.
    let (service, results) = PreviewService::start(PreviewLimits::default());
    for (name, path) in &files {
        let mut max_file_size = u64::MAX / 2;
        c.bench_function(&format!("preview/text_head_5mb_{name}"), |b| {
            b.iter(|| {
                max_file_size -= 1;
                service.set_limits(PreviewLimits {
                    max_file_size,
                    ..PreviewLimits::default()
                });
                let content = answer(&results, service.request(path.clone()));
                assert!(matches!(content, PreviewContent::Text { .. }), "{content:?}");
                black_box(content)
            });
        });
    }
    let (_, utf16) = &files[1];
    c.bench_function("preview/text_range_256k_utf16", |b| {
        b.iter(|| {
            let id =
                service.request_range(utf16.clone(), 1024 * 1024, TEXT_MORE_BYTES, TextEncoding::Utf16Le);
            black_box(answer(&results, id))
        });
    });
}

criterion_group!(benches, text);
criterion_main!(benches);
