//! 字典：英文原文（遊戲內的 CP437 位元組）→ 譯文。
//!
//! 譯文在載入時就換算成「替身字串」：CP437 裡有的字照抄，其他字（中文、全形標點）各佔兩格，
//! 以兩個空白佔位，並記下這兩格要貼哪個字圖。遊戲照常把替身字串寫進畫面，hook 再把空白格換成字圖。
//!
//! 句型規則處理遊戲執行時拼出來的句子：原文含 {1}～{9} 佔位，整句比對；佔位抓到的片段先查整句字典、
//! 再遞迴套規則，都沒有就照抄英文。結果不能比原文寬，換算成替身字串後存進快取，下一幀直接用。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::cp437;

/// 句型結果快取的上限，滿了整個清空。ponytail: 全清最簡單；畫面上同時出現的句子遠少於此，命中率有問題再換 LRU
const CACHE_LIMIT: usize = 50_000;
/// 佔位片段遞迴套規則的層數上限
const MAX_DEPTH: usize = 3;

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

/// 佔位能收的片段：任意、只收數字（`{#1}`）、只收字典裡有整句條目的字詞（`{@1}`，例如月份名）。
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Any,
    Number,
    Term,
}

enum Seg {
    Lit(Vec<u8>),
    Cap(usize, Kind),
}

enum Out {
    Text(String),
    Cap(usize),
}

pub struct Rule {
    pattern: Vec<Seg>,
    output: Vec<Out>,
    align: String,
    /// 樣板本身有中文；沒有的話（例如只換標點的外框），至少要有一個佔位翻得出來才算命中
    has_cjk: bool,
    /// 開頭、結尾的固定字，先用來快速排除
    prefix: Vec<u8>,
    suffix: Vec<u8>,
}

/// 一個佔位：編號與能收的片段。
struct Placeholder(usize, Kind);

/// 拆出 `{1}`～`{9}`、`{#1}`～`{#9}`、`{@1}`～`{@9}`；其餘依 `text` 轉換（原文轉 CP437 位元組、譯文保留字串）。
fn split_placeholders<T>(s: &str, mut text: impl FnMut(&str) -> Option<T>) -> Option<Vec<Result<T, Placeholder>>> {
    let mut parts = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        let b = rest.as_bytes();
        let kind = match b.get(i + 1) {
            Some(b'#') => Kind::Number,
            Some(b'@') => Kind::Term,
            _ => Kind::Any,
        };
        let d = i + 1 + usize::from(kind != Kind::Any);
        if d + 1 < b.len() && (b'1'..=b'9').contains(&b[d]) && b[d + 1] == b'}' {
            if i > 0 {
                parts.push(Ok(text(&rest[..i])?));
            }
            parts.push(Err(Placeholder((b[d] - b'1') as usize, kind)));
            rest = &rest[d + 2..];
        } else {
            parts.push(Ok(text(&rest[..=i])?));
            rest = &rest[i + 1..];
        }
    }
    if !rest.is_empty() {
        parts.push(Ok(text(rest)?));
    }
    Some(parts)
}

/// 數字佔位能接受的片段：可帶負號、千分位逗號的整數。
fn is_number(piece: &[u8]) -> bool {
    let digits = piece.strip_prefix(b"-").unwrap_or(piece);
    !digits.is_empty() && digits[0].is_ascii_digit() && digits.iter().all(|c| c.is_ascii_digit() || *c == b',')
}

