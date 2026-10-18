//! A small PDF writer for reports: A4 pages, Helvetica text, filled boxes, lines and images.
//! Text uses the PDF standard fonts (nothing to embed); characters outside Western European
//! (WinAnsi) show as "?".

use std::fmt::Write as _;
use std::io::Write as _;

/// Character widths (1/1000 em) for codes 32…255 in WinAnsi encoding.
const REGULAR: [u16; 224] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278,
    278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722,
    667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, 556,
    556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761,
    761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 278, 333, 556, 556, 556, 556, 260,
    556, 333, 737, 370, 556, 584, 333, 737, 333, 400, 584, 333, 333, 333, 556, 537, 278, 333, 333, 365, 556, 834, 834, 834, 611, 667, 667,
    667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278, 722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722,
    667, 667, 611, 556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278, 556, 556, 556, 556, 556, 556, 556, 584,
    611, 556, 556, 556, 556, 500, 556, 500,
];
const BOLD: [u16; 224] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333,
    333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722,
    667, 944, 667, 667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611, 611,
    611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761,
    761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 761, 278, 333, 556, 556, 556, 556, 280,
    556, 333, 737, 370, 556, 584, 333, 737, 333, 400, 584, 333, 333, 333, 611, 556, 278, 333, 333, 365, 556, 834, 834, 834, 611, 722, 722,
    722, 722, 722, 722, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278, 722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722,
    667, 667, 611, 556, 556, 556, 556, 556, 556, 889, 556, 556, 556, 556, 556, 278, 278, 278, 278, 611, 611, 611, 611, 611, 611, 611, 584,
    611, 611, 611, 611, 611, 556, 611, 556,
];

#[derive(Clone, Copy, PartialEq)]
pub enum Font {
    Regular,
    Bold,
}

pub type Rgb = (u8, u8, u8);

struct Image {
    w: u32,
    h: u32,
    rgb: Vec<u8>,
    alpha: Option<Vec<u8>>,
}

pub struct Pdf {
    pages: Vec<String>,
    images: Vec<Image>,
    /// page being drawn on
    cur: usize,
}

/// Character → WinAnsi byte.
fn ansi(c: char) -> u8 {
    match c {
        ' '..='~' => c as u8,
        '\u{a0}'..='\u{ff}' => c as u32 as u8,
        '€' => 0x80,
        '…' => 0x85,
        '‘' => 0x91,
        '’' => 0x92,
        '“' => 0x93,
        '”' => 0x94,
        '•' => 0x95,
        '–' => 0x96,
        '—' => 0x97,
        '™' => 0x99,
        '\t' => b' ',
        _ => b'?',
    }
}

fn byte_width(b: u8, font: Font) -> u16 {
    match b {
        0x80 | 0x96 => 556,
        0x85 | 0x97 | 0x99 => 1000,
        0x91 | 0x92 => 222,
        0x95 => 350,
        0x93 | 0x94 => 333,
        32..=255 => match font {
            Font::Regular => REGULAR[(b - 32) as usize],
            Font::Bold => BOLD[(b - 32) as usize],
        },
        _ => 500,
    }
}

fn col(c: Rgb) -> String {
    format!("{:.3} {:.3} {:.3}", c.0 as f32 / 255.0, c.1 as f32 / 255.0, c.2 as f32 / 255.0)
}

impl Default for Pdf {
    fn default() -> Self {
        Self::new()
    }
}

impl Pdf {
    /// A4 portrait, in points.
    pub const W: f32 = 595.28;
    pub const H: f32 = 841.89;

    pub fn new() -> Self {
        Pdf { pages: vec![String::new()], images: vec![], cur: 0 }
    }

    pub fn new_page(&mut self) {
        self.pages.push(String::new());
        self.cur = self.pages.len() - 1;
    }

    /// Draw on page `i` (0-based) again, e.g. to add "page 2 of 5" footers at the end.
    pub fn goto_page(&mut self, i: usize) {
        self.cur = i.min(self.pages.len() - 1);
    }

    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    fn out(&mut self) -> &mut String {
        &mut self.pages[self.cur]
    }

    /// Width of `s` in points.
    pub fn width(s: &str, size: f32, font: Font) -> f32 {
        s.chars().map(|c| byte_width(ansi(c), font) as f32).sum::<f32>() * size / 1000.0
    }

