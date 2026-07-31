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

/// Build a single A4 page whose text is laid out in two columns: a left band
/// near x=72 and a right band near x=340, each with `rows` lines stacked top to
/// bottom. Used to exercise multi-column detection (line focus `H`/`L`).
pub fn pdf_two_column_page(rows: usize) -> Vec<u8> {
    let mut runs = String::new();
    for (col, x) in [72.0_f32, 340.0_f32].into_iter().enumerate() {
        for r in 0..rows {
            let y = 750.0 - r as f32 * 40.0;
            let text = format!("C{}L{}.", col + 1, r + 1);
            runs.push_str(&format!("BT /F1 18 Tf {x} {y} Td ({text}) Tj ET\n"));
        }
    }

    let mut buf: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let total_objects = 5; // catalog, pages, font, page, content
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
    write_obj(
        &mut buf,
        &mut offsets,
        2,
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        4,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
          /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        5,
        format!("<< /Length {} >>\nstream\n{runs}\nendstream", runs.len()).as_bytes(),
    );

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

/// Build a single A4 page containing a caption line plus one raster image
/// (a 2x2 DeviceRGB image drawn as a ~120x90 pt box near the middle of the
/// page). Used to exercise the image branch of `Document::page_content`.
pub fn pdf_with_image() -> Vec<u8> {
    // 1 = catalog, 2 = pages, 3 = font, 4 = page, 5 = content, 6 = image.
    let total_objects = 6;
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
    write_obj(
        &mut buf,
        &mut offsets,
        2,
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        4,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
          /Resources << /Font << /F1 3 0 R >> /XObject << /Im0 6 0 R >> >> \
          /Contents 5 0 R >>",
    );
    // Caption text, then draw the image scaled to 120x90 at (100, 400).
    let stream = "BT /F1 24 Tf 72 750 Td (Caption) Tj ET\nq 120 0 0 90 100 400 cm /Im0 Do Q";
    write_obj(
        &mut buf,
        &mut offsets,
        5,
        format!(
            "<< /Length {} >>\nstream\n{stream}\nendstream",
            stream.len()
        )
        .as_bytes(),
    );
    // 2x2 DeviceRGB image => 4 pixels * 3 bytes = 12 bytes of sample data.
    let pixels: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 0];
    let mut img: Vec<u8> = format!(
        "<< /Type /XObject /Subtype /Image /Width 2 /Height 2 /ColorSpace /DeviceRGB \
         /BitsPerComponent 8 /Length {} >>\nstream\n",
        pixels.len()
    )
    .into_bytes();
    img.extend_from_slice(&pixels);
    img.extend_from_slice(b"\nendstream");
    write_obj(&mut buf, &mut offsets, 6, &img);

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

/// Build a single A4 page with a heading, a `cols` x `rows` ruled table, and a
/// caption 40pt below it. Used to exercise table detection.
pub fn pdf_with_table(cols: usize, rows: usize) -> Vec<u8> {
    pdf_with_table_gap(cols, rows, 40.0)
}

/// [`pdf_with_table`] with the caption set `caption_gap` points under the last
/// rule.
///
/// A small gap is the interesting case: MuPDF's table box is the ruled region
/// and reaches past the last row, so a caption set tight under the grid is what
/// tempts detection into swallowing it.
///
/// The rules are drawn as stroked rectangles because MuPDF's table hunt looks
/// for ruled regions among a page's vector rectangles; a table of text alone,
/// with no rules, is far less reliably recognised.
pub fn pdf_with_table_gap(cols: usize, rows: usize, caption_gap: f32) -> Vec<u8> {
    const LEFT: f32 = 100.0;
    const TOP: f32 = 700.0;
    const COL_W: f32 = 90.0;
    const ROW_H: f32 = 28.0;

    let table_w = COL_W * cols as f32;
    let table_h = ROW_H * rows as f32;
    let mut content = String::new();

    content.push_str("BT /F1 16 Tf 100 760 Td (Results overview) Tj ET\n");

    // Ruling lines: one stroked rectangle per row and per column, so the grid
    // is unambiguous both horizontally and vertically.
    content.push_str("0.7 w\n");
    for r in 0..rows {
        let y = TOP - ROW_H * (r + 1) as f32;
        content.push_str(&format!("{LEFT} {y} {table_w} {ROW_H} re S\n"));
    }
    for c in 0..cols {
        let x = LEFT + COL_W * c as f32;
        let y = TOP - table_h;
        content.push_str(&format!("{x} {y} {COL_W} {table_h} re S\n"));
    }

    // Cell text, one short run per cell, inset from the rule.
    for r in 0..rows {
        for c in 0..cols {
            let x = LEFT + COL_W * c as f32 + 8.0;
            let y = TOP - ROW_H * (r + 1) as f32 + 9.0;
            content.push_str(&format!(
                "BT /F1 10 Tf {x} {y} Td (R{}C{}) Tj ET\n",
                r + 1,
                c + 1
            ));
        }
    }

    let caption_y = TOP - table_h - caption_gap;
    content.push_str(&format!(
        "BT /F1 10 Tf 100 {caption_y} Td (Caption for the table) Tj ET\n"
    ));

    let mut buf: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let total_objects = 5; // catalog, pages, font, page, content
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
    write_obj(
        &mut buf,
        &mut offsets,
        2,
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        4,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
          /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        5,
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
        .as_bytes(),
    );

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

/// Build a single A4 page with body prose, a display equation set apart on its
/// own line, and more prose. Used to exercise equation detection.
///
/// The equation is set in base-14 `Symbol`, which needs no embedded font.
/// MuPDF reports the font name (`Symbol`) so the font signal fires; on some
/// builds the encoding also turns `a + b = g` into `α + β = γ` (character
/// signal), on others the Latin bytes come through unchanged.
pub fn pdf_with_equation() -> Vec<u8> {
    let mut content = String::new();
    // Prose. Long lines, so these set the column width the equation is measured
    // against.
    let body = [
        "The database is a set of directories that each contain a copy of the",
        "same layout, so that applications may add to it without touching any",
        "of the files that another application installed there previously.",
    ];
    for (i, text) in body.iter().enumerate() {
        let y = 740.0 - i as f32 * 14.0;
        content.push_str(&format!("BT /F1 10 Tf 100 {y} Td ({text}) Tj ET\n"));
    }
    // The equation: indented, short, and in a maths font.
    content.push_str("BT /F2 11 Tf 250 680 Td (a + b = g) Tj ET\n");
    for (i, text) in body.iter().enumerate() {
        let y = 650.0 - i as f32 * 14.0;
        content.push_str(&format!("BT /F1 10 Tf 100 {y} Td ({text}) Tj ET\n"));
    }
    two_font_page_pdf(&content, b"/Symbol")
}

/// Build a single A4 page with body prose, a display equation of several rows —
/// an aligned system — set apart from it, and more prose.
///
/// The multi-row shape is the point: with one row a formula's box and its
/// single line rectangle are the same, so only this fixture can show either
/// that the rows are one stop at line scope or that they draw as one box.
/// Rows are deliberately of different widths, so a box over all of them is
/// visibly wider than any one row.
pub fn pdf_with_multiline_equation() -> Vec<u8> {
    let mut content = String::new();
    let body = [
        "The database is a set of directories that each contain a copy of the",
        "same layout, so that applications may add to it without touching any",
        "of the files that another application installed there previously.",
    ];
    for (i, text) in body.iter().enumerate() {
        let y = 740.0 - i as f32 * 14.0;
        content.push_str(&format!("BT /F1 10 Tf 100 {y} Td ({text}) Tj ET\n"));
    }
    // The rows of the system: indented, short, in a maths font, and ragged so
    // their union is wider than any single one.
    for (i, row) in ["a + b = g", "b + g = d + e", "g = a"].iter().enumerate() {
        let y = 680.0 - i as f32 * 16.0;
        content.push_str(&format!("BT /F2 11 Tf 250 {y} Td ({row}) Tj ET\n"));
    }
    for (i, text) in body.iter().enumerate() {
        let y = 600.0 - i as f32 * 14.0;
        content.push_str(&format!("BT /F1 10 Tf 100 {y} Td ({text}) Tj ET\n"));
    }
    two_font_page_pdf(&content, b"/Symbol")
}

/// Build a single A4 page with a large bold heading, several lines of body
/// prose, a short bold subheading at body size, and more prose. Used to
/// exercise heading detection.
pub fn pdf_with_heading() -> Vec<u8> {
    let mut content = String::new();
    // Heading: 18pt bold, well above the 10pt body.
    content.push_str("BT /F2 18 Tf 100 740 Td (2.1. Directory layout) Tj ET\n");
    // Body prose. Long lines so they set the column width.
    let body = [
        "The database is a set of directories that each contain a copy of the",
        "same layout, so that applications may add to it without touching any",
        "of the files that another application installed there previously.",
    ];
    for (i, text) in body.iter().enumerate() {
        let y = 710.0 - i as f32 * 14.0;
        content.push_str(&format!("BT /F1 10 Tf 100 {y} Td ({text}) Tj ET\n"));
    }
    // Subheading: bold at body size, short enough not to fill the column.
    content.push_str("BT /F2 10 Tf 100 650 Td (Ordering) Tj ET\n");
    for (i, text) in body.iter().enumerate() {
        let y = 630.0 - i as f32 * 14.0;
        content.push_str(&format!("BT /F1 10 Tf 100 {y} Td ({text}) Tj ET\n"));
    }

    two_font_page_pdf(&content, b"/Helvetica-Bold")
}

/// Assemble a one-page A4 PDF around `content`, with `/F1` Helvetica and `/F2`
/// the given base font.
fn two_font_page_pdf(content: &str, second_font: &[u8]) -> Vec<u8> {
    let mut buf: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let total_objects = 6; // catalog, pages, text font, second font, page, content
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
    write_obj(
        &mut buf,
        &mut offsets,
        2,
        b"<< /Type /Pages /Kids [5 0 R] /Count 1 >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    let mut font2: Vec<u8> = b"<< /Type /Font /Subtype /Type1 /BaseFont ".to_vec();
    font2.extend_from_slice(second_font);
    font2.extend_from_slice(b" >>");
    write_obj(&mut buf, &mut offsets, 4, &font2);
    write_obj(
        &mut buf,
        &mut offsets,
        5,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
          /Resources << /Font << /F1 3 0 R /F2 4 0 R >> >> /Contents 6 0 R >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        6,
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
        .as_bytes(),
    );

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

/// Assemble a one-page A4 PDF around `content`, with `/F1` Helvetica.
fn single_page_pdf(content: &str) -> Vec<u8> {
    let mut buf: Vec<u8> = b"%PDF-1.4\n".to_vec();
    let total_objects = 5;
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
    write_obj(
        &mut buf,
        &mut offsets,
        2,
        b"<< /Type /Pages /Kids [4 0 R] /Count 1 >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        3,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        4,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
          /Resources << /Font << /F1 3 0 R >> >> /Contents 5 0 R >>",
    );
    write_obj(
        &mut buf,
        &mut offsets,
        5,
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
        .as_bytes(),
    );
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

/// Build a `pages`-page document with body prose, a page number at the foot,
/// and a line at the top that is either the same on every page (a running
/// header) or different on each. Used to exercise furniture detection, which
/// keys on repetition rather than position.
///
/// Note the coordinate flip: PDF content streams are bottom-up, so the header
/// is written at a high `y` and comes out near the *top* of MuPDF's top-down
/// structured text.
pub fn pdf_with_running_header(pages: usize, repeat_header: bool) -> Vec<u8> {
    let body = [
        "The database is a set of directories that each contain a copy of",
        "the same layout, so applications may add to it independently.",
        "Every entry is resolved in order until one of them matches.",
        "A later directory may override what an earlier one provided.",
    ];
    let mut streams = Vec::new();
    for p in 0..pages {
        let mut content = String::new();
        // The varying headers must differ in *words*, not merely in a number:
        // digits are masked before comparison, so "Chapter 1"/"Chapter 2" are
        // one running head with a counter in it, and are meant to be caught.
        let varying = [
            "Installing the database",
            "Resolving a media type",
            "Writing a glob pattern",
            "Matching by magic bytes",
            "Extending the namespace",
            "Recommended checking order",
        ];
        let header = if repeat_header {
            "Shared MIME-info Database".to_owned()
        } else {
            varying[p % varying.len()].to_owned()
        };
        content.push_str(&format!("BT /F1 9 Tf 100 800 Td ({header}) Tj ET\n"));
        for (i, text) in body.iter().enumerate() {
            let y = 700.0 - i as f32 * 16.0;
            content.push_str(&format!("BT /F1 11 Tf 100 {y} Td ({text}) Tj ET\n"));
        }
        content.push_str(&format!("BT /F1 9 Tf 300 40 Td ({}) Tj ET\n", p + 1));
        streams.push(content);
    }

    // Object numbering: 1 catalog, 2 pages, 3 font, then per page 4+2i / 5+2i.
    let total_objects = 3 + 2 * pages;
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
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    write_obj(
        &mut buf,
        &mut offsets,
        2,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
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
    for (i, content) in streams.iter().enumerate() {
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
        write_obj(
            &mut buf,
            &mut offsets,
            content_num,
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
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

/// Build a document whose footer carries two independent fields on one
/// baseline -- a manuscript running head and a "Page N of M" folio -- that
/// swap sides on alternating pages, the way a facing-page layout keeps a
/// running head on the outer edge of both recto and verso. Used to exercise
/// margin-repetition detection when two fields share a physical line and
/// which one reads first therefore differs page to page.
pub fn pdf_with_alternating_margin_fields(pages: usize) -> Vec<u8> {
    let body = [
        "The database is a set of directories that each contain a copy of",
        "the same layout, so applications may add to it independently.",
        "Every entry is resolved in order until one of them matches.",
        "A later directory may override what an earlier one provided.",
    ];
    let header = "AUTHOR SUBMITTED MANUSCRIPT - NF-108620.R1";
    let mut streams = Vec::new();
    for p in 0..pages {
        let mut content = String::new();
        for (i, text) in body.iter().enumerate() {
            let y = 700.0 - i as f32 * 16.0;
            content.push_str(&format!("BT /F1 11 Tf 100 {y} Td ({text}) Tj ET\n"));
        }
        let folio = format!("Page {} of {pages}", p + 1);
        // One BT/ET, one shared baseline: the two fields are two `Tj` calls
        // in the same text object, positioned by a relative `Td` jump between
        // them, so a naive whole-line read sees one string with the fields in
        // whichever order this page put them.
        if p % 2 == 0 {
            content.push_str(&format!(
                "BT /F1 9 Tf 72 40 Td ({header}) Tj 320 0 Td ({folio}) Tj ET\n"
            ));
        } else {
            content.push_str(&format!(
                "BT /F1 9 Tf 72 40 Td ({folio}) Tj 320 0 Td ({header}) Tj ET\n"
            ));
        }
        streams.push(content);
    }

    // Object numbering: 1 catalog, 2 pages, 3 font, then per page 4+2i / 5+2i.
    let total_objects = 3 + 2 * pages;
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
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    write_obj(
        &mut buf,
        &mut offsets,
        2,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
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
    for (i, content) in streams.iter().enumerate() {
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
        write_obj(
            &mut buf,
            &mut offsets,
            content_num,
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
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

/// Build a document whose body lines are each preceded by a line number in
/// the left margin, restarting at 1 on every page -- the manuscript
/// line-numbering common in submission and review drafts. Each number and its
/// body line are separate text objects (as a real line-numbering package
/// typically emits them), positioned with a clear gap between the number
/// column and the body's left edge.
pub fn pdf_with_line_numbers(pages: usize, lines_per_page: usize) -> Vec<u8> {
    let words = [
        "Every",
        "entry",
        "in",
        "the",
        "database",
        "is",
        "resolved",
        "before",
        "the",
        "next",
        "one",
        "is",
        "considered",
        "at",
        "all",
        "here",
    ];
    let mut streams = Vec::new();
    for _ in 0..pages {
        let mut content = String::new();
        for i in 0..lines_per_page {
            let y = 780.0 - i as f32 * 16.0;
            let number = i + 1;
            let text = words[i % words.len()];
            content.push_str(&format!("BT /F1 9 Tf 50 {y} Td ({number}) Tj ET\n"));
            content.push_str(&format!(
                "BT /F1 11 Tf 100 {y} Td ({text} continues here) Tj ET\n"
            ));
        }
        streams.push(content);
    }

    // Object numbering: 1 catalog, 2 pages, 3 font, then per page 4+2i / 5+2i.
    let total_objects = 3 + 2 * pages;
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
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 4 + 2 * i)).collect();
    write_obj(
        &mut buf,
        &mut offsets,
        2,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {pages} >>",
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
    for (i, content) in streams.iter().enumerate() {
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
        write_obj(
            &mut buf,
            &mut offsets,
            content_num,
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
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

/// Build a single A4 page with a lead-in line ending in a colon, a bulleted
/// list of three items, and a closing sentence. Used to exercise list
/// detection, where items carry no terminating full stop.
pub fn pdf_with_list() -> Vec<u8> {
    let mut content = String::new();
    content.push_str("BT /F1 11 Tf 100 700 Td (The files created by the tool are:) Tj ET\n");
    let items = [
        "the globs file, mapping names to types",
        "the magic file, mapping content to types",
        "the aliases file, mapping aliases to types",
    ];
    for (i, text) in items.iter().enumerate() {
        let y = 670.0 - i as f32 * 18.0;
        // The bullet sits at the list indent, its text a little further in.
        content.push_str(&format!("BT /F1 11 Tf 100 {y} Td (\\267) Tj ET\n"));
        content.push_str(&format!("BT /F1 11 Tf 118 {y} Td ({text}) Tj ET\n"));
    }
    content.push_str("BT /F1 11 Tf 100 600 Td (Each of them is regenerated in turn.) Tj ET\n");
    single_page_pdf(&content)
}

/// Build a single page carrying rotated text: a 90&deg; stamp down the left
/// margin and a 45&deg; watermark across the middle. With `whole_page` the page
/// contains *only* rotated text, so the rotated direction is the dominant one.
///
/// Rotation needs the six-number `Tm` operator rather than `Td`.
pub fn pdf_with_rotated_text(whole_page: bool) -> Vec<u8> {
    let body = [
        "The database is a set of directories that each contain a copy of",
        "the same layout, so applications may add to it independently.",
        "Every entry is resolved in order until one of them matches.",
        "A later directory may override what an earlier one provided.",
        "Each application installs exactly one file into the directory.",
        "The order in which they are read is not otherwise significant.",
    ];
    let mut content = String::new();
    if whole_page {
        // Every line turned ninety degrees: a page laid out sideways, where
        // the rotated direction *is* the reading direction.
        for (i, text) in body.iter().enumerate() {
            let x = 100.0 + i as f32 * 16.0;
            content.push_str(&format!(
                "BT /F1 11 Tf 0 1 -1 0 {x} 120 Tm ({text}) Tj ET\n"
            ));
        }
        return single_page_pdf(&content);
    }
    // 90 degrees counter-clockwise, down the left margin.
    content.push_str("BT /F1 10 Tf 0 1 -1 0 40 300 Tm (CONFIDENTIAL DRAFT COPY) Tj ET\n");
    // 45 degrees across the page.
    content.push_str("BT /F1 30 Tf 0.707 0.707 -0.707 0.707 150 250 Tm (PREPRINT) Tj ET\n");
    for (i, text) in body.iter().enumerate() {
        let y = 700.0 - i as f32 * 16.0;
        content.push_str(&format!("BT /F1 11 Tf 100 {y} Td ({text}) Tj ET\n"));
    }
    single_page_pdf(&content)
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
