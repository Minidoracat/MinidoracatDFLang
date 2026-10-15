//! 依目前的文字格大小畫字圖集 PNG：每字畫在兩格寬、一格高的方框裡，再切成左右兩張貼圖；
//! 後面接每字四張「往下位移半格」的貼圖（上列左右、下列左右），給分頁標籤這類半格位移的文字用。
//!
//! 遊戲以最近鄰縮放貼圖；字圖的像素大小剛好等於畫面上的格子大小，就不會被縮放而糊掉或斷筆。
//! 清晰度靠三件事：字形套用字型內建的 hinting（思源黑體是 CFF 的 PostScript hints，橫筆對齊像素格）；
//! 字的原點取整數像素，hinting 才不會被小數位移抵銷；覆蓋率先換算成 ALPHA_EXPONENT 次方再存成 alpha。
//! 字型依序找：第一個含有該字的字型負責畫（主字型之後的字型用來補字）。

use std::path::Path;

use skrifa::instance::{LocationRef, Size};
use skrifa::outline::{DrawSettings, HintingInstance, HintingOptions, OutlinePen};
use skrifa::{FontRef, MetadataProvider};
use zeno::{Command, Mask, PathBuilder};

/// 每列幾張貼圖；偶數，同一字的左右半格不會被拆到兩列。
pub const TILES_PER_ROW: usize = 64;
/// 字身（em）相對格高的倍數。方塊字約佔字身的 0.88：0.875 時 16 px 的格子字高約 12 px，和英文大寫字母（約 11 px）相近。
/// 1.0 時字高約 14 px、上下只剩一像素：方框畫在最後一列下緣的框線（冒險側欄、日誌的協議欄）會壓到最後一列的字。
const EM_PER_CELL: f32 = 0.875;
/// 字往上提的格高比例：英文大寫字母在格子的第 1～11 列，中文置中後上提一像素（16 px 的格子），字身中線才和英文對齊，
/// 下緣也離框線遠一點。
const RAISE_PER_CELL: f32 = 1.0 / 16.0;
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

/// 依格高設定好 hinting 的字型組；字型依序找，第一個有該字的負責畫。
pub struct Glyphs<'a> {
    refs: Vec<FontRef<'a>>,
    outlines: Vec<skrifa::outline::OutlineGlyphCollection<'a>>,
    hinting: Vec<HintingInstance>,
    size: Size,
}

impl<'a> Glyphs<'a> {
    /// 字身等於格高（介面上的中文字大小）。
    pub fn new(fonts: &'a [Font], cell_h: usize) -> Result<Self, String> {
        Self::with_px(fonts, (cell_h as f32 * EM_PER_CELL).round())
    }

