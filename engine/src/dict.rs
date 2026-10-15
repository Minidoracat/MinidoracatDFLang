//! 字典：英文原文（遊戲內的 CP437 位元組）→ 譯文。
//!
//! 譯文在載入時就換算成「替身字串」：CP437 裡有的字照抄，其他字（中文、全形標點）各佔兩格，
//! 以兩個空白佔位，並記下這兩格要貼哪個字圖。遊戲照常把替身字串寫進畫面，hook 再把空白格換成字圖。
//!
//! 句型規則處理遊戲執行時拼出來的句子：原文含 {1}～{9} 佔位，整句比對；佔位抓到的片段先查整句字典、
//! 再遞迴套規則，都沒有就照抄英文。結果不能比原文寬，換算成替身字串後存進快取，下一幀直接用。
//!
//! 人物頁、訪客說明這類遊戲拼成一整段的文字句與句之間空兩格：整段查不到時逐句查（translate_sentences）。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::cp437;

/// 句型結果快取的上限，滿了整個清空。ponytail: 全清最簡單；畫面上同時出現的句子遠少於此，命中率有問題再換 LRU
const CACHE_LIMIT: usize = 50_000;
/// 佔位片段遞迴套規則的層數上限：只防堆疊過深。清單句型（`{%1}, {%2}`）每一項遞迴一層，價值觀的清單最長三十多項
const MAX_DEPTH: usize = 64;

/// 譯文片段：(文字, 原文位置)。句型的固定字記比對範圍的開頭，佔位的譯文記佔位片段的開頭（遞迴下去也一樣），
/// 彩色文字框依原文那個位置的顏色替譯文上色（見 ctb.rs）。
pub type Pieces = Vec<(String, usize)>;

/// 片段的譯文與「乾淨」與否。乾淨：整句字典的條目，或句型本身有中文字或英文字，或每個佔位都乾淨地翻出來。
/// `{1}, {@2}` 這種只有標點的外框把 `{1}` 照抄英文時不乾淨；`{%n}` 只收乾淨的，清單裡翻不出來的一項才不會變成半中半英。
type Found = (Pieces, bool);

/// 一次翻譯裡算過的片段：原文片段 → 譯文片段（位置從片段開頭算）。
/// 清單句型的佔位會回頭試很多種切法，同一段反覆出現，不記住會變成指數時間。
type Memo = HashMap<Vec<u8>, Option<Found>>;

fn concat(pieces: Pieces) -> String {
    pieces.into_iter().map(|(t, _)| t).collect()
}

#[derive(Clone)]
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

/// 佔位能收的片段：任意、只收數字（`{#1}`）、只收字典裡有整句條目的字詞（`{@1}`，例如月份名）、
/// 只收翻得出來的片段（`{%1}`：整句字典或句型都行，翻不出來這條句型就不算命中；子句、清單用，免得半中半英）。
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Any,
    Number,
    Term,
    Full,
    /// `{~n}`：比對同 `{n}`，譯文可以不寫（「伐木工用伐木工的右前臂…」的第二個主人）
    Skip,
}

enum Seg {
    Lit(Vec<u8>),
    Cap(usize, Kind),
}

/// 譯文的一段：固定字，或佔位（編號, 補字）。補字見 `{n+時}`。
enum Out {
    Text(String),
    Cap(usize, String),
}

/// 時間、原因助詞：佔位譯文已經以這些字結尾時，`{n+時}` 不再補字（介系詞片語「經歷創傷後」「在衝突中」「因…而」本身就帶）。
const PARTICLES: &str = "時时後后中而間间前際际近地";
/// `{n+色}`：顏色當述語時補「色」（金→金色、深黃褐→深黃褐色），譯文已經以「色」結尾就不補。
const COLOR: &str = "色";

pub struct Rule {
    pattern: Vec<Seg>,
    output: Vec<Out>,
    align: String,
    /// 樣板本身有中文字（全形標點不算）；沒有的話（只換標點的外框 `（{1}）`、`{1}，♂`），至少要有一個佔位翻得出來才算命中
    has_cjk: bool,
    /// 開頭、結尾的固定字，先用來快速排除
    prefix: Vec<u8>,
    suffix: Vec<u8>,
    /// 樣板的固定字含英文字母。沒有英文字的樣板（`{1}, {@2}`、`{@1} {@2}`）能套的字串太多，排在最後比對
    specific: bool,
    /// 清單句型：佔位全是 `{%n}`、固定字沒有英文字（`{%1}, {%2}`）。每一項都要翻得出來才命中，所以排在最前面：
    /// 項目句型若尾端是任意佔位（`to pray at {1}`），先比就會把後面的「, 下一項」也吞進去。只用在佔位裡，不套整句
    list: bool,
    /// 開頭就是任意佔位（`{1} was given away`）：開頭沒有限制，同一類裡排在後面，
    /// 免得把外框句的前半（「He feels sad after 神器名」）整個吞進 {1}
    loose: bool,
    /// 只用在佔位裡（align 欄寫 `inner`）：冠詞外框 `The {%1}` 套在整句上，會把「The dwarf is singing well.」去掉 The
    /// 之後拿剩下的半句去比別的句型，比出不相干的譯文；放在佔位裡（「{%1} misses {%2}!」的 The vulture）才安全。
    inner: bool,
}

/// 一個佔位：編號、能收的片段、譯文裡的補字（`{3+時}`，原文不能有）。
struct Placeholder(usize, Kind, String);