    /// `s` shortened with "…" so it fits in `max` points.
    pub fn fit(s: &str, size: f32, font: Font, max: f32) -> String {
        if Self::width(s, size, font) <= max {
            return s.to_string();
        }
        let mut out = String::new();
        let ell = Self::width("…", size, font);
        let mut w = 0.0;
        for c in s.chars() {
            let cw = byte_width(ansi(c), font) as f32 * size / 1000.0;
            if w + cw + ell > max {
                break;
            }
            w += cw;
            out.push(c);
        }
        out.trim_end().to_string() + "…"
    }

    /// Text with its baseline at `y` points from the top of the page.
    pub fn text(&mut self, x: f32, y: f32, size: f32, font: Font, color: Rgb, s: &str) {
        let mut esc = String::new();
        for b in s.chars().map(ansi) {
            match b {
                b'(' | b')' | b'\\' => {
                    esc.push('\\');
                    esc.push(b as char);
                }
                32..=126 => esc.push(b as char),
                _ => {
                    let _ = write!(esc, "\\{:03o}", b);
                }
            }
        }
        let f = if font == Font::Bold { "F2" } else { "F1" };
        let c = col(color);
        let y = Self::H - y;
        let _ = writeln!(self.out(), "BT {c} rg /{f} {size:.2} Tf {x:.2} {y:.2} Td ({esc}) Tj ET");
    }

    pub fn text_right(&mut self, right: f32, y: f32, size: f32, font: Font, color: Rgb, s: &str) {
        let w = Self::width(s, size, font);
        self.text(right - w, y, size, font, color, s);
    }

    pub fn text_center(&mut self, cx: f32, y: f32, size: f32, font: Font, color: Rgb, s: &str) {
        let w = Self::width(s, size, font);
        self.text(cx - w / 2.0, y, size, font, color, s);
    }