    /// 指定字身像素（吉祥物標籤的作者名用較小的字）。
    pub fn with_px(fonts: &'a [Font], px: f32) -> Result<Self, String> {
        let size = Size::new(px);
        let refs: Vec<FontRef> = fonts.iter().map(|f| FontRef::new(&f.0).map_err(|e| e.to_string())).collect::<Result<_, _>>()?;
        let outlines: Vec<_> = refs.iter().map(|f| f.outline_glyphs()).collect();
        let hinting = outlines
            .iter()
            .map(|o| HintingInstance::new(o, size, LocationRef::default(), HintingOptions::default()).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        Ok(Glyphs { refs, outlines, hinting, size })
    }

    /// ch 的字寬（像素）；沒有字型能畫時回傳 None。
    pub fn advance(&self, ch: char) -> Option<f32> {
        self.refs.iter().find_map(|font| {
            let gid = font.charmap().map(ch).filter(|g| g.to_u32() != 0)?;
            font.glyph_metrics(self.size, LocationRef::default()).advance_width(gid)
        })
    }

    /// 把 ch 的覆蓋率畫進 box_w×box_h 的遮罩（先清空）：水平依字寬置中，垂直讓「上緣到下緣」的中線落在方框中線再往下移 dy 像素
    /// （負數往上）。沒有字型能畫時回傳 false。
    pub fn draw(&self, ch: char, box_w: usize, box_h: usize, dy: f32, mask: &mut [u8]) -> Result<bool, String> {
        let location = LocationRef::default();
        let found = self.refs.iter().enumerate().find_map(|(f, font)| {
            let gid = font.charmap().map(ch).filter(|g| g.to_u32() != 0)?;
            Some((f, gid, self.outlines[f].get(gid)?))
        });
        let Some((f, gid, glyph)) = found else { return Ok(false) };
        let metrics = self.refs[f].metrics(self.size, location);
        let advance = self.refs[f].glyph_metrics(self.size, location).advance_width(gid).unwrap_or(self.size.ppem().unwrap_or(0.0));
        let mut pen = Pen {
            path: Vec::new(),
            x: ((box_w as f32 - advance) / 2.0).round(),
            y: (box_h as f32 / 2.0 + (metrics.ascent + metrics.descent) / 2.0 + dy).round(),
        };
        glyph.draw(DrawSettings::hinted(&self.hinting[f], false), &mut pen).map_err(|e| format!("{ch}: {e}"))?;
        mask[..box_w * box_h].fill(0);
        Mask::new(&pen.path).size(box_w as u32, box_h as u32).render_into(mask, None);
        Ok(true)
    }
}

/// 錨定貼圖標籤（出發準備畫面的分頁，見 hooks.rs finish_label）的字身與上提：遊戲把貼圖照原尺寸從固定的上緣畫起，
/// 英文字型表的字（8×12）大寫字母約 10 px；中文用一般大小（約 13 px）會壓到分頁下緣，改用小一號、靠上。
const LABEL_EM_PER_CELL: f32 = 0.75;
const LABEL_RAISE_PER_CELL: f32 = 2.0 / 16.0;

/// 字圖集：前 2n 張是每字左右兩格；之後每字四張（2n + 4i 起：上列左、上列右、下列左、下列右）是同一個字往下位移半格、
/// 跨在兩列中間的樣子，遊戲畫半格位移的文字時，上列格子標 TOP_OF_TEXT、下列標 BOTTOM_OF_TEXT（見 hooks.rs paint）；
/// 最後每字兩張（6n + 2i 起）是錨定貼圖標籤用的小一號字。
pub fn render(fonts: &[Font], chars: &[char], cell_w: usize, cell_h: usize, out: &Path) -> Result<Report, String> {
    let n = chars.len();
    let tiles = n * 8;
    let width = TILES_PER_ROW.min(tiles.max(2)) * cell_w;
    let height = tiles.div_ceil(TILES_PER_ROW).max(1) * cell_h;
    let mut rgba = vec![0u8; width * height * 4];
    let alpha: Vec<u8> = (0..=255u32).map(|a| ((a as f32 / 255.0).powf(ALPHA_EXPONENT) * 255.0).round() as u8).collect();
    let glyphs = Glyphs::new(fonts, cell_h)?;
    let small = Glyphs::with_px(fonts, (cell_h as f32 * LABEL_EM_PER_CELL).round())?;
    let box_w = cell_w * 2;
    let dy = -(cell_h as f32 * RAISE_PER_CELL).round();
    let label_dy = -(cell_h as f32 * LABEL_RAISE_PER_CELL).round();
    // 遮罩 (sx, sy) 起一格大小的區塊寫進第 tile 張貼圖
    let mut put = |tile: usize, mask: &[u8], sx: usize, sy: usize| {
        let (tx, ty) = (tile % TILES_PER_ROW * cell_w, tile / TILES_PER_ROW * cell_h);
        for y in 0..cell_h {
            for x in 0..cell_w {
                let o = ((ty + y) * width + tx + x) * 4;
                rgba[o..o + 4].copy_from_slice(&[255, 255, 255, alpha[mask[(sy + y) * box_w + sx + x] as usize]]);
            }
        }
    };

    let mut mask = vec![0u8; box_w * cell_h * 2];
    let mut missing = Vec::new();
    for (i, &ch) in chars.iter().enumerate() {
        if !glyphs.draw(ch, box_w, cell_h, dy, &mut mask)? {
            missing.push(ch);
            continue;
        }
        for half in 0..2 {
            put(2 * i + half, &mask, half * cell_w, 0);
        }
        // 兩列高的方框置中＝一般位置往下移半格
        glyphs.draw(ch, box_w, cell_h * 2, dy, &mut mask)?;
        for row in 0..2 {
            for half in 0..2 {
                put(2 * n + 4 * i + 2 * row + half, &mask, half * cell_w, row * cell_h);
            }
        }
        small.draw(ch, box_w, cell_h, label_dy, &mut mask)?;
        for half in 0..2 {
            put(6 * n + 2 * i + half, &mask, half * cell_w, 0);
        }
    }

    crate::image::write_png(out, width, height, &rgba)?;
    Ok(Report { tiles, missing })
}
