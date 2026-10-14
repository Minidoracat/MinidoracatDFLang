//! 標題畫面吉祥物：依目前的文字格大小把圖縮成整格大小，底部疊上名牌（語言標籤＋作者名），以及 PNG 輸出。
//!
//! 遊戲一格畫一張貼圖，貼圖像素剛好等於格子像素時才不會再被遊戲縮放；所以圖在啟動時才縮。
//! 縮小用面積平均（預乘 alpha），邊緣不會有色邊；名牌文字用和字圖集相同的 hinting 點陣化，跟介面上的中文一樣清楚。

use std::fs::File;
use std::io::{BufReader, BufWriter};
use std::path::Path;

use crate::atlas::{Font, Glyphs};

/// 名牌配色：吉祥物外套的深藍底、白字語言標籤、粉紅色作者名、白色外框（和貼紙白邊一致）
const LABEL_BG: [f32; 3] = [38.0, 48.0, 94.0];
const LABEL_FG: [f32; 3] = [255.0, 255.0, 255.0];
const LABEL_CREDIT: [f32; 3] = [255.0, 182.0, 213.0];
const LABEL_EDGE: [f32; 3] = [255.0, 255.0, 255.0];

pub fn write_png(out: &Path, width: usize, height: usize, rgba: &[u8]) -> Result<(), String> {
    let file = File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(rgba).map_err(|e| e.to_string())
}

/// 預乘 alpha 的 RGBA 畫布（值域 0～1）。
struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(w: usize, h: usize) -> Self {
        Canvas { w, h, px: vec![[0.0; 4]; w * h] }
    }

    /// 在 (x, y) 疊上一個預乘像素（over 運算）。
    fn over(&mut self, x: usize, y: usize, p: [f32; 4]) {
        let d = &mut self.px[y * self.w + x];
        (0..4).for_each(|k| d[k] = p[k] + d[k] * (1.0 - p[3]));
    }

    fn to_rgba(&self) -> Vec<u8> {
        self.px
            .iter()
            .flat_map(|p| {
                let a = p[3];
                let c = |v: f32| if a > 0.0 { (v / a * 255.0).round().clamp(0.0, 255.0) as u8 } else { 0 };
                [c(p[0]), c(p[1]), c(p[2]), (a * 255.0).round().clamp(0.0, 255.0) as u8]
            })
            .collect()
    }
}

fn solid(rgb: [f32; 3], coverage: f32) -> [f32; 4] {
    [rgb[0] / 255.0 * coverage, rgb[1] / 255.0 * coverage, rgb[2] / 255.0 * coverage, coverage]
}

/// 每個輸出座標涵蓋的來源像素與權重（面積平均，權重和為 1）。
fn spans(src: usize, dst: usize) -> Vec<Vec<(usize, f32)>> {
    let scale = src as f32 / dst as f32;
    (0..dst)
        .map(|o| {
            let (a, b) = (o as f32 * scale, (o + 1) as f32 * scale);
            (a.floor() as usize..(b.ceil() as usize).min(src))
                .filter_map(|i| {
                    let w = b.min(i as f32 + 1.0) - a.max(i as f32);
                    (w > 0.0).then_some((i, w / scale))
                })
                .collect()
        })
        .collect()
}

/// 讀 8-bit RGBA PNG，以面積平均縮成 dw×dh，回傳預乘像素。
fn load_scaled(src: &Path, dh: usize) -> Result<(usize, Vec<[f32; 4]>), String> {
    let file = File::open(src).map_err(|e| format!("{}: {e}", src.display()))?;
    let mut reader = png::Decoder::new(BufReader::new(file)).read_info().map_err(|e| format!("{}: {e}", src.display()))?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or("png too large")?];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("{}: {e}", src.display()))?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err(format!("{}: 需要 8-bit RGBA PNG", src.display()));
    }
    let (sw, sh) = (info.width as usize, info.height as usize);
    let dw = ((sw * dh) as f32 / sh as f32).round().max(1.0) as usize;
    let premul: Vec<[f32; 4]> = buf[..sw * sh * 4]
        .chunks_exact(4)
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 / 255.0 * a, p[1] as f32 / 255.0 * a, p[2] as f32 / 255.0 * a, a]
        })
        .collect();
    let mut rows = vec![[0f32; 4]; dw * sh];
    for (x, span) in spans(sw, dw).iter().enumerate() {
        for y in 0..sh {
            let acc = &mut rows[y * dw + x];
            for &(i, w) in span {
                let p = premul[y * sw + i];
                (0..4).for_each(|k| acc[k] += p[k] * w);
            }
        }
    }
    let mut out = vec![[0f32; 4]; dw * dh];
    for (y, span) in spans(sh, dh).iter().enumerate() {
        for x in 0..dw {
            let acc = &mut out[y * dw + x];
            for &(j, w) in span {
                let p = rows[j * dw + x];
                (0..4).for_each(|k| acc[k] += p[k] * w);
            }
        }
    }
    Ok((dw, out))
}

