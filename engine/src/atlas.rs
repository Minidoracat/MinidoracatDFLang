//! 依目前的文字格大小畫字圖集 PNG：每字畫在兩格寬、一格高的方框裡，再切成左右兩張貼圖。
//!
//! 遊戲以最近鄰縮放貼圖；字圖的像素大小剛好等於畫面上的格子大小，就不會被縮放而糊掉或斷筆。
//! 字型依序找：第一個含有該字的字型負責畫（主字型之後的字型用來補字）。

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use fontdue::{Font, FontSettings};

/// 每列幾張貼圖；偶數，同一字的左右半格不會被拆到兩列。
pub const TILES_PER_ROW: usize = 64;
/// 方塊字約佔字身的 0.92；字身取格高的 1.08 倍，字剛好填滿格高。
const EM_PER_CELL: f32 = 1.08;

pub struct Report {
    pub tiles: usize,
    pub missing: Vec<char>,
}

pub fn load_fonts(paths: &[&str]) -> Result<Vec<Font>, String> {
    paths
        .iter()
        .map(|p| {
            let data = std::fs::read(p).map_err(|e| format!("{p}: {e}"))?;
            Font::from_bytes(data, FontSettings::default()).map_err(|e| format!("{p}: {e}"))
        })
        .collect()
}

pub fn render(fonts: &[Font], chars: &[char], cell_w: usize, cell_h: usize, out: &Path) -> Result<Report, String> {
    let (box_w, box_h) = (cell_w * 2, cell_h);
    let tiles = chars.len() * 2;
    let width = TILES_PER_ROW.min(tiles.max(2)) * cell_w;
    let height = tiles.div_ceil(TILES_PER_ROW).max(1) * cell_h;
    let mut rgba = vec![0u8; width * height * 4];
    let px = cell_h as f32 * EM_PER_CELL;
    let mut missing = Vec::new();

    for (i, &ch) in chars.iter().enumerate() {
        let Some(font) = fonts.iter().find(|f| f.lookup_glyph_index(ch) != 0) else {
            missing.push(ch);
            continue;
        };
        let line = font.horizontal_line_metrics(px).ok_or("font has no horizontal metrics")?;
        let (m, bitmap) = font.rasterize(ch, px);
        // 水平依字寬置中；垂直讓「上緣到下緣」的中線落在方框中線
        let left = (box_w as f32 - m.advance_width) / 2.0 + m.xmin as f32;
        let baseline = box_h as f32 / 2.0 + (line.ascent + line.descent) / 2.0;
        let top = baseline - (m.ymin as f32 + m.height as f32);
        let (tile_x, tile_y) = ((2 * i) % TILES_PER_ROW * cell_w, (2 * i) / TILES_PER_ROW * cell_h);
        for gy in 0..m.height {
            for gx in 0..m.width {
                let (x, y) = (left.round() as isize + gx as isize, top.round() as isize + gy as isize);
                if x < 0 || y < 0 || x >= box_w as isize || y >= box_h as isize {
                    continue;
                }
                let o = ((tile_y + y as usize) * width + tile_x + x as usize) * 4;
                rgba[o..o + 4].copy_from_slice(&[255, 255, 255, bitmap[gy * m.width + gx]]);
            }
        }
    }

    let file = File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut encoder = png::Encoder::new(BufWriter::new(file), width as u32, height as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgba).map_err(|e| e.to_string())?;
    Ok(Report { tiles, missing })
}
