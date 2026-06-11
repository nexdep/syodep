//! Programmatic PDF fixtures for tests.
//!
//! Generating fixtures in code (instead of checking in binary PDFs) keeps
//! the repository clean and makes the fixtures self-describing. The builder
//! emits a minimal but spec-conforming PDF 1.4 file with one text line per
//! page, computing the cross-reference table offsets exactly.

/// Build a PDF with one A4 page (595x842 pt) per entry of `page_texts`,
/// each showing its text in Helvetica at the top of the page.
pub fn pdf_with_pages(page_texts: &[&str]) -> Vec<u8> {
    // Object numbering: 1 = catalog, 2 = pages root, 3 = font,
    // then per page i (0-based): 4 + 2i = page, 5 + 2i = its content stream.
    let n_pages = page_texts.len();
    let total_objects = 3 + 2 * n_pages;

    let mut buf: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let mut offsets: Vec<usize> = vec![0; total_objects + 1];

    let write_obj = |buf: &mut Vec<u8>, offsets: &mut Vec<usize>, num: usize, body: &[u8]| {
        offsets[num] = buf.len();
        buf.extend_from_slice(format!("{num} 0 obj\n").as_bytes());
        buf.extend_from_slice(body);
        buf.extend_from_slice(b"\nendobj\n");
    };

    write_obj(
        &mut buf,
        &mut offsets,
        1,
        b"<< /Type /Catalog /Pages 2 0 R >>",
    );

    let kids: Vec<String> = (0..n_pages).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    write_obj(
        &mut buf,
        &mut offsets,
        2,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {n_pages} >>",
            kids.join(" ")
        )
        .as_bytes(),
    );

    write_obj(
        &mut buf,
        &mut offsets,
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );

    for (i, text) in page_texts.iter().enumerate() {
        let page_num = 4 + 2 * i;
        let content_num = 5 + 2 * i;
        write_obj(
            &mut buf,
            &mut offsets,
            page_num,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
                 /Resources << /Font << /F1 3 0 R >> >> /Contents {content_num} 0 R >>"
            )
            .as_bytes(),
        );
        let escaped = text
            .replace('\\', "\\\\")
            .replace('(', "\\(")
            .replace(')', "\\)");
        let stream = format!("BT /F1 24 Tf 72 750 Td ({escaped}) Tj ET");
        write_obj(
            &mut buf,
            &mut offsets,
            content_num,
            format!(
                "<< /Length {} >>\nstream\n{stream}\nendstream",
                stream.len()
            )
            .as_bytes(),
        );
    }

    let xref_offset = buf.len();
    buf.extend_from_slice(format!("xref\n0 {}\n", total_objects + 1).as_bytes());
    buf.extend_from_slice(b"0000000000 65535 f \n");
    for offset in &offsets[1..] {
        buf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    buf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_offset}\n%%EOF\n",
            total_objects + 1
        )
        .as_bytes(),
    );
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_a_parseable_header_and_trailer() {
        let bytes = pdf_with_pages(&["one", "two"]);
        assert!(bytes.starts_with(b"%PDF-1.4"));
        assert!(bytes.ends_with(b"%%EOF\n"));
    }

    #[test]
    fn escapes_parentheses_and_backslashes() {
        let bytes = pdf_with_pages(&["a (tricky) \\ string"]);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("a \\(tricky\\) \\\\ string"));
    }
}