/// 圓角矩形 (0,0)–(w,h)、圓角半徑 r 在像素中心 (x+0.5, y+0.5) 的覆蓋率（有一像素寬的反鋸齒）。
fn rounded_rect(x: usize, y: usize, w: f32, h: f32, r: f32) -> f32 {
    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
    let qx = (px - w / 2.0).abs() - (w / 2.0 - r);
    let qy = (py - h / 2.0).abs() - (h / 2.0 - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - r;
    (0.5 - outside).clamp(0.0, 1.0)
}

/// 一行字的覆蓋率遮罩，高 line_h：全形字（CP437 以外）每字兩格寬，和介面上的中文字一樣；CP437 字照字型的字寬排。
/// 回傳 (寬, 遮罩)。
fn text_line(glyphs: &Glyphs, text: &str, cell_w: usize, line_h: usize) -> Result<(usize, Vec<u8>), String> {
    let boxes: Vec<(char, usize)> = text
        .chars()
        .map(|ch| match crate::cp437::byte_of(ch) {
            Some(_) => glyphs.advance(ch).map(|a| (ch, a.round().max(1.0) as usize)).ok_or_else(|| format!("「{ch}」沒有字型能畫")),
            None => Ok((ch, 2 * cell_w)),
        })
        .collect::<Result<_, _>>()?;
    let width: usize = boxes.iter().map(|b| b.1).sum();
    let mut line = vec![0u8; width * line_h];
    let mut x0 = 0;
    for (ch, w) in boxes {
        let mut mask = vec![0u8; w * line_h];
        if !glyphs.draw(ch, w, line_h, &mut mask)? {
            return Err(format!("「{ch}」沒有字型能畫"));
        }
        for y in 0..line_h {
            for x in 0..w {
                line[y * width + x0 + x] = mask[y * w + x];
            }
        }
        x0 += w;
    }
    Ok((width, line))
}

/// 吉祥物圖縮成高 rows 格；底部置中疊一個名牌，壓住圖的下緣半個名牌高。
/// 名牌第一行是語言標籤（label，和介面上的中文字同大），第二行是作者名（credit，小一號，可空）。
/// 輸出寬補透明到整格，回傳寬幾格。
pub fn mascot(src: &Path, out: &Path, cell_w: usize, cell_h: usize, rows: usize, label: &str, credit: &str, fonts: &[Font])
    -> Result<usize, String> {
    let height = rows * cell_h;
    let edge = (cell_h / 8).max(1);
    let (pad_x, pad_y) = ((cell_w * 3).div_ceil(5), (cell_h / 5).max(1));
    let big = Glyphs::new(fonts, cell_h)?;
    let small_px = (cell_h as f32 * 0.75).round();
    let small = Glyphs::with_px(fonts, small_px)?;
    let small_h = (cell_h as f32 * 0.8).round() as usize;
    let mut lines = Vec::new();
    if !label.is_empty() {
        lines.push((text_line(&big, label, cell_w, cell_h)?, cell_h, LABEL_FG));
    }
    if !credit.is_empty() {
        lines.push((text_line(&small, credit, cell_w, small_h)?, small_h, LABEL_CREDIT));
    }
    let text_w = lines.iter().map(|((w, _), _, _)| *w).max().unwrap_or(0);
    let text_h: usize = lines.iter().map(|(_, h, _)| *h).sum();
    let (tag_w, tag_h) = if lines.is_empty() { (0, 0) } else { (text_w + 2 * (pad_x + edge), text_h + 2 * (pad_y + edge)) };

    let (img_w, img) = load_scaled(src, height - tag_h / 2)?;
    let cols = img_w.max(tag_w).div_ceil(cell_w);
    let mut canvas = Canvas::new(cols * cell_w, height);
    let left = (canvas.w - img_w) / 2;
    for (i, p) in img.iter().enumerate() {
        canvas.over(left + i % img_w, i / img_w, *p);
    }
    if lines.is_empty() {
        write_png(out, canvas.w, canvas.h, &canvas.to_rgba())?;
        return Ok(cols);
    }

    let (lx, ly) = ((canvas.w - tag_w) / 2, height - tag_h);
    let radius = cell_h as f32 * 0.4;
    for y in 0..tag_h {
        for x in 0..tag_w {
            let outer = rounded_rect(x, y, tag_w as f32, tag_h as f32, radius + edge as f32);
            let inner = if x < edge || y < edge {
                0.0
            } else {
                rounded_rect(x - edge, y - edge, (tag_w - 2 * edge) as f32, (tag_h - 2 * edge) as f32, radius)
            };
            canvas.over(lx + x, ly + y, solid(LABEL_EDGE, outer));
            canvas.over(lx + x, ly + y, solid(LABEL_BG, inner));
        }
    }
    let mut ty = ly + edge + pad_y;
    for ((w, mask), h, color) in &lines {
        let tx = lx + (tag_w - w) / 2;
        for y in 0..*h {
            for x in 0..*w {
                canvas.over(tx + x, ty + y, solid(*color, mask[y * w + x] as f32 / 255.0));
            }
        }
        ty += h;
    }

    write_png(out, canvas.w, canvas.h, &canvas.to_rgba())?;
    Ok(cols)
}