impl Rule {
    /// 原文樣板必須能轉成 CP437、每個佔位只出現一次、佔位不能相鄰（相鄰時切點不唯一）；
    /// 譯文用到的佔位要和原文一致（譯文裡的 `#`、`@` 寫不寫都可以）。
    fn parse(en: &str, zh: &str, align: &str) -> Option<Rule> {
        let mut pattern: Vec<Seg> = Vec::new();
        for part in split_placeholders(en, cp437::encode)? {
            match (part, pattern.last_mut()) {
                (Ok(bytes), Some(Seg::Lit(lit))) => lit.extend(bytes),
                (Ok(bytes), _) => pattern.push(Seg::Lit(bytes)),
                (Err(_), Some(Seg::Cap(..))) => return None,
                (Err(Placeholder(i, kind)), _) => pattern.push(Seg::Cap(i, kind)),
            }
        }
        let output: Vec<Out> = split_placeholders(zh, |t| Some(t.to_owned()))?
            .into_iter()
            .map(|p| p.map_or_else(|Placeholder(i, _)| Out::Cap(i), Out::Text))
            .collect();
        let caps = |it: &mut dyn Iterator<Item = usize>| {
            let mut v: Vec<usize> = it.collect();
            v.sort_unstable();
            v
        };
        let en_caps = caps(&mut pattern.iter().filter_map(|s| if let Seg::Cap(i, _) = s { Some(*i) } else { None }));
        let zh_caps = caps(&mut output.iter().filter_map(|o| if let Out::Cap(i) = o { Some(*i) } else { None }));
        if en_caps.is_empty() || en_caps != zh_caps || en_caps.windows(2).any(|w| w[0] == w[1]) {
            return None;
        }
        let lit = |s: Option<&Seg>| match s {
            Some(Seg::Lit(l)) => l.clone(),
            _ => Vec::new(),
        };
        Some(Rule {
            prefix: lit(pattern.first()),
            suffix: lit(pattern.last()),
            has_cjk: output.iter().any(|o| matches!(o, Out::Text(t) if t.chars().any(|c| cp437::byte_of(c).is_none()))),
            pattern,
            output,
            align: align.to_owned(),
        })
    }

    /// 整句比對；佔位取最短的可行片段（至少一個字）。成功時回傳每個佔位的 (起, 迄)。`terms` 給 `{@n}` 查字詞。
    fn matches(&self, text: &[u8], terms: &HashMap<Vec<u8>, Entry>) -> Option<[(usize, usize); 9]> {
        if text.len() < self.prefix.len() + self.suffix.len() || !text.starts_with(&self.prefix) || !text.ends_with(&self.suffix) {
            return None;
        }
        let mut caps = [(0, 0); 9];
        match_at(&self.pattern, text, 0, &mut caps, terms).then_some(caps)
    }
}

fn match_at(segs: &[Seg], text: &[u8], pos: usize, caps: &mut [(usize, usize); 9], terms: &HashMap<Vec<u8>, Entry>) -> bool {
    let Some((first, rest)) = segs.split_first() else { return pos == text.len() };
    match first {
        Seg::Lit(lit) => text[pos..].starts_with(lit) && match_at(rest, text, pos + lit.len(), caps, terms),
        Seg::Cap(i, kind) => {
            let fits = |end: usize| match kind {
                Kind::Any => true,
                Kind::Number => is_number(&text[pos..end]),
                Kind::Term => terms.contains_key(&text[pos..end]),
            };
            match rest.first() {
                None => {
                    caps[*i] = (pos, text.len());
                    pos < text.len() && fits(text.len())
                }
                Some(Seg::Lit(next)) => {
                    let mut end = pos + 1;
                    while end + next.len() <= text.len() {
                        if text[end..].starts_with(next) && fits(end) {
                            caps[*i] = (pos, end);
                            if match_at(rest, text, end, caps, terms) {
                                return true;
                            }
                        }
                        end += 1;
                    }
                    false
                }
                Some(Seg::Cap(..)) => false,
            }
        }
    }
}

#[derive(Default)]
pub struct Dict {
    pub entries: HashMap<Vec<u8>, Entry>,
    pub rules: Vec<Rule>,
    /// 原文 → 句型規則的結果（None 表示沒有規則命中，也要記住，免得每幀重比）
    cache: Mutex<HashMap<Vec<u8>, Option<Arc<Entry>>>>,
    /// 段落換行後的每一行：文字 → 代號、代號 → 替身字串（見 mtb.rs）
    lines: Mutex<Lines>,
    /// 字 → 字圖編號；字圖 i 的左右半格在 texpos[2i]、texpos[2i+1]。
    pub glyph_of: HashMap<char, u32>,
    pub texpos: Vec<i32>,
}

#[derive(Default)]
struct Lines {
    key_of: HashMap<String, Vec<u8>>,
    entries: HashMap<Vec<u8>, Arc<Entry>>,
}

pub struct LoadReport {
    pub loaded: usize,
    /// 原文含 CP437 以外的字（遊戲不可能顯示這種字串），或規則的佔位寫法不合規定，略過。
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