/// 拆出 `{1}`～`{9}`、`{#1}`～`{#9}`、`{@1}`～`{@9}`、`{%1}`～`{%9}`，以及帶補字的 `{1+時}`；
/// 其餘依 `text` 轉換（原文轉 CP437 位元組、譯文保留字串）。
fn split_placeholders<T>(s: &str, mut text: impl FnMut(&str) -> Option<T>) -> Option<Vec<Result<T, Placeholder>>> {
    let mut parts = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find('{') {
        let b = rest.as_bytes();
        let kind = match b.get(i + 1) {
            Some(b'#') => Kind::Number,
            Some(b'@') => Kind::Term,
            Some(b'%') => Kind::Full,
            Some(b'~') => Kind::Skip,
            _ => Kind::Any,
        };
        let d = i + 1 + usize::from(kind != Kind::Any);
        let close = match (b.get(d), b.get(d + 1)) {
            (Some(b'1'..=b'9'), Some(b'}')) => Some((d + 1, "")),
            (Some(b'1'..=b'9'), Some(b'+')) => rest[d + 2..].find('}').map(|k| (d + 2 + k, &rest[d + 2..d + 2 + k])),
            _ => None,
        };
        match close.filter(|(_, extra)| !extra.contains('{')) {
            Some((end, extra)) => {
                if i > 0 {
                    parts.push(Ok(text(&rest[..i])?));
                }
                parts.push(Err(Placeholder((b[d] - b'1') as usize, kind, extra.to_owned())));
                rest = &rest[end + 1..];
            }
            None => {
                parts.push(Ok(text(&rest[..=i])?));
                rest = &rest[i + 1..];
            }
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

/// 段落框裡可以點的連結（傳說頁面、物品說明的人名、地名、作品名）在原文裡換成記號 `\x01序號\x02`（CP437 的 ☺、☻），
/// 名字本身不給句型看：名字常含 of、the、and，句型的固定字會切進名字裡。記號和沒有連結的名字一樣翻不出來：
/// `{n}` 照抄、`{%n}` 不收（收了的話，給片語用的 `{%n}` 會吞下單獨的名字，搶走專寫的句型），排版時再換回遊戲的連結字（見 mtb.rs）。
pub const LINK_OPEN: u8 = 1;
pub const LINK_CLOSE: u8 = 2;

/// 譯文裡的連結記號依出現順序是第幾個連結；n 個連結都要恰好出現一次，否則 None（句型漏掉或重複了名字）。
pub fn link_order(translation: &str, n: usize) -> Option<Vec<usize>> {
    let (open, close) = (cp437::TABLE[LINK_OPEN as usize], cp437::TABLE[LINK_CLOSE as usize]);
    let mut order = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    for (i, _) in translation.match_indices(open) {
        let rest = &translation[i + open.len_utf8()..];
        let k: usize = rest[..rest.find(close)?].parse().ok()?;
        if std::mem::replace(seen.get_mut(k)?, true) {
            return None;
        }
        order.push(k);
    }
    (order.len() == n).then_some(order)
}

/// 整句字典查詞；查不到時改查大小寫不同的寫法：遊戲常把名稱放句首時改大寫（`Cats, ♂`），動物的稱呼還會
/// 每個字大寫（`Stray One-humped Camel`），raws 裡只有小寫；公告又常把介面上大寫的名稱寫成小寫
/// （`the woodcutter` 對到職業 `Woodcutter`、`Trade Depot` 對到建造選單的 `Trade depot`）。
/// 全大寫的字串（`PAUSED`）不試；名稱都短，超過 64 位元組的不試，免得配置記憶體。
fn lookup_name<'a>(d: &'a Dict, key: &[u8]) -> Option<&'a Entry> {
    d.entries.get(key).or_else(|| {
        if key.len() > 64 || !key.iter().any(u8::is_ascii_lowercase) {
            return None;
        }
        let mut buf = [0u8; 64];
        let lower = &mut buf[..key.len()];
        lower.copy_from_slice(key);
        lower.make_ascii_lowercase();
        let lower = &*lower;
        key.iter().any(u8::is_ascii_uppercase).then(|| d.entries.get(lower)).flatten()
            .or_else(|| d.folded.get(lower).and_then(|k| d.entries.get(k)))
    })
}

/// 佔位裡的查詞：先照 lookup_name，再查只在佔位裡用的詞（字串檔 align 寫 inner；含大寫時退回全小寫）。
fn lookup_inner<'a>(d: &'a Dict, key: &[u8]) -> Option<&'a Entry> {
    lookup_name(d, key).or_else(|| d.inner.get(key)).or_else(|| {
        if key.len() > 64 || !key.iter().any(u8::is_ascii_uppercase) || !key.iter().any(u8::is_ascii_lowercase) {
            return None;
        }
        let mut buf = [0u8; 64];
        let lower = &mut buf[..key.len()];
        lower.copy_from_slice(key);
        lower.make_ascii_lowercase();
        d.inner.get(&*lower)
    })
}

impl Rule {
    /// 原文樣板必須能轉成 CP437、每個佔位只出現一次、佔位不能相鄰（相鄰時切點不唯一），也不能帶補字；
    /// 譯文每個佔位最多用一次，而且都要是原文有的；原文的佔位除了 `{@n}`、`{~n}` 都要用到（譯文裡的 `#`、`@`、`%`、`~` 寫不寫都可以）。
    /// `{@n}` 可以不寫：中文常省略重複的主詞（「他深受感動，卻因不喜歡大自然而困擾」不必再寫一次「他」）。
    /// 譯文的 `{n+時}`：佔位譯文後面補「時」，但譯文已經以時間、原因助詞（PARTICLES）結尾就不補；`{n+色}` 已經以「色」結尾就不補。
    fn parse(en: &str, zh: &str, align: &str) -> Option<Rule> {
        let mut pattern: Vec<Seg> = Vec::new();
        for part in split_placeholders(en, cp437::encode)? {
            match (part, pattern.last_mut()) {
                (Ok(bytes), Some(Seg::Lit(lit))) => lit.extend(bytes),
                (Ok(bytes), _) => pattern.push(Seg::Lit(bytes)),
                (Err(_), Some(Seg::Cap(..))) => return None,
                (Err(Placeholder(_, _, extra)), _) if !extra.is_empty() => return None,
                (Err(Placeholder(i, kind, _)), _) => pattern.push(Seg::Cap(i, kind)),
            }
        }
        let output: Vec<Out> = split_placeholders(zh, |t| Some(t.to_owned()))?
            .into_iter()
            .map(|p| p.map_or_else(|Placeholder(i, _, extra)| Out::Cap(i, extra), Out::Text))
            .collect();
        let en_caps: Vec<(usize, Kind)> = pattern.iter().filter_map(|s| if let Seg::Cap(i, k) = s { Some((*i, *k)) } else { None }).collect();
        let zh_caps: Vec<usize> = output.iter().filter_map(|o| if let Out::Cap(i, _) = o { Some(*i) } else { None }).collect();
        let unique = |v: &[usize]| {
            let mut seen = [false; 9];
            v.iter().all(|&i| !std::mem::replace(&mut seen[i], true))
        };
        if en_caps.is_empty()
            || !unique(&en_caps.iter().map(|c| c.0).collect::<Vec<_>>())
            || !unique(&zh_caps)
            || !zh_caps.iter().all(|i| en_caps.iter().any(|c| c.0 == *i))
            || !en_caps.iter().all(|(i, k)| matches!(k, Kind::Term | Kind::Skip) || zh_caps.contains(i))
        {
            return None;
        }
        let lit = |s: Option<&Seg>| match s {
            Some(Seg::Lit(l)) => l.clone(),
            _ => Vec::new(),
        };
        Some(Rule {
            prefix: lit(pattern.first()),
            suffix: lit(pattern.last()),
            has_cjk: output.iter().any(|o| matches!(o, Out::Text(t) if t.chars().any(|c| c.is_alphabetic() && cp437::byte_of(c).is_none()))),
            specific: pattern.iter().any(|s| matches!(s, Seg::Lit(l) if l.iter().any(u8::is_ascii_alphabetic))),
            list: pattern.iter().all(|s| match s {
                Seg::Cap(_, kind) => *kind == Kind::Full,
                Seg::Lit(l) => !l.iter().any(u8::is_ascii_alphabetic),
            }),
            loose: matches!(pattern.first(), Some(Seg::Cap(_, Kind::Any | Kind::Skip))),
            inner: align == "inner",
            pattern,
            output,
            align: align.to_owned(),
        })
    }

    /// 整句比對；佔位取最短的可行片段（至少一個字）。成功時回傳每個佔位的 (起, 迄)。
    /// `fits` 判斷限定佔位（`{#n}`、`{@n}`、`{%n}`）能不能收這段。
    fn matches(&self, text: &[u8], fits: &mut dyn FnMut(Kind, &[u8]) -> bool) -> Option<[(usize, usize); 9]> {
        if text.len() < self.prefix.len() + self.suffix.len() || !text.starts_with(&self.prefix) || !text.ends_with(&self.suffix) {
            return None;
        }
        let mut caps = [(0, 0); 9];
        match_at(&self.pattern, text, 0, &mut caps, fits).then_some(caps)
    }
}

