//! 字典：英文原文（遊戲內的 CP437 位元組）→ 譯文。
//!
//! 譯文在載入時就換算成「替身字串」：CP437 裡有的字照抄，其他字（中文、全形標點）各佔兩格，
//! 以兩個空白佔位，並記下這兩格要貼哪個字圖。遊戲照常把替身字串寫進畫面，hook 再把空白格換成字圖。

use std::collections::HashMap;

use crate::cp437;

pub struct Entry {
    /// 替身字串（CP437）。結尾多一個 NUL：MSVC std::string 長字串模式要求緩衝區以 NUL 結尾。
    shadow: Vec<u8>,
    /// (替身字串內的格位, 字圖編號)；每個字圖佔 offset 與 offset+1 兩格。
    pub glyphs: Vec<(usize, u32)>,
    pub text: String,
}

impl Entry {
    pub fn shadow(&self) -> &[u8] {
        &self.shadow[..self.shadow.len() - 1]
    }

    /// 遊戲以 `space` 截斷字串時（space > 0），不要切到半個中文字：回傳可用的格數與字圖數。
    pub fn fit(&self, space: i32) -> (usize, usize) {
        let full = self.shadow().len();
        if space <= 0 || full <= space as usize {
            return (full, self.glyphs.len());
        }
        let mut cut = space as usize;
        let mut n = 0;
        for &(off, _) in &self.glyphs {
            if off + 2 > cut {
                cut = cut.min(off);
                break;
            }
            n += 1;
        }
        (cut, n)
    }
}

#[derive(Default)]
pub struct Dict {
    pub entries: HashMap<Vec<u8>, Entry>,
    /// 字 → 字圖編號；字圖 i 的左右半格在 texpos[2i]、texpos[2i+1]。
    pub glyph_of: HashMap<char, u32>,
    pub texpos: Vec<i32>,
}

pub struct LoadReport {
    pub loaded: usize,
    /// 原文含 CP437 以外的字，遊戲不可能顯示這種字串，略過。
    pub bad_source: usize,
    /// 譯文用到字圖集沒有的字，以 '?' 代替；verify 應該在建置時就擋下。
    pub missing_glyphs: usize,
}

impl Dict {
    pub fn set_glyphs(&mut self, chars: &str, texpos: Vec<i32>) -> bool {
        let chars: Vec<char> = chars.chars().collect();
        if texpos.len() != chars.len() * 2 {
            return false;
        }
        self.glyph_of = chars.into_iter().enumerate().map(|(i, c)| (c, i as u32)).collect();
        self.texpos = texpos;
        true
    }

    /// `align` 是呼叫端對原文的對齊方式：遊戲先用原文長度算好起點再寫字，
    /// 置中（"c"）或靠右（"r"）的譯文較短時，前面補空白，讓譯文落在原文同一個中心或右緣。
    fn entry(&self, en_cells: usize, text: &str, align: &str) -> (Entry, usize) {
        let mut shadow = Vec::with_capacity(text.len() + 1);
        let mut glyphs = Vec::new();
        let mut missing = 0;
        for c in text.chars() {
            if let Some(b) = cp437::byte_of(c) {
                shadow.push(b);
            } else if let Some(&g) = self.glyph_of.get(&c) {
                glyphs.push((shadow.len(), g));
                shadow.extend_from_slice(b"  ");
            } else {
                shadow.push(b'?');
                missing += 1;
            }
        }
        let spare = en_cells.saturating_sub(shadow.len());
        let pad = match align {
            "c" => spare / 2,
            "r" => spare,
            _ => 0,
        };
        if pad > 0 {
            shadow.splice(0..0, std::iter::repeat_n(b' ', pad));
            glyphs.iter_mut().for_each(|g| g.0 += pad);
        }
        shadow.push(0);
        (Entry { shadow, glyphs, text: text.to_owned() }, missing)
    }

    /// 讀 `原文<TAB>譯文[<TAB>對齊]` 一行一條的 UTF-8 檔；空行與 # 開頭的行略過。
    pub fn load_tsv(&mut self, content: &str) -> LoadReport {
        let mut report = LoadReport { loaded: 0, bad_source: 0, missing_glyphs: 0 };
        for line in content.lines() {
            let mut cols = line.split('\t');
            let (Some(en), Some(zh)) = (cols.next(), cols.next()) else { continue };
            if en.is_empty() || en.starts_with('#') {
                continue;
            }
            let Some(key) = cp437::encode(en) else {
                report.bad_source += 1;
                continue;
            };
            let (entry, missing) = self.entry(key.len(), zh, cols.next().unwrap_or(""));
            report.missing_glyphs += missing;
            self.entries.insert(key, entry);
            report.loaded += 1;
        }
        report
    }
}