    /// 段落的一行中文換成代號（KEY_PREFIX + 六位數），畫字 hook 看到代號就畫這一行；缺字圖時回傳 None。
    pub fn line_key(&self, text: &str) -> Option<Vec<u8>> {
        let mut lines = self.lines.lock();
        if let Some(key) = lines.key_of.get(text) {
            return Some(key.clone());
        }
        let (entry, missing) = self.entry(0, text, "");
        if missing > 0 {
            return None;
        }
        let mut key = crate::mtb::KEY_PREFIX.to_vec();
        key.extend(format!("{:06}", lines.entries.len()).bytes());
        lines.entries.insert(key.clone(), Arc::new(entry));
        lines.key_of.insert(text.to_owned(), key.clone());
        Some(key)
    }

    pub fn line_entry(&self, key: &[u8]) -> Option<Arc<Entry>> {
        self.lines.lock().entries.get(key).cloned()
    }

    /// `align` 是呼叫端對原文的對齊方式：遊戲先用原文長度算好起點再寫字，
    /// 置中（"c"）或靠右（"r"）的譯文較短時，前面補空白，讓譯文落在原文同一個中心或右緣。
    /// 置中的後面也補滿到原文寬度：有底色的文字（標題副標題的黑底）兩側才會一樣寬。
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
        if align == "c" {
            shadow.resize(en_cells.max(shadow.len()), b' ');
        }
        shadow.push(0);
        (Entry { shadow, glyphs, text: text.to_owned() }, missing)
    }

    /// 讀 `原文<TAB>譯文[<TAB>對齊]` 一行一條的 UTF-8 檔；空行與 # 開頭的行略過。
    fn read_tsv(content: &str) -> impl Iterator<Item = (&str, &str, &str)> {
        content.lines().filter_map(|line| {
            let mut cols = line.split('\t');
            let (en, zh) = (cols.next()?, cols.next()?);
            (!en.is_empty() && !en.starts_with('#')).then_some((en, zh, cols.next().unwrap_or("")))
        })
    }

    pub fn load_tsv(&mut self, content: &str) -> LoadReport {
        let mut report = LoadReport { loaded: 0, bad_source: 0, missing_glyphs: 0 };
        for (en, zh, align) in Self::read_tsv(content) {
            let Some(key) = cp437::encode(en) else {
                report.bad_source += 1;
                continue;
            };
            let (entry, missing) = self.entry(key.len(), zh, align);
            report.missing_glyphs += missing;
            self.entries.insert(key, entry);
            report.loaded += 1;
        }
        report
    }

    /// 句型規則檔，格式同 load_tsv；依檔案順序比對，先寫的優先。
    pub fn load_rules_tsv(&mut self, content: &str) -> LoadReport {
        let mut report = LoadReport { loaded: 0, bad_source: 0, missing_glyphs: 0 };
        for (en, zh, align) in Self::read_tsv(content) {
            let Some(rule) = Rule::parse(en, zh, align) else {
                report.bad_source += 1;
                continue;
            };
            report.missing_glyphs += rule
                .output
                .iter()
                .filter_map(|o| if let Out::Text(t) = o { Some(t) } else { None })
                .flat_map(|t| t.chars())
                .filter(|c| cp437::byte_of(*c).is_none() && !self.glyph_of.contains_key(c))
                .count();
            self.rules.push(rule);
            report.loaded += 1;
        }
        self.cache.lock().clear();
        report
    }

    /// 一段原文的譯文（不含對齊）：整句字典優先，再套句型規則。
    pub fn translate_text(&self, key: &[u8]) -> Option<String> {
        self.entries.get(key).map(|e| e.text.clone()).or_else(|| self.apply_rules(key, 0).map(|(t, _)| t))
    }

    fn apply_rules(&self, key: &[u8], depth: usize) -> Option<(String, &Rule)> {
        for rule in &self.rules {
            let Some(caps) = rule.matches(key, &self.entries) else { continue };
            let mut out = String::new();
            let mut translated = false;
            for part in &rule.output {
                match part {
                    Out::Text(t) => out.push_str(t),
                    Out::Cap(i) => {
                        let piece = &key[caps[*i].0..caps[*i].1];
                        let sub = self.entries.get(piece).map(|e| e.text.clone()).or_else(|| {
                            (depth < MAX_DEPTH).then(|| self.apply_rules(piece, depth + 1).map(|(t, _)| t)).flatten()
                        });
                        translated |= sub.is_some();
                        out.push_str(&sub.unwrap_or_else(|| cp437::decode(piece)));
                    }
                }
            }
            if rule.has_cjk || translated {
                return Some((out, rule));
            }
        }
        None
    }

    /// hook 用：整句字典沒有時，套句型規則並換算成替身字串。譯文比原文寬或缺字圖時放棄，照畫英文。
    pub fn rule_entry(&self, key: &[u8]) -> Option<Arc<Entry>> {
        if self.rules.is_empty() {
            return None;
        }
        let mut cache = self.cache.lock();
        if let Some(hit) = cache.get(key) {
            return hit.clone();
        }
        let entry = self.apply_rules(key, 0).and_then(|(text, rule)| {
            let (entry, missing) = self.entry(key.len(), &text, &rule.align);
            (missing == 0 && entry.shadow().len() <= key.len()).then(|| Arc::new(entry))
        });
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key.to_vec(), entry.clone());
        entry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(rules: &str) -> Dict {
        let mut d = Dict::default();
        let chars = "貪婪勞動與的歷史來訪了。很長";
        assert!(d.set_glyphs(chars, vec![0; chars.chars().count() * 2]));
        d.load_tsv("Cupidity\t貪婪\nLabor\t勞動\n");
        d.load_rules_tsv(rules);
        d
    }

    #[test]
    fn reorders_placeholders_and_translates_captures() {
        let d = dict("Histories of {1} and {2}\t{1}與{2}的歷史\tc\n");
        assert_eq!(d.translate_text(b"Histories of Cupidity and Labor").as_deref(), Some("貪婪與勞動的歷史"));
        assert_eq!(d.translate_text(b"Histories of Greed and Labor").as_deref(), Some("Greed與勞動的歷史"));
        assert_eq!(d.translate_text(b"Histories of Labor"), None);
    }

    #[test]
    fn earlier_capture_takes_the_shortest_piece() {
        let d = dict("{1} and {2}\t{1}與{2}\n");
        assert_eq!(d.translate_text(b"Labor and Cupidity and Greed").as_deref(), Some("勞動與貪婪與Greed"));
    }

    #[test]
    fn wrapper_without_chinese_needs_a_translated_capture() {
        let d = dict("-{1}-\t-{1}-\n");
        assert_eq!(d.translate_text(b"-Cupidity-").as_deref(), Some("-貪婪-"));
        assert_eq!(d.translate_text(b"-Greed-"), None);
    }

    #[test]
    fn captures_apply_rules_recursively() {
        let d = dict("-{1}-\t-{1}-\n{1} is visiting.\t{1}來訪了。\n");
        assert_eq!(d.translate_text(b"-Urist is visiting.-").as_deref(), Some("-Urist來訪了。-"));
    }

    #[test]
    fn hook_falls_back_to_english_when_wider_than_source() {
        let d = dict("x{1}\t{1}很長很長很長\n{1} is visiting.\t{1}來訪了。\tc\n");
        assert!(d.rule_entry(b"xA").is_none());
        let e = d.rule_entry(b"Urist is visiting.").expect("fits");
        assert_eq!(e.shadow().len(), 18, "置中時替身字串補滿到原文寬度");
        assert_eq!(e.glyphs[0].0, 2 + 5, "左邊補 (18 - 13) / 2 = 2 格，再接 Urist");
        assert!(d.rule_entry(b"Urist is visiting.").is_some(), "第二次走快取");
    }

    #[test]
    fn numeric_placeholder_only_takes_numbers() {
        let d = dict("{#1}th {2}\t{2}{1}日\n{1} pts\t{1} 點\n");
        assert_eq!(d.translate_text(b"15th Labor").as_deref(), Some("勞動15日"));
        assert_eq!(d.translate_text(b"1,250th Labor").as_deref(), Some("勞動1,250日"));
        assert_eq!(d.translate_text(b"fraught with many perils"), None, "with 的 th 前面不是數字");
        assert_eq!(d.translate_text(b"14 pts").as_deref(), Some("14 點"));
    }

    #[test]
    fn term_placeholder_only_takes_dictionary_words() {
        let d = dict("{#1}st {@2}\t{2}{1}日\n");
        assert_eq!(d.translate_text(b"1st Labor").as_deref(), Some("勞動1日"));
        assert_eq!(d.translate_text(b"1st Labor, early spring, 5"), None, "不能把後面整串當成月份名");
    }

    #[test]
    fn rejects_bad_placeholder_layouts() {
        let r = dict("").load_rules_tsv("{1}{2}\t{1}{2}\n{1} x\t{2}\n{1} y\t{1}{1}\nno placeholder\t沒有\n");
        assert_eq!((r.loaded, r.bad_source), (0, 4));
    }
}