fn match_at(segs: &[Seg], text: &[u8], pos: usize, caps: &mut [(usize, usize); 9], fits: &mut dyn FnMut(Kind, &[u8]) -> bool) -> bool {
    let Some((first, rest)) = segs.split_first() else { return pos == text.len() };
    match first {
        Seg::Lit(lit) => text[pos..].starts_with(lit) && match_at(rest, text, pos + lit.len(), caps, fits),
        Seg::Cap(i, kind) => {
            // 佔位不跨句：抓的片段不能含句界（人名、清單項目都不會跨句；跨了就是把下一句也吞進來）
            let stop = (pos + 1..text.len()).find(|&k| is_boundary(text, k) || is_box_break(text, k)).unwrap_or(text.len());
            match rest.first() {
                None => {
                    caps[*i] = (pos, text.len());
                    pos < stop && stop == text.len() && (matches!(kind, Kind::Any | Kind::Skip) || fits(*kind, &text[pos..]))
                }
                Some(Seg::Lit(next)) => {
                    let mut end = pos + 1;
                    while end <= stop && end + next.len() <= text.len() {
                        if text[end..].starts_with(next) && (matches!(kind, Kind::Any | Kind::Skip) || fits(*kind, &text[pos..end])) {
                            caps[*i] = (pos, end);
                            if match_at(rest, text, end, caps, fits) {
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

/// 句界：句末標點（. ! ? "）後面連續兩個空白，k 是第一個空白。遊戲拼段落時句間空兩格。
/// 雙引號裡面的不算（石板刻文「I am …, once of the Underworld.  By …」整段引在一句裡）。
fn is_boundary(text: &[u8], k: usize) -> bool {
    k > 0 && text[k] == b' ' && text.get(k + 1) == Some(&b' ') && b".!?\"".contains(&text[k - 1]) && !in_quote(text, k)
}

/// 段落框（mtb）的句界：遊戲排段落時把句間兩個空白收成一個，留下「句末標點（可再接收尾的引號）、一個空白、
/// 大寫字母、[B] 這類標記或連結」。佔位不跨這種句界，段落框整段查詞時才不會把後面整串吞進來；
/// translate_paragraph 也在這裡還原成兩個空白再切句。雙引號裡面的不算。
fn is_box_break(text: &[u8], k: usize) -> bool {
    let end = |i: usize| b".!?".contains(&text[i]) || (text[i] == b'"' && i > 0 && b".!?".contains(&text[i - 1]));
    k > 0 && text[k] == b' ' && end(k - 1) && text.get(k + 1).is_some_and(|c| c.is_ascii_uppercase() || *c == b'[' || *c == LINK_OPEN)
        && !in_quote(text, k)
}

/// 位置 k 在一對雙引號裡（前面的雙引號是奇數個）。只在其他條件都成立時才數，句界很少，不必預先算。
fn in_quote(text: &[u8], k: usize) -> bool {
    text[..k].iter().filter(|&&c| c == b'"').count() % 2 == 1
}

/// 樣板固定字的顏色位置：夾在兩個佔位之間、這兩個佔位在原文裡也相鄰時，用原文兩者之間那段字的位置
/// （「笨拙、虛弱」的「、」對應原文紅色的「 and 」）；其餘用句首（0）。
fn literal_at(output: &[Out], p: usize, caps: &[(usize, usize); 9]) -> usize {
    let cap = |o: &Out| match o {
        Out::Cap(i, _) => Some(*i),
        Out::Text(_) => None,
    };
    let (Some(i), Some(j)) = (output[..p].iter().rev().find_map(cap), output[p + 1..].iter().find_map(cap)) else { return 0 };
    let (end, next) = (caps[i].1, caps[j].0);
    if end <= next && !caps.iter().any(|&(s, _)| end <= s && s < next) { end } else { 0 }
}

#[derive(Default)]
pub struct Dict {
    pub entries: HashMap<Vec<u8>, Entry>,
    /// 只在佔位裡用的詞（字串檔 align 寫 inner）：句型的 {@n}、{%n} 抓到時查得到，整句查詢與介面單獨畫出時不用。
    /// 樂器音色的 dark、full、round 這類一般單字：句型要用，介面上單獨出現時不該翻成音色。
    inner: HashMap<Vec<u8>, Entry>,
    /// 全部小寫的原文 → 原文：只收含大小寫字母、而且全小寫寫法本身沒有條目的（見 lookup_name）。
    /// 兩條原文只差大小寫、譯文又不同時不收，結果才不看雜湊表的順序。
    folded: HashMap<Vec<u8>, Vec<u8>>,
    /// 原文含逗號的條目（身體部位 `first finger, right hand`）：片段以這種條目結尾、前面又不是清單的「, 」時不套清單句型，
    /// 「the goblin's first finger, right hand」才會整條當部位名，不會在逗號切成兩項
    comma_keys: Vec<Vec<u8>>,
    /// 只在某些畫面套用的整句：原文 → [(畫面, 限定列, 譯文)]。同一個英文在不同畫面（或同畫面不同列）意思不同時用（見 screen_entry）。
    pub screens: HashMap<Vec<u8>, Vec<(String, Option<i32>, Entry)>>,
    pub rules: Vec<Rule>,
    /// 原文 → 句型規則的結果（None 表示沒有規則命中，也要記住，免得每幀重比）
    cache: Mutex<HashMap<Vec<u8>, Option<Arc<Entry>>>>,
    /// 字 → 字圖編號；字圖 i 的左右半格在 texpos[2i]、texpos[2i+1]。
    pub glyph_of: HashMap<char, u32>,
    pub texpos: Vec<i32>,
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

    /// 任一段文字的替身字串與字圖（段落換行後的一段，見 mtb.rs）；缺字圖時回傳 None。
    pub fn text_entry(&self, text: &str) -> Option<Entry> {
        let (entry, missing) = self.entry(0, text, "");
        (missing == 0).then_some(entry)
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
            report.loaded += 1;
            if align == "inner" {
                // 句中常把首字大寫的片語寫成小寫（「In the simple passage」對 `The simple passage`）：另存一份小寫的，原樣的條目優先
                let lower = key.to_ascii_lowercase();
                if lower != key {
                    self.inner.entry(lower).or_insert_with(|| entry.clone());
                }
                self.inner.insert(key, entry);
                continue;
            }
            self.entries.insert(key, entry);
            if let Some(key) = box_form(en).and_then(|f| cp437::encode(&f)).filter(|k| !self.entries.contains_key(k)) {
                let (entry, _) = self.entry(key.len(), zh, align);
                self.entries.insert(key, entry);
            }
        }
        self.rebuild_folded();
        self.comma_keys = self.entries.keys().filter(|k| k.windows(2).any(|w| w == b", ")).cloned().collect();
        report
    }

    /// 重建大小寫不同時的查詞索引（見 lookup_name）。
    fn rebuild_folded(&mut self) {
        let mut folded: HashMap<Vec<u8>, Option<&Vec<u8>>> = HashMap::new();
        for (key, entry) in &self.entries {
            if !key.iter().any(u8::is_ascii_uppercase) || !key.iter().any(u8::is_ascii_lowercase) {
                continue;
            }
            let lower = key.to_ascii_lowercase();
            if self.entries.contains_key(&lower) {
                continue;
            }
            let slot = folded.entry(lower).or_insert(Some(key));
            if slot.is_some_and(|k| self.entries[k].text != entry.text) {
                *slot = None;
            }
        }
        self.folded = folded.into_iter().filter_map(|(lower, key)| Some((lower, key?.clone()))).collect();
    }

    /// 依畫面套用的整句：`畫面<TAB>原文<TAB>譯文<TAB>對齊<TAB>限定列`，畫面是 DFHack 焦點字串的開頭（如 dwarfmode），
    /// 限定列空白表示任何一列。
    pub fn load_screens_tsv(&mut self, content: &str) -> LoadReport {
        let mut report = LoadReport { loaded: 0, bad_source: 0, missing_glyphs: 0 };
        for line in content.lines().filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let mut cols = line.split('\t');
            let (Some(screen), Some(en), Some(zh)) = (cols.next(), cols.next(), cols.next()) else { continue };
            let align = cols.next().unwrap_or("");
            let row = match cols.next().unwrap_or("") {
                "" => None,
                r => match r.parse::<i32>() {
                    Ok(r) => Some(r),
                    Err(_) => {
                        report.bad_source += 1;
                        continue;
                    }
                },
            };
            let Some(key) = cp437::encode(en) else {
                report.bad_source += 1;
                continue;
            };
            let (entry, missing) = self.entry(key.len(), zh, align);
            report.missing_glyphs += missing;
            self.screens.entry(key).or_default().push((screen.to_owned(), row, entry));
            report.loaded += 1;
        }
        report
    }

    /// 目前畫面專用的譯文。focus 是 DFHack 的焦點字串（可能有好幾個，以換行分隔）；
    /// 畫面要是其中一個的開頭，而且剛好在 / 或結尾切開（dwarfmode 符合 dwarfmode/Default，不符合 dwarfmodeX）；
    /// 條目限定了列時，字串也要畫在那一列（y）。
    pub fn screen_entry<'a>(&'a self, key: &[u8], focus: &str, y: i32) -> Option<&'a Entry> {
        let list = self.screens.get(key)?;
        let on = |screen: &str| {
            focus.lines().any(|f| f.strip_prefix(screen).is_some_and(|rest| rest.is_empty() || rest.starts_with('/')))
        };
        list.iter().find(|(screen, row, _)| row.is_none_or(|r| r == y) && on(screen)).map(|(_, _, entry)| entry)
    }

    /// 句型規則檔，格式同 load_tsv；依檔案順序比對，先寫的優先，但清單句型一律最先，沒有英文字的樣板一律排在有英文字的後面，
    /// 同一類裡開頭就是任意佔位的排在後面。
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
                .filter_map(|o| match o {
                    Out::Text(t) | Out::Cap(_, t) => Some(t),
                })
                .flat_map(|t| t.chars())
                .filter(|c| cp437::byte_of(*c).is_none() && !self.glyph_of.contains_key(c))
                .count();
            self.rules.push(rule);
            report.loaded += 1;
            if let Some(alias) = box_form(en).and_then(|f| Rule::parse(&f, zh, align)) {
                self.rules.push(alias);
            }
        }
        // 穩定排序：同一類維持檔案順序
        self.rules.sort_by_key(|r| (!r.list, !r.specific, r.loose));
        self.cache.lock().clear();
        report
    }

    /// 一段原文的譯文（不含對齊）：整句字典優先（含首字大寫的名稱），再套句型規則。
    pub fn translate_text(&self, key: &[u8]) -> Option<String> {
        self.piece(key, 0, &mut Memo::new()).map(|(p, _)| concat(p))
    }

    /// 遊戲拼成一整段的文字：整段查不到時，在句末標點後的兩個以上空白切開逐句查（遊戲拼段落時句間空兩格），
    /// 每句都有譯文才算，句間不留空白。成功回傳譯文片段（位置是整段原文的位置），否則回傳查不到的句子。
    /// 整段用雙引號包住（人物頁概況的引言）時引號內照樣逐句查，引號換成條目 `""` 的譯文（「」），沒有就照原樣。
    pub fn translate_sentences(&self, text: &[u8]) -> Result<Pieces, Vec<Vec<u8>>> {
        let mut memo = Memo::new();
        if let Some((p, _)) = self.piece(text, 0, &mut memo) {
            return Ok(p);
        }
        if let [b'"', inner @ .., b'"'] = text {
            let inner = self.translate_sentences(inner)?;
            let marks = lookup_name(self, b"\"\"").map_or("\"\"", |e| e.text.as_str());
            let split = marks.char_indices().nth(1).map_or(marks.len(), |(i, _)| i);
            let mut out = vec![(marks[..split].to_owned(), 0)];
            out.extend(inner.into_iter().map(|(t, at)| (t, at + 1)));
            out.push((marks[split..].to_owned(), text.len() - 1));
            return Ok(out);
        }
        let mut out = Pieces::new();
        let mut missing = Vec::new();
        for (start, end) in sentence_spans(text) {
            let sentence = &text[start..end];
            // 沒有英文字的句子照原樣：遊戲某個分支沒寫字、只接了句號（「.」），或音樂形式的節奏示意行（「| x - x | x x x |」，
            // 只有單一字母和符號）。至少兩個字母連在一起才算英文
            if !sentence.windows(2).any(|w| w[0].is_ascii_alphabetic() && w[1].is_ascii_alphabetic()) {
                out.push((cp437::decode(sentence), start));
                continue;
            }
            // 遊戲偶爾在句子中間多拼一個空白（「The force pulls the left ankle,  shattering…」）：照原樣查不到時收成一個再查，
            // 譯文位置都記在句首（只影響彩色文字框的上色）
            let found = self.piece(sentence, 0, &mut memo).map(|(p, _)| p.into_iter().map(|(t, at)| (t, at + start)).collect::<Vec<_>>())
                .or_else(|| {
                    let single = collapse_spaces(sentence)?;
                    self.piece(&single, 0, &mut memo).map(|(p, _)| p.into_iter().map(|(t, _)| (t, start)).collect())
                });
            match found {
                Some(p) => out.extend(p),
                None => missing.push(sentence.to_vec()),
            }
        }
        if missing.is_empty() && !out.is_empty() { Ok(out) } else { Err(missing) }
    }

    /// 段落框（mtb）的一整段：先整段查；查不到時在 [B]／[R]／[P] 處分段，每段把單空白句界還原成兩個空白再逐句查
    /// （和彩色文字框一樣），每段都翻得出來才算。有換色標記的段落只整段查（逐句時顏色對不回去）。
    pub fn translate_paragraph(&self, key: &[u8]) -> Option<String> {
        if let Some(t) = self.translate_text(key) {
            return Some(t);
        }
        if key.windows(3).any(|w| w == b"[C:") {
            return None;
        }
        let mut out = String::new();
        let mut rest = key;
        loop {
            let mark = [b"[B]", b"[R]", b"[P]"].iter()
                .filter_map(|m| rest.windows(3).position(|w| w == *m).map(|i| (i, *m)))
                .min_by_key(|&(i, _)| i);
            let body = mark.map_or(rest, |(i, _)| &rest[..i]).trim_ascii();
            if !body.is_empty() {
                let mut restored = Vec::with_capacity(body.len() + 8);
                for (k, &c) in body.iter().enumerate() {
                    restored.push(c);
                    if is_box_break(body, k) {
                        restored.push(b' ');
                    }
                }
                out.push_str(&concat(self.translate_sentences(&restored).ok()?));
            }
            let Some((i, m)) = mark else { break };
            out.push_str(std::str::from_utf8(m).unwrap_or_default());
            rest = &rest[i + 3..];
        }
        Some(out)
    }

    /// 一個片段的譯文：整句字典（含大寫退回小寫），再套句型。
    fn piece(&self, key: &[u8], depth: usize, memo: &mut Memo) -> Option<Found> {
        if let Some(e) = if depth > 0 { lookup_inner(self, key) } else { lookup_name(self, key) } {
            return Some((vec![(e.text.clone(), 0)], true));
        }
        if depth > MAX_DEPTH {
            return None;
        }
        if let Some(hit) = memo.get(key) {
            return hit.clone();
        }
        // 先記成翻不出來：佔位抓到整段（`{%1}.` 這類）又遞迴回同一段時就此打住
        memo.insert(key.to_vec(), None);
        let found = self.apply_rules(key, depth, memo).map(|(p, _, clean)| (p, clean));
        memo.insert(key.to_vec(), found.clone());
        found
    }

    /// 第一條命中的句型：(譯文片段, 句型, 乾淨與否)。
    fn apply_rules(&self, key: &[u8], depth: usize, memo: &mut Memo) -> Option<(Pieces, &Rule, bool)> {
        // 清單與冠詞外框只會是句子的一部分：最外層（整句）不套，免得把「人名 likes 甲, 乙」當成一項
        let ends_with_comma_entry = self.comma_keys.iter().any(|k| {
            key.len() > k.len() && key.ends_with(k) && !key[..key.len() - k.len()].ends_with(b", ")
        });
        for rule in self.rules.iter().filter(|r| depth > 0 || !(r.list || r.inner)).filter(|r| !(r.list && ends_with_comma_entry)) {
            let mut fits = |kind: Kind, piece: &[u8]| match kind {
                Kind::Any | Kind::Skip => true,
                Kind::Number => is_number(piece),
                Kind::Term => lookup_inner(self, piece).is_some(),
                Kind::Full => matches!(self.piece(piece, depth + 1, memo), Some((_, true))),
            };
            let Some(caps) = rule.matches(key, &mut fits) else { continue };
            let mut out = Pieces::new();
            let mut translated = false;
            let mut all_clean = true;
            for (p, part) in rule.output.iter().enumerate() {
                match part {
                    Out::Text(t) => out.push((t.clone(), literal_at(&rule.output, p, &caps))),
                    Out::Cap(i, extra) => {
                        let (start, end) = caps[*i];
                        let from = out.len();
                        match self.piece(&key[start..end], depth + 1, memo) {
                            Some((sub, clean)) => {
                                translated = true;
                                all_clean &= clean;
                                out.extend(sub.into_iter().map(|(t, at)| (t, at + start)));
                            }
                            None => {
                                // 數字照抄也算乾淨（「plump helmets [5]」的 5 不必翻）
                                all_clean &= rule.pattern.iter().any(|s| matches!(s, Seg::Cap(j, Kind::Number) if j == i));
                                out.push((cp437::decode(&key[start..end]), start));
                            }
                        }
                        let last = out[from..].iter().rev().find_map(|(t, _)| t.chars().last());
                        let skip = if extra == COLOR { COLOR } else { PARTICLES };
                        if !extra.is_empty() && !last.is_some_and(|c| skip.contains(c)) {
                            out.push((extra.clone(), start));
                        }
                    }
                }
            }
            if rule.has_cjk || translated {
                return Some((out, rule, rule.has_cjk || rule.specific || all_clean));
            }
        }
        None
    }

    /// hook 用：整句字典沒有時，先試首字小寫的名稱，再套句型規則並換算成替身字串。譯文比原文寬或缺字圖時放棄，照畫英文。
    pub fn rule_entry(&self, key: &[u8]) -> Option<Arc<Entry>> {
        let mut cache = self.cache.lock();
        if let Some(hit) = cache.get(key) {
            return hit.clone();
        }
        // 小寫寫法的原文和 key 等長，替身字串與對齊都能照用
        let entry = lookup_name(self, key).map(|e| Arc::new(e.clone())).or_else(|| {
            self.apply_rules(key, 0, &mut Memo::new()).and_then(|(pieces, rule, _)| {
                let (entry, missing) = self.entry(key.len(), &concat(pieces), &rule.align);
                (missing == 0 && entry.shadow().len() <= key.len()).then(|| Arc::new(entry))
            })
        });
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(key.to_vec(), entry.clone());
        entry
    }
}

/// 連續空白收成一個；沒有連續空白時回傳 None。
fn collapse_spaces(text: &[u8]) -> Option<Vec<u8>> {
    if !text.windows(2).any(|w| w == b"  ") {
        return None;
    }
    let mut out = Vec::with_capacity(text.len());
    for &c in text {
        if !(c == b' ' && out.last() == Some(&b' ')) {
            out.push(c);
        }
    }
    Some(out)
}

/// 彈出訊息（段落框，見 mtb.rs plain_text）把原文拆成單字再接回：單字間一個空白，[B] [R] [P] 自成一字。
/// 同一則公告在警報清單照原樣畫（句間常空兩格），在彈出視窗是這個樣子；和原文不同時載入成別名，一條兩處都能用。
/// 有換色標記的不處理：段落框把標記移到單字前面、第一個顏色不標，規則不同。
fn box_form(en: &str) -> Option<String> {
    if en.contains("[C:") {
        return None;
    }
    let spaced = ["[B]", "[R]", "[P]"].iter().fold(en.to_owned(), |s, m| s.replace(m, &format!(" {m} ")));
    let mut out = String::with_capacity(spaced.len());
    for c in spaced.chars() {
        if !(c == ' ' && out.ends_with(' ')) {
            out.push(c);
        }
    }
    // 片段頭尾的空白是原文的一部分（" cancels "），留一個；標記帶出來的頭尾空白去掉
    let mut s = out.as_str();
    if !en.starts_with(' ') {
        s = s.trim_start_matches(' ');
    }
    if !en.ends_with(' ') {
        s = s.trim_end_matches(' ');
    }
    (s != en).then(|| s.to_owned())
}

/// 句子的 (起, 迄)：在 . ! ? " 後面連續兩個以上空白處切開，去掉頭尾空白，略過空句。
fn sentence_spans(text: &[u8]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < text.len() {
        if is_boundary(text, i) {
            spans.push((start, i));
            while text.get(i) == Some(&b' ') {
                i += 1;
            }
            start = i;
        } else {
            i += 1;
        }
    }
    spans.push((start, text.len()));
    spans
        .into_iter()
        .map(|(s, e)| {
            let s = s + text[s..e].iter().take_while(|&&c| c == b' ').count();
            let e = e - text[s..e].iter().rev().take_while(|&&c| c == b' ').count();
            (s, e)
        })
        .filter(|(s, e)| s < e)
        .collect()
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
        let d = dict("-{1}-\t-{1}-\n({1})\t（{1}）\n");
        assert_eq!(d.translate_text(b"-Cupidity-").as_deref(), Some("-貪婪-"));
        assert_eq!(d.translate_text(b"-Greed-"), None);
        assert_eq!(d.translate_text(b"(Cupidity)").as_deref(), Some("（貪婪）"));
        assert_eq!(d.translate_text(b"(Greed)"), None, "全形標點不算樣板裡的中文");
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
    fn capitalized_names_fall_back_to_lowercase_entries() {
        let mut d = dict("{1} of {@2}\t{2}的{1}\n");
        d.load_tsv("greed\t貪婪\nhistories\t歷史\nlabor\t來訪\ngreed labor\t貪婪勞動\n");
        assert_eq!(d.rule_entry(b"Greed").map(|e| e.text.clone()).as_deref(), Some("貪婪"), "整句：大寫開頭退回小寫條目");
        assert_eq!(d.translate_text(b"Greed Labor").as_deref(), Some("貪婪勞動"), "每個字大寫的稱呼也退回小寫");
        assert_eq!(d.translate_text(b"Histories of Greed").as_deref(), Some("貪婪的歷史"), "佔位片段與限定佔位也退回小寫");
        assert_eq!(d.translate_text(b"Labor").as_deref(), Some("勞動"), "大寫條目存在時用大寫條目");
        assert_eq!(d.translate_text(b"GREED"), None, "全大寫的字串不退回");
    }

    #[test]
    fn rules_with_english_words_are_tried_before_bare_placeholder_rules() {
        let d = dict("{@1} {@2}\t{1}與{2}\nLabor {@1}\t{1}的勞動\n");
        assert_eq!(d.translate_text(b"Labor Cupidity").as_deref(), Some("貪婪的勞動"), "通用樣板寫在前面也要讓給有英文字的");
        assert_eq!(d.translate_text(b"Cupidity Labor").as_deref(), Some("貪婪與勞動"));
    }

    #[test]
    fn screen_entries_only_apply_on_their_screen_and_row() {
        let mut d = dict("");
        d.load_screens_tsv("dwarfmode\tDrink\t勞動\t\t0\nchoose_start_site\tTemperate\t貪婪\n");
        assert!(d.screen_entry(b"Drink", "dwarfmode/Default", 0).is_some());
        assert!(d.screen_entry(b"Drink", "dwarfmode", 0).is_some());
        assert!(d.screen_entry(b"Drink", "dwarfmode/Default", 9).is_none(), "限定第 0 列，畫在別列不套");
        assert!(d.screen_entry(b"Drink", "dwarfmodeX/Default", 0).is_none(), "要在 / 切開");
        assert!(d.screen_entry(b"Drink", "dungeonmode/Default", 0).is_none());
        assert!(d.screen_entry(b"Temperate", "title/Default\nchoose_start_site", 7).is_some(), "焦點有好幾個時任一個符合即可；沒限定列");
    }

    #[test]
    fn full_placeholder_rejects_untranslatable_pieces() {
        let d = dict("{1} sang {%2}.\t{1}唱了{2}。\nof {@1}\t{1}的歷史\n");
        assert_eq!(d.translate_text(b"Urist sang of Labor.").as_deref(), Some("Urist唱了勞動的歷史。"), "句型翻得出來的片段可收");
        assert_eq!(d.translate_text(b"Urist sang Cupidity.").as_deref(), Some("Urist唱了貪婪。"), "整句字典的片段可收");
        assert_eq!(d.translate_text(b"Urist sang badly."), None, "翻不出來就不算命中，不留半句英文");
    }

    #[test]
    fn lists_recurse_one_item_per_level_and_backtrack() {
        let mut d = dict("{1} likes {%2} and {%3}.\t{1}喜歡{2}和{3}。\n{%1}, {%2}\t{1}、{2}\n");
        d.load_tsv("greed\t貪婪\nlabor and toil\t勞動\n");
        let items: Vec<&str> = std::iter::repeat_n("Labor", 30).collect();
        let long = format!("Urist likes {} and Cupidity.", items.join(", "));
        let zh = d.translate_text(long.as_bytes()).expect("三十項清單");
        assert!(zh.starts_with("Urist喜歡勞動、勞動") && zh.ends_with("勞動和貪婪。"), "{zh}");
        assert_eq!(d.translate_text(b"Urist likes greed, labor and toil and Labor.").as_deref(), Some("Urist喜歡貪婪、勞動和勞動。"),
                   "項目本身含 and：在 and 處切開翻不出來就往後找");
        let bad = format!("Urist likes {}, Zzz and Cupidity.", items.join(", "));
        assert_eq!(d.translate_text(bad.as_bytes()), None, "其中一項翻不出來：回頭試遍切法也要很快結束");
    }

    #[test]
    fn sentences_translate_one_by_one_with_source_positions() {
        let mut d = dict("{1} is visiting.\t{1}來訪了。\n");
        d.load_tsv("Labor ends.\t勞動結束。\n");
        let text = b"Urist is visiting.  Labor ends.  ";
        let pieces = d.translate_sentences(text).expect("兩句都有譯文");
        assert_eq!(pieces, vec![("Urist".into(), 0), ("來訪了。".into(), 0), ("勞動結束。".into(), 20)]);
        assert_eq!(d.translate_sentences(b"Labor ends.  Greed went home."), Err(vec![b"Greed went home.".to_vec()]), "缺一句就整段不翻，回傳缺的句子");
        assert_eq!(sentence_spans(b"A b.  C, d  e!  "), vec![(0, 4), (6, 14)], "只在句末標點後切");
    }

    #[test]
    fn placeholders_never_swallow_the_next_sentence() {
        let mut d = dict("{1} is visiting.\t{1}來訪了。\n");
        d.load_tsv("Labor ends.\t勞動結束。\n");
        let text = b"Urist is visiting.  Labor ends.";
        assert_eq!(d.translate_text(text), None, "整段比對時 {{1}} 不能抓到「Urist is visiting.  Labor ends」");
        assert_eq!(d.translate_sentences(text).map(|p| p.into_iter().map(|(t, _)| t).collect::<String>()), Ok("Urist來訪了。勞動結束。".into()));
        assert_eq!(d.translate_text(b"Labor ends. Urist is visiting."), None,
                   "段落框把句間空白收成一個時也不跨句（遊戲的名稱不含「Dr.」這類縮寫，句點後接大寫就當句界）");
    }

    #[test]
    fn list_rules_split_before_item_rules_but_only_inside_placeholders() {
        let d = dict("Labor at {1}\t到{1}勞動\n{%1}, {%2}\t{1}、{2}\n{1} likes {%2}.\t{1}喜歡{2}。\n{1}, {@2}\t{1}，{2}\n");
        assert_eq!(d.translate_text(b"Urist likes Labor at The Hall, Cupidity.").as_deref(), Some("Urist喜歡到The Hall勞動、貪婪。"),
                   "清單句型先切，項目句型的 {{1}} 才不會吞掉「, Cupidity」");
        assert_eq!(d.translate_text(b"Urist likes Labor at Bob, Zzz.").as_deref(), Some("Urist喜歡到Bob, Zzz勞動。"), "有一項翻不出來時清單不成立，照項目句型");
        assert_eq!(d.translate_text(b"Urist likes Zzz, Labor.").as_deref(), None,
                   "{{%2}} 不收「Zzz，勞動」：只有標點的外框 {{1}}, {{@2}} 照抄英文不算翻得出來，整句照英文");
        assert_eq!(d.translate_text(b"Urist, Labor").as_deref(), Some("Urist，勞動"), "整句的「人名, 職業」照舊");
    }

    #[test]
    fn rejects_bad_placeholder_layouts() {
        let r = dict("").load_rules_tsv("{1}{2}\t{1}{2}\n{1} x\t{2}\n{1} y\t{1}{1}\nno placeholder\t沒有\n{1} z {@2}\t{2}\n{1+x} w\t{1}\n");
        assert_eq!((r.loaded, r.bad_source), (0, 6), "倒數第二條漏了任意佔位 {{1}}；最後一條原文帶補字");
    }

    #[test]
    fn suffix_is_added_only_when_the_capture_lacks_a_particle() {
        let mut d = dict("{@1} feels {%2}.\t{1}{2+時}感到喜悅。\nafter {%1}\t{1}後\n");
        d.load_tsv("he\t他\ntalking with Labor\t和勞動交談\nwhen Labor came\t勞動來時\n");
        assert_eq!(d.translate_text(b"He feels talking with Labor.").as_deref(), Some("他和勞動交談時感到喜悅。"), "沒有介系詞的片語補「時」");
        assert_eq!(d.translate_text(b"He feels after talking with Labor.").as_deref(), Some("他和勞動交談後感到喜悅。"), "「後」結尾不補");
        assert_eq!(d.translate_text(b"He feels when Labor came.").as_deref(), Some("他勞動來時感到喜悅。"), "已經有「時」不重複");
    }

    #[test]
    fn rules_starting_with_a_free_placeholder_are_tried_after_constrained_ones() {
        let mut d = dict("{1} was given away\t{1}被送走\n{@1} feels {%2}.\t{1}{2+時}感到喜悅。\nafter {1} was given away\t在{1}被送走後\n");
        d.load_tsv("he\t他\n");
        assert_eq!(d.translate_text(b"He feels after The Gem was given away.").as_deref(), Some("他在The Gem被送走後感到喜悅。"),
                   "寫在前面的 {{1}} was given away 不能把「He feels after The Gem」吞進 {{1}}");
    }

    #[test]
    fn term_placeholders_may_be_left_out_of_the_translation() {
        let d = dict("{@1} loves {%2}, and {@3} hates {%4}.\t{1}熱愛{2}，卻討厭{4}。\n");
        assert_eq!(d.translate_text(b"Labor loves Cupidity, and Labor hates Labor.").as_deref(), Some("勞動熱愛貪婪，卻討厭勞動。"),
                   "{{@3}}（重複的主詞）可以省略");
    }

    #[test]
    fn popup_spacing_matches_entries_written_with_alert_spacing() {
        let mut d = dict("A caravan from {1} has arrived.  Trade is {@2}.[B]Be ready.\t{1}的商隊到了。交易{2}。[B]準備好。\n");
        d.load_tsv("Labor ends.  Rest begins.\t勞動結束。休息開始。\nLabor ends. Rest begins.\t另寫的\ngood\t好\n cancels \t取消\n");
        assert_eq!(d.translate_text(b"A caravan from Urist has arrived. Trade is good. [B] Be ready.").as_deref(),
                   Some("Urist的商隊到了。交易好。[B]準備好。"), "彈出視窗：單字間一個空白、[B] 自成一字");
        assert_eq!(d.translate_text(b"A caravan from Urist has arrived.  Trade is good.[B]Be ready.").as_deref(),
                   Some("Urist的商隊到了。交易好。[B]準備好。"), "警報清單照原文");
        assert_eq!(d.translate_text(b"Labor ends. Rest begins.").as_deref(), Some("另寫的"), "明寫的條目不被別名蓋掉");
        assert_eq!(d.translate_text(b"cancels").as_deref(), None, "片段頭尾的空白不去掉");
    }

    #[test]
    fn quoted_speech_translates_sentence_by_sentence_inside_the_quotes() {
        let mut d = dict("");
        d.load_tsv("The battle rages...\t戰鬥正激烈……\nI am so happy!\t我好開心！\n");
        let zh = |d: &Dict, s: &[u8]| d.translate_sentences(s).map(concat);
        assert_eq!(zh(&d, b"\"The battle rages...  I am so happy!\""), Ok("\"戰鬥正激烈……我好開心！\"".to_owned()), "沒有引號條目時照原樣");
        d.load_tsv("\"\"\t「」\n");
        assert_eq!(zh(&d, b"\"The battle rages...  I am so happy!\""), Ok("「戰鬥正激烈……我好開心！」".to_owned()));
        assert_eq!(zh(&d, b"\"The battle rages...  I am sad.\""), Err(vec![b"I am sad.".to_vec()]), "缺的句子不含引號");
    }

    #[test]
    fn lookups_fall_back_to_entries_that_differ_only_in_case() {
        let mut d = dict("{1} cancels {%2}.\t{1}取消了{2}。\n");
        d.load_tsv("Woodcutter\t伐木工\nTrade depot\t貿易站\nash\t梣樹\nAsh\t木灰\nRest Area\t甲\nRest area\t乙\n");
        assert_eq!(d.translate_text(b"woodcutter").as_deref(), Some("伐木工"), "公告裡小寫的職業");
        assert_eq!(d.translate_text(b"Trade Depot").as_deref(), Some("貿易站"), "每個字大寫的建築名");
        assert_eq!(d.translate_text(b"ash").as_deref(), Some("梣樹"), "有完全相同的寫法時照用");
        assert_eq!(d.translate_text(b"rest area"), None, "只差大小寫的兩條譯文不同時不猜");
        assert_eq!(d.translate_text(b"Urist cancels trade Depot.").as_deref(), Some("Urist取消了貿易站。"));
    }

    #[test]
    fn article_wrappers_apply_only_inside_placeholders() {
        let mut d = dict("The {%1}\t{1}\tinner\nthe {%1}\t{1}\tinner\n{@1} is {@2} {%3}.\t{1}{3+時}感到{2}。\n{%1} misses {%2}!\t{1}沒打中{2}！\n");
        d.load_tsv("dwarf\t矮人\nsinging\t歌唱\nwell\t水井\nvulture\t禿鷲\n");
        assert_eq!(d.translate_text(b"The dwarf is singing well."), None, "整句去掉 The 之後不能拿半句去比想法句");
        assert_eq!(d.translate_text(b"The vulture misses the dwarf!").as_deref(), Some("禿鷲沒打中矮人！"));
    }

    #[test]
    fn skip_placeholders_match_freely_and_may_be_left_out() {
        let mut d = dict("The {%1}\t{1}\tinner\nthe {%1}\t{1}\tinner\n{%1}'s {%2}\t{1}的{2}\tinner\n\
                          {%1} locks {%2} with {~3}'s {%4}!\t{1}用{4}鎖住了{2}！\n");
        d.load_tsv("woodcutter\t伐木工\nvulture\t禿鷲\nright hip\t右髖部\nright lower arm\t右前臂\n");
        assert_eq!(d.translate_text(b"The woodcutter locks the vulture's right hip with The woodcutter's right lower arm!").as_deref(),
                   Some("伐木工用右前臂鎖住了禿鷲的右髖部！"), "攻擊者自己的部位不必再寫一次主人");
    }

    #[test]
    fn part_names_with_commas_are_not_split_as_lists() {
        let mut d = dict("the {%1}\t{1}\tinner\n{%1}'s {%2}\t{1}的{2}\tinner\n{%1}, {%2}\t{1}、{2}\n{%1} bites {%2}!\t{1}咬了{2}！\n");
        d.load_tsv("goblin\t哥布林\ndwarf\t矮人\nfirst toe\t第一腳趾\nright foot\t右腳\nfirst toe, right foot\t右腳第一腳趾\n");
        assert_eq!(d.translate_text(b"the goblin bites the dwarf's first toe, right foot!").as_deref(), Some("哥布林咬了矮人的右腳第一腳趾！"));
        assert_eq!(d.translate_text(b"the goblin bites the dwarf's first toe, first toe, right foot!").as_deref(),
                   Some("哥布林咬了矮人的第一腳趾、右腳第一腳趾！"), "前面是清單的「, 」時照樣切");
    }

    #[test]
    fn sentences_with_stray_double_spaces_fall_back_to_single_spaces() {
        let mut d = dict("The force pulls the {%1}, shattering {%2}!\t這股力道拉扯了{1}，打碎了{2}！\n");
        d.load_tsv("left ankle\t左腳踝\nthe bone\t骨頭\nHe is slow to anger.\t他不容易動怒。\n");
        let zh = |s: &[u8]| d.translate_sentences(s).map(concat);
        assert_eq!(zh(b"The force pulls the left ankle,  shattering the bone!"), Ok("這股力道拉扯了左腳踝，打碎了骨頭！".to_owned()));
        assert_eq!(zh(b"He is slow to anger.  The force pulls the left ankle,  shattering the bone!"),
                   Ok("他不容易動怒。這股力道拉扯了左腳踝，打碎了骨頭！".to_owned()), "句間的兩個空白照樣切句");
    }

    #[test]
    fn punctuation_only_sentences_pass_through() {
        let mut d = dict("");
        d.load_tsv("He is slow to anger.\t他不容易動怒。\n");
        assert_eq!(d.translate_sentences(b"He is slow to anger.  .  He is slow to anger.").map(concat),
                   Ok("他不容易動怒。.他不容易動怒。".to_owned()), "遊戲沒寫字只接句號的分支不讓整段變回英文");
        assert_eq!(d.translate_sentences(b"He is slow to anger.  | x - x | x x x |").map(concat),
                   Ok("他不容易動怒。| x - x | x x x |".to_owned()), "節奏示意行只有單一字母和符號");
    }

    #[test]
    fn box_paragraphs_fall_back_to_blocks_and_sentences() {
        let mut d = dict("{1} is visiting.\t{1}來訪了。\n");
        d.load_tsv("Labor ends.\t勞動結束。\n");
        assert_eq!(d.translate_paragraph(b"Labor ends. Urist is visiting. [B] Labor ends.").as_deref(), Some("勞動結束。Urist來訪了。[B]勞動結束。"),
                   "段落框的單空白句界還原後逐句查，[B] 照原樣接回");
        assert_eq!(d.translate_paragraph(b"Labor ends. Urist is visiting. [B] Nobody knows."), None, "缺一句就整段照英文");
    }

    #[test]
    fn number_captures_count_as_clean() {
        let mut d = dict("{%1} [{#2}]\t{1} [{2}]\n{1} has stolen {%2}!\t{1}偷走了{2}！\n");
        d.load_tsv("plump helmets\t肉盔菇\n");
        assert_eq!(d.translate_text(b"Urist has stolen plump helmets [5]!").as_deref(), Some("Urist偷走了肉盔菇 [5]！"),
                   "{{%2}} 只收乾淨的譯文：數字照抄不算半中半英");
    }

    #[test]
    fn color_suffix_is_added_only_when_the_color_lacks_one() {
        let mut d = dict("{@1} hair is {%2}.\t{1}的頭髮是{2+色}。\n");
        d.load_tsv("her\t她\ngold\t金\ndark tan\t深黃褐\nred\t紅色\n");
        assert_eq!(d.translate_text(b"Her hair is gold.").as_deref(), Some("她的頭髮是金色。"));
        assert_eq!(d.translate_text(b"Her hair is dark tan.").as_deref(), Some("她的頭髮是深黃褐色。"));
        assert_eq!(d.translate_text(b"Her hair is red.").as_deref(), Some("她的頭髮是紅色。"), "已經有「色」就不再補");
    }

    #[test]
    fn inner_words_are_used_only_inside_placeholders() {
        let mut d = dict("It has a {@1} timbre.\t音色{1}。\n");
        d.load_tsv("dark\t暗沉\tinner\n");
        assert_eq!(d.translate_text(b"It has a dark timbre.").as_deref(), Some("音色暗沉。"));
        assert_eq!(d.translate_text(b"dark"), None, "介面上單獨出現的 dark 不翻成音色");
        assert_eq!(d.translate_text(b"Dark"), None);
    }

    #[test]
    fn link_tokens_behave_like_names_and_start_box_sentences() {
        let mut d = dict("{1} is visiting.\t{1}來訪了。\n{1} of {%2}\t{2}的{1}\nIn {#1}, {2} {%3}.\t{1}年，{2}{3}。\nIn {#1}, {2} settled in {3}.\t{1}年，{2}定居於{3}。\n");
        d.load_tsv("Labor ends.\t勞動結束。\n");
        assert_eq!(d.translate_text(b"Histories of \x010\x02"), None, "{{%n}} 不收單獨的連結，和沒有連結的名字一樣");
        assert_eq!(d.translate_text(b"In 1, \x010\x02 settled in \x011\x02.").as_deref(), Some("1年，☺0☻定居於☺1☻。"),
                   "給片語的 {{%3}} 不能吞下單獨的名字，搶走後面專寫的句型");
        assert_eq!(d.translate_paragraph(b"Labor ends. \x010\x02 is visiting.").as_deref(), Some("勞動結束。☺0☻來訪了。"),
                   "連結開頭的句子是新句，{{1}} 不能吞進前一句");
        assert_eq!(link_order("☺1☻與☺0☻", 2), Some(vec![1, 0]));
        assert_eq!(link_order("☺0☻與☺0☻", 2), None, "重複");
        assert_eq!(link_order("☺0☻", 2), None, "漏掉");
    }

    #[test]
    fn quotes_end_sentences_but_text_inside_them_stays_whole() {
        let mut d = dict("It asks, \"{1}\"\t問道：「{1}」\nThe slab reads \"{1}\".\t石板上寫著：「{1}」。\n");
        d.load_tsv("Labor ends.\t勞動結束。\n");
        assert_eq!(d.translate_paragraph(b"It asks, \"Why?\" Labor ends.").as_deref(), Some("問道：「Why?」勞動結束。"),
                   "段落框裡 ?\" 後面一個空白接大寫字母也是句界");
        assert_eq!(d.translate_sentences(b"The slab reads \"I am Urist.  By Labor, I bind.\".  Labor ends.").map(concat).as_deref(),
                   Ok("石板上寫著：「I am Urist.  By Labor, I bind.」。勞動結束。"), "引號裡的句子不切開");
        assert_eq!(d.translate_paragraph(b"The slab reads \"I am Urist. By Labor, I bind.\". Labor ends.").as_deref(),
                   Some("石板上寫著：「I am Urist. By Labor, I bind.」。勞動結束。"), "段落框也一樣");
    }
}