    /// Filled rectangle; `y` = top edge from the top of the page.
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: Rgb) {
        let c = col(fill);
        let y = Self::H - y - h;
        let _ = writeln!(self.out(), "{c} rg {x:.2} {y:.2} {w:.2} {h:.2} re f");
    }

    /// Rounded filled rectangle.
    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, fill: Rgb) {
        let c = col(fill);
        let (x0, y0, x1, y1) = (x, Self::H - y - h, x + w, Self::H - y);
        let r = r.min(w / 2.0).min(h / 2.0);
        let k = r * 0.5523;
        let o = self.out();
        let _ = writeln!(o, "{c} rg {:.2} {y0:.2} m", x0 + r);
        let _ = writeln!(o, "{:.2} {y0:.2} l {:.2} {y0:.2} {x1:.2} {:.2} {x1:.2} {:.2} c", x1 - r, x1 - r + k, y0 + r - k, y0 + r);
        let _ = writeln!(o, "{x1:.2} {:.2} l {x1:.2} {:.2} {:.2} {y1:.2} {:.2} {y1:.2} c", y1 - r, y1 - r + k, x1 - r + k, x1 - r);
        let _ = writeln!(o, "{:.2} {y1:.2} l {:.2} {y1:.2} {x0:.2} {:.2} {x0:.2} {:.2} c", x0 + r, x0 + r - k, y1 - r + k, y1 - r);
        let _ = writeln!(o, "{x0:.2} {:.2} l {x0:.2} {:.2} {:.2} {y0:.2} {:.2} {y0:.2} c f", y0 + r, y0 + r - k, x0 + r - k, x0 + r);
    }

    pub fn line(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, width: f32, color: Rgb) {
        let c = col(color);
        let (y1, y2) = (Self::H - y1, Self::H - y2);
        let _ = writeln!(self.out(), "{c} RG {width:.2} w {x1:.2} {y1:.2} m {x2:.2} {y2:.2} l S");
    }

    /// Add an RGBA image; returns its number for `image`.
    pub fn add_image(&mut self, w: u32, h: u32, rgba: &[u8]) -> usize {
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        let mut alpha = Vec::with_capacity((w * h) as usize);
        for px in rgba.chunks_exact(4) {
            rgb.extend_from_slice(&px[..3]);
            alpha.push(px[3]);
        }
        let alpha = alpha.iter().any(|&a| a < 255).then_some(alpha);
        self.images.push(Image { w, h, rgb, alpha });
        self.images.len() - 1
    }

    /// Draw image `n` with its top-left corner at (x, y).
    pub fn image(&mut self, n: usize, x: f32, y: f32, w: f32, h: f32) {
        let y = Self::H - y - h;
        let _ = writeln!(self.out(), "q {w:.2} 0 0 {h:.2} {x:.2} {y:.2} cm /Im{n} Do Q");
    }

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut e = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        let _ = e.write_all(data);
        e.finish().unwrap_or_default()
    }

    /// The finished PDF file.
    pub fn finish(self, title: &str) -> Vec<u8> {
        let mut objs: Vec<Vec<u8>> = vec![];
        // 1 catalog, 2 pages, 3 regular font, 4 bold font, 5 info; then images, then pages
        let n_img = self.images.len();
        let first_img = 6;
        let img_objs: usize = self.images.iter().map(|i| if i.alpha.is_some() { 2 } else { 1 }).sum();
        let first_page = first_img + img_objs;
        let kids: Vec<String> = (0..self.pages.len()).map(|i| format!("{} 0 R", first_page + i * 2)).collect();
        objs.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
        objs.push(format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), self.pages.len()).into_bytes());
        objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec());
        objs.push(b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec());
        let esc_title: String =
            title.chars().map(ansi).filter(|b| (32..127).contains(b) && !matches!(b, b'(' | b')' | b'\\')).map(|b| b as char).collect();
        objs.push(format!("<< /Title ({esc_title}) /Producer (RadioMaster) /Creator (RadioMaster) >>").into_bytes());
        let mut img_ids = vec![];
        for im in &self.images {
            let id = first_img + objs.len() - 5;
            img_ids.push(id);
            let smask = if im.alpha.is_some() { format!(" /SMask {} 0 R", id + 1) } else { String::new() };
            let data = Self::deflate(&im.rgb);
            let mut o = format!(
                "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode /Length {}{smask} >>\nstream\n",
                im.w,
                im.h,
                data.len()
            )
            .into_bytes();
            o.extend_from_slice(&data);
            o.extend_from_slice(b"\nendstream");
            objs.push(o);
            if let Some(a) = &im.alpha {
                let data = Self::deflate(a);
                let mut o = format!(
                    "<< /Type /XObject /Subtype /Image /Width {} /Height {} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length {} >>\nstream\n",
                    im.w,
                    im.h,
                    data.len()
                )
                .into_bytes();
                o.extend_from_slice(&data);
                o.extend_from_slice(b"\nendstream");
                objs.push(o);
            }
        }
        let xobjects: String = (0..n_img).map(|i| format!("/Im{i} {} 0 R ", img_ids[i])).collect();
        for (i, content) in self.pages.iter().enumerate() {
            let content_id = first_page + i * 2 + 1;
            objs.push(
                format!(
                    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Resources << /Font << /F1 3 0 R /F2 4 0 R >> /XObject << {xobjects}>> >> /Contents {content_id} 0 R >>",
                    Self::W,
                    Self::H
                )
                .into_bytes(),
            );
            let data = Self::deflate(content.as_bytes());
            let mut o = format!("<< /Length {} /Filter /FlateDecode >>\nstream\n", data.len()).into_bytes();
            o.extend_from_slice(&data);
            o.extend_from_slice(b"\nendstream");
            objs.push(o);
        }
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = vec![];
        for (i, o) in objs.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
        for off in offsets {
            out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{xref}\n%%EOF\n", objs.len() + 1).as_bytes(),
        );
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_valid_looking_pdf() {
        let mut p = Pdf::new();
        p.rect(20.0, 20.0, 100.0, 40.0, (20, 120, 210));
        p.text(30.0, 45.0, 14.0, Font::Bold, (255, 255, 255), "Report (test) — café \\ 100%");
        let img = p.add_image(2, 1, &[255, 0, 0, 255, 0, 0, 255, 128]);
        p.image(img, 200.0, 20.0, 20.0, 10.0);
        p.new_page();
        p.line(10.0, 10.0, 200.0, 10.0, 1.0, (0, 0, 0));
        let bytes = p.finish("Test");
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.starts_with("%PDF-1.4"));
        assert!(s.contains("/Count 2"));
        assert!(s.trim_end().ends_with("%%EOF"));
        assert_eq!(Pdf::fit("A long title here", 10.0, Font::Regular, 40.0).chars().last(), Some('…'));
        assert!((Pdf::width("Hello", 10.0, Font::Regular) - 22.78).abs() < 0.1);
    }
}
