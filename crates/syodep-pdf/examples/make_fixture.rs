//! Writes a small multi-page test PDF, used by CI smoke tests:
//!
//! cargo run -p syodep-pdf --features test-support --example make_fixture -- out.pdf [pages]

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: make_fixture <out.pdf> [pages]");
    let pages: usize = args.next().map(|p| p.parse().expect("pages")).unwrap_or(3);
    let texts: Vec<String> = (1..=pages)
        .map(|i| format!("syodep fixture page {i} of {pages}"))
        .collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    std::fs::write(&path, syodep_pdf::test_support::pdf_with_pages(&refs)).expect("write pdf");
    println!("wrote {path} ({pages} pages)");
}
