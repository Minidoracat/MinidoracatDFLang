//! 依目前的文字格大小畫字圖集 PNG：每字畫在兩格寬、一格高的方框裡，再切成左右兩張貼圖。
//!
//! 遊戲以最近鄰縮放貼圖；字圖的像素大小剛好等於畫面上的格子大小，就不會被縮放而糊掉或斷筆。
//! 清晰度靠三件事：字形套用字型內建的 hinting（思源黑體是 CFF 的 PostScript hints，橫筆對齊像素格）；
//! 字的原點取整數像素，hinting 才不會被小數位移抵銷；覆蓋率先換算成 ALPHA_EXPONENT 次方再存成 alpha。
//! 字型依序找：第一個含有該字的字型負責畫（主字型之後的字型用來補字）。

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, HintingInstance, HintingOptions, OutlinePen};
use skrifa::{FontRef, MetadataProvider};
use zeno::{Command, Mask, PathBuilder};

/// 每列幾張貼圖；偶數，同一字的左右半格不會被拆到兩列。
pub const TILES_PER_ROW: usize = 64;
/// 字身（em）相對格高的倍數。方塊字約佔字身的 0.9，1.0 時字高約為格高的九成，上下各留約一像素。
const EM_PER_CELL: f32 = 1.0;
/// 覆蓋率存成 alpha 前的次方。實測遊戲畫 ADDCOLOR 貼圖時 alpha 約被平方（53.16，字圖 alpha 0.56 在畫面上只剩約 0.26），
/// 筆畫邊緣會變得太淡、看起來斷筆；先取 0.7 次方，畫面上約為覆蓋率的 1.4～1.7 次方，邊緣收細但筆畫連續。
const ALPHA_EXPONENT: f32 = 0.7;

pub struct Report {
    pub tiles: usize,
    pub missing: Vec<char>,
}

/// 字型檔的原始位元組；FontRef 只借用，畫字時才建立。
pub struct Font(Vec<u8>);

pub fn load_fonts(paths: &[&str]) -> Result<Vec<Font>, String> {
    paths
        .iter()
        .map(|p| {
            let data = std::fs::read(p).map_err(|e| format!("{p}: {e}"))?;
            FontRef::new(&data).map_err(|e| format!("{p}: {e}"))?;
            Ok(Font(data))
        })
        .collect()
}

/// 收集字形輪廓：字型座標 y 朝上，點陣 y 朝下；(x, y) 是原點（基線左端）在方框裡的位置。
struct Pen {
    path: Vec<Command>,
    x: f32,
    y: f32,
}

impl Pen {
    fn at(&self, x: f32, y: f32) -> [f32; 2] {
        [self.x + x, self.y - y]
    }
}

impl OutlinePen for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.path.move_to(p);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.path.line_to(p);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        let (c, p) = (self.at(cx0, cy0), self.at(x, y));
        self.path.quad_to(c, p);
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        let (c0, c1, p) = (self.at(cx0, cy0), self.at(cx1, cy1), self.at(x, y));
        self.path.curve_to(c0, c1, p);
    }

    fn close(&mut self) {
        self.path.close();
    }
}

pub fn render(fonts: &[Font], chars: &[char], cell_w: usize, cell_h: usize, out: &Path) -> Result<Report, String> {
    let (box_w, box_h) = (cell_w * 2, cell_h);
    let tiles = chars.len() * 2;
    let width = TILES_PER_ROW.min(tiles.max(2)) * cell_w;
    let height = tiles.div_ceil(TILES_PER_ROW).max(1) * cell_h;
    let mut rgba = vec![0u8; width * height * 4];
    let size = Size::new((cell_h as f32 * EM_PER_CELL).round());
    let location = LocationRef::default();
    let alpha: Vec<u8> = (0..=255u32).map(|a| ((a as f32 / 255.0).powf(ALPHA_EXPONENT) * 255.0).round() as u8).collect();

    let refs: Vec<FontRef> = fonts.iter().map(|f| FontRef::new(&f.0).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
    let outlines: Vec<_> = refs.iter().map(|f| f.outline_glyphs()).collect();
    let hinting: Vec<HintingInstance> = outlines
        .iter()
        .map(|o| HintingInstance::new(o, size, location, HintingOptions::default()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;

    let mut mask = vec![0u8; box_w * box_h];
    let mut missing = Vec::new();
    for (i, &ch) in chars.iter().enumerate() {
        let found = refs.iter().enumerate().find_map(|(f, font)| {
            let gid = font.charmap().map(ch).filter(|g| g.to_u32() != 0)?;
            Some((f, gid, outlines[f].get(gid)?))
        });
        let Some((f, gid, glyph)) = found else {
            missing.push(ch);
            continue;
        };
        let metrics = refs[f].metrics(size, location);
        let advance = refs[f].glyph_metrics(size, location).advance_width(gid).unwrap_or(size.ppem().unwrap_or(0.0));
        // 水平依字寬置中；垂直讓「上緣到下緣」的中線落在方框中線
        let mut pen = Pen {
            path: Vec::new(),
            x: ((box_w as f32 - advance) / 2.0).round(),
            y: (box_h as f32 / 2.0 + (metrics.ascent + metrics.descent) / 2.0).round(),
        };
        glyph.draw(DrawSettings::hinted(&hinting[f], false), &mut pen).map_err(|e| format!("{ch}: {e}"))?;
        mask.fill(0);
        Mask::new(&pen.path).size(box_w as u32, box_h as u32).render_into(&mut mask, None);

        let (tile_x, tile_y) = ((2 * i) % TILES_PER_ROW * cell_w, (2 * i) / TILES_PER_ROW * cell_h);
        for y in 0..box_h {
            for x in 0..box_w {
                let o = ((tile_y + y) * width + tile_x + x) * 4;
                rgba[o..o + 4].copy_from_slice(&[255, 255, 255, alpha[mask[y * box_w + x] as usize]]);
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
