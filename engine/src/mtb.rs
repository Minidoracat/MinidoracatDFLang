//! 自動換行段落（markup_text_boxst）：教學、說明、彈出視窗的長文。
//!
//! 遊戲先把一段英文拆成單字清單，排版函式（mtb_set_width）依寬度算出每個單字的座標，
//! 畫面上再一個字一個字畫（彈出視窗走 top_addst）。整句比對看到的只有單字，永遠對不到。
//! 這裡 hook 排版函式：原函式排完英文後，把單字接回整段去查字典；有譯文就依中文規則重新換行，
//! 替每個單字訂好畫字計畫——每段中文交給同一列的一個英文單字（載體）畫，其他單字不畫。
//! 畫字 hook 看到載體單字，就在「載體座標＋位移」畫那段中文。
//!
//! 遊戲的單字字串一律不改：彈出視窗排好的單字會存進存檔，改過的字串讀檔後（或移除本 MOD 後）會變成亂碼。
//! 沒有譯文的段落也登記成「照畫英文」：單字若剛好是字典裡的介面標籤（All、Fish、Iron…）才不會被換成中文，夾在英文句子裡。
//!
//! 有連結的段落（傳說頁面、物品說明裡可以點的人名、地名）：連結字由遊戲照英文畫、依座標判斷點擊，
//! 所以照中文排版把連結字搬到中文裡名字的位置（只改座標，見 plan_links）。這種段落是畫面開啟時才產生的，不進存檔。

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, OnceLock};

use parking_lot::Mutex;

use crate::cp437;
use crate::dict::{self, Dict, Entry, LINK_CLOSE, LINK_OPEN};

/// markup_text_boxst／markup_text_wordst 的欄位位移，由 Lua 依 DFHack 的結構定義傳入。
#[derive(Clone, Copy)]
pub struct MtbLayout {
    pub words: usize,
    pub current_width: usize,
    pub word_str: usize,
    pub word_red: usize,
    pub word_link: usize,
    pub word_px: usize,
    pub word_py: usize,
    pub word_flags: usize,
    /// gps.uccolor 的位址：遊戲調色盤 16 色 × RGB。單字只存 RGB，換回調色盤編號，段落原文才不受玩家配色影響
    pub palette: usize,
}

type Palette = [[u8; 3]; 16];

unsafe fn palette(l: &MtbLayout) -> Option<Palette> {
    (l.palette != 0).then(|| unsafe { *(l.palette as *const Palette) })
}

/// 標記 [C:前景:背景:亮度] 的顏色在調色盤上的 RGB（背景不影響字色）
fn marker_rgb(palette: &Palette, [fg, _, bright]: [u8; 3]) -> [u8; 3] {
    palette[(fg as usize % 8) + 8 * usize::from(bright != 0)]
}

pub static MTB_LAYOUT: OnceLock<MtbLayout> = OnceLock::new();

/// markup_text_word_flag：[R] 換行、[B] 空一行、[P] 換行並縮排四格
const NEW_LINE: u32 = 1;
const BLANK_LINE: u32 = 2;
const INDENT: u32 = 4;
const MARKERS: u32 = NEW_LINE | BLANK_LINE | INDENT;

/// 不能放在行首的標點（中文與半形；簡中的引號是 “”），換行時連同前一個字一起移到下一行
const NO_START: &str = "，。、；：！？）」』》〉】〕”’…‥％・～,.;:!?)]}%";
/// 不能放在行尾的標點，換行時移到下一行
const NO_END: &str = "（「『《〈【〔“‘([{";

/// 每個段落（以 markup_text_boxst 位址區分）上次排版的單字清單與寬度：都沒變就不重做。
struct State {
    words: Vec<usize>,
    width: i32,
    translated: bool,
}

static STATES: LazyLock<Mutex<HashMap<usize, State>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// 一段中文：畫在載體字串座標＋(dx, dy)；color 有值時用它上色，沒有就沿用遊戲當下的顏色。
pub struct Piece {
    pub dx: i32,
    pub dy: i32,
    pub color: Option<[u8; 3]>,
    pub entry: Entry,
}

/// 段落單字或自動換行的一行被畫到時要做的事。
#[derive(Clone)]
pub enum Plan {
    /// 沒有譯文：照畫英文，不查字典
    Plain,
    /// 沒有譯文的提示行（見 lines.rs）：照畫英文，畫到時把整句記成未翻譯（不記片段）
    Missing(Arc<[u8]>),
    /// 有譯文，但這個字串不負責畫中文：不畫
    Skip,
    /// 載體：畫這幾段中文
    Draw(Arc<[Piece]>),
}

/// 段落單字的畫字計畫：字串位址 → (排版後的座標, 計畫)。
static WORDS: LazyLock<Mutex<HashMap<usize, (i32, i32, Plan)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// ponytail: 段落釋放時沒有通知，舊單字只在同一段落重排時撤掉；超過上限整張清掉，
/// 當時畫面上的段落要等下次重排才恢復中文。要精確就 hook 段落的解構函式。
const WORDS_LIMIT: usize = 200_000;

unsafe fn read<T: Copy>(base: usize, offset: usize) -> T {
    unsafe { *((base + offset) as *const T) }
}

unsafe fn write<T>(base: usize, offset: usize, v: T) {
    unsafe { *((base + offset) as *mut T) = v }
}

/// MSVC std::string：16 bytes 內建緩衝區（或長字串指標）、長度、容量。
unsafe fn str_bytes<'a>(s: usize) -> &'a [u8] {
    unsafe {
        let (size, cap): (usize, usize) = (read(s, 16), read(s, 24));
        let data = if cap > 15 { read::<*const u8>(s, 0) } else { s as *const u8 };
        std::slice::from_raw_parts(data, size)
    }
}

/// 一個連結：同一個連結的連續單字，以及頭尾黏在字上、不屬於名字的字元數。遊戲把 [/LPAGE] 後面緊接的字元
/// 也算進同一個單字（`Mangemined.` 的句號、`Urist's` 的 's），連單字一起畫成連結色；這些字元移到記號外面給句型看，
/// 中文版面裡不畫（見 plan_links）。
struct Link {
    words: Vec<usize>,
    head: usize,
    tail: usize,
}

/// 連結字開頭（( " '）與結尾（標點、括號、引號、所有格 's）不屬於名字的位元組數；整個字都是這些字元時不切。
fn link_trim(word: &[u8]) -> (usize, usize) {
    let head = word.iter().take_while(|c| b"(\"'".contains(c)).count();
    let rest = &word[head..];
    let mut tail = rest.iter().rev().take_while(|c| b".,!?:;)\"'".contains(c)).count();
    if rest[..rest.len() - tail].ends_with(b"'s") {
        tail += 2;
    }
    if head + tail >= word.len() { (0, 0) } else { (head, tail) }
}

impl Link {
    /// 第 j 個字去掉頭尾字元後的部分（畫在中文版面裡的名字）
    unsafe fn name<'a>(&self, j: usize, words: &[usize], l: &MtbLayout) -> &'a [u8] {
        let w = unsafe { str_bytes(words[self.words[j]] + l.word_str) };
        let head = if j == 0 { self.head } else { 0 };
        let tail = if j + 1 == self.words.len() { self.tail } else { 0 };
        &w[head..w.len() - tail]
    }
}

/// 單字接回整段：單字間一個空白，單獨的 . , ? ! 直接接在前一字後面（遊戲排版也是這樣貼著），
/// 換行標記寫成 [R]、[B]、[P]；換顏色的單字前面加 [C:前景:背景:亮度]（第一個單字的顏色當整段底色，不標）。
/// 都和遊戲的標記語法相同，譯文可以照用。可以點的連結寫成記號 ☺序號☻（見 dict::LINK_OPEN），
/// 黏在連結字頭尾的標點寫在記號外面；一併回傳每個連結。顏色不在調色盤上的段落不處理。
unsafe fn plain_text(words: &[usize], l: &MtbLayout, palette: Option<&Palette>) -> Option<(Vec<u8>, Vec<Link>)> {
    let mut out: Vec<u8> = Vec::new();
    let mut color: Option<[u8; 3]> = None;
    let mut links: Vec<Link> = Vec::new();
    let mut prev_link = -1;
    // 目前這個連結最後一個字的尾巴：連結結束時接在記號後面
    let mut tail: &[u8] = b"";
    for (i, &w) in words.iter().enumerate() {
        unsafe {
            let flags: u32 = read(w, l.word_flags);
            let link: i32 = read(w, l.word_link);
            let text = str_bytes(w + l.word_str);
            if link != -1 && flags & MARKERS == 0 {
                // 連結的顏色由遊戲依連結種類決定，不看單字的顏色
                let (head, t) = link_trim(text);
                match links.last_mut() {
                    Some(span) if link == prev_link => span.words.push(i),
                    _ => {
                        out.extend_from_slice(tail);
                        if !out.is_empty() {
                            out.push(b' ');
                        }
                        out.extend_from_slice(&text[..head]);
                        out.push(LINK_OPEN);
                        out.extend_from_slice(links.len().to_string().as_bytes());
                        out.push(LINK_CLOSE);
                        links.push(Link { words: vec![i], head, tail: 0 });
                    }
                }
                links.last_mut().unwrap().tail = t;
                tail = &text[text.len() - t..];
                prev_link = link;
                continue;
            }
            out.extend_from_slice(std::mem::take(&mut tail));
            prev_link = -1;
            let token: &[u8] = if flags & NEW_LINE != 0 {
                b"[R]"
            } else if flags & BLANK_LINE != 0 {
                b"[B]"
            } else if flags & INDENT != 0 {
                b"[P]"
            } else {
                text
            };
            if token.is_empty() {
                continue;
            }
            if !out.is_empty() && !(token.len() == 1 && b".,?!".contains(&token[0])) {
                out.push(b' ');
            }
            if flags & MARKERS == 0 {
                let c: [u8; 3] = read(w, l.word_red);
                if color.is_some_and(|prev| prev != c) {
                    // ponytail: 調色盤有兩格同色時取第一格，原文就和遊戲原始標記不同而對不上；遇到再改成比對原始字串
                    let i = palette?.iter().position(|p| *p == c)?;
                    out.extend_from_slice(format!("[C:{}:0:{}]", i % 8, i / 8).as_bytes());
                }
                color = Some(c);
            }
            out.extend_from_slice(token);
        }
    }
    out.extend_from_slice(tail);
    Some((out, links))
}

/// 未翻譯紀錄用：連結記號換回名字（☺名字☻），`t -m` 可以直接拿來查。
unsafe fn readable(key: &[u8], words: &[usize], links: &[Link], l: &MtbLayout) -> Vec<u8> {
    let mut out = Vec::with_capacity(key.len() + 16 * links.len());
    let mut rest = key;
    for link in links {
        let Some(i) = rest.iter().position(|&c| c == LINK_OPEN) else { break };
        let j = rest[i..].iter().position(|&c| c == LINK_CLOSE).map_or(rest.len(), |j| i + j + 1);
        out.extend_from_slice(&rest[..=i]);
        for n in 0..link.words.len() {
            if n > 0 {
                out.push(b' ');
            }
            out.extend_from_slice(unsafe { link.name(n, words, l) });
        }
        out.push(LINK_CLOSE);
        rest = &rest[j..];
    }
    out.extend_from_slice(rest);
    out
}

/// 換行的最小單位：中文字（兩格）、連在一起的英數（不拆開）、空白、換行標記、換色標記、連結字。
#[derive(Debug, PartialEq)]
enum Atom {
    Text(String, usize),
    Space,
    NewLine,
    Blank,
    Indent,
    Color([u8; 3]),
    /// 第幾個連結的第幾個字、格數
    Link(usize, usize, usize),
}

/// `[C:前景:背景:亮度]`：回傳顏色與標記的長度。遊戲偶爾多拼一個冒號（`[C:7:0:0:]`），照樣收。
fn color_marker(s: &str) -> Option<([u8; 3], usize)> {
    let body = s.strip_prefix("[C:")?;
    let end = body.find(']')?;
    let mut nums = body[..end].strip_suffix(':').unwrap_or(&body[..end]).split(':').map(|n| n.parse::<u8>().ok());
    let color = [nums.next()??, nums.next()??, nums.next()??];
    nums.next().is_none().then_some((color, end + 4))
}

/// 譯文裡的連結記號 ☺序號☻：回傳序號與記號長度。
fn link_marker(s: &str) -> Option<(usize, usize)> {
    let (open, close) = (cp437::TABLE[LINK_OPEN as usize], cp437::TABLE[LINK_CLOSE as usize]);
    let body = s.strip_prefix(open)?;
    let end = body.find(close)?;
    Some((body[..end].parse().ok()?, open.len_utf8() + end + close.len_utf8()))
}

/// links[k] 是第 k 個連結各字的格數：連結記號展開成這些字，字間一個空白（遊戲照英文畫，可以在字間換行）。
fn atoms(text: &str, links: &[Vec<usize>]) -> Vec<Atom> {
    let mut out = Vec::new();
    let mut run = String::new();
    let flush = |run: &mut String, out: &mut Vec<Atom>| {
        if !run.is_empty() {
            let n = run.chars().count();
            out.push(Atom::Text(std::mem::take(run), n));
        }
    };
    let mut rest = text;
    while let Some(ch) = rest.chars().next() {
        if let Some((color, n)) = color_marker(rest) {
            flush(&mut run, &mut out);
            out.push(Atom::Color(color));
            rest = &rest[n..];
            continue;
        }
        if let Some((k, n)) = link_marker(rest) {
            flush(&mut run, &mut out);
            for (j, &w) in links.get(k).into_iter().flatten().enumerate() {
                if j > 0 {
                    out.push(Atom::Space);
                }
                out.push(Atom::Link(k, j, w));
            }
            rest = &rest[n..];
            continue;
        }
        let marker = [("[R]", Atom::NewLine), ("[B]", Atom::Blank), ("[P]", Atom::Indent)]
            .into_iter()
            .find(|(m, _)| rest.starts_with(m));
        if let Some((m, atom)) = marker {
            flush(&mut run, &mut out);
            out.push(atom);
            rest = &rest[m.len()..];
            continue;
        }
        rest = &rest[ch.len_utf8()..];
        match cp437::byte_of(ch) {
            Some(b' ') => {
                flush(&mut run, &mut out);
                out.push(Atom::Space);
            }
            Some(_) if !NO_START.contains(ch) && !NO_END.contains(ch) => run.push(ch),
            _ => {
                flush(&mut run, &mut out);
                let w = if cp437::byte_of(ch).is_some() { 1 } else { 2 };
                out.push(Atom::Text(ch.to_string(), w));
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

/// 換行後的一段：(px, py, 文字, 顏色)。同一行中途換顏色就拆成兩段。
pub type Segment = (i32, i32, String, [u8; 3]);

/// 換行後連結字的位置：(第幾個連結, 連結裡第幾個字, px, py)。
type Placed = (usize, usize, i32, i32);

/// 依中文規則換行，回傳每段的位置、文字與顏色：base 是整段底色（RGB），[C:…] 之後改用標記的顏色（跨行延續），
/// 標記的顏色由 rgb 換算。和遊戲的排版一致：[R] 換行、[B] 再空一行、[P] 換行並縮排四格；
/// 中文字之間不加空白，英數之間的空白保留；行首不放 NO_START 標點、行尾不放 NO_END 標點。
pub fn wrap(text: &str, width: usize, base: [u8; 3], rgb: impl Fn([u8; 3]) -> [u8; 3]) -> Vec<Segment> {
    wrap_links(text, width, base, rgb, &[]).0
}

/// 同 wrap，另外排譯文裡的連結記號：links[k] 是第 k 個連結各字的格數。回傳的段不含連結字，連結字的位置另外回傳。
fn wrap_links(text: &str, width: usize, base: [u8; 3], rgb: impl Fn([u8; 3]) -> [u8; 3], links: &[Vec<usize>]) -> (Vec<Segment>, Vec<Placed>) {
    /// 一行裡的各段：(文字, 格數, 顏色, 連結字)；連結字的文字是空的
    struct Line {
        px: usize,
        py: i32,
        parts: Vec<(String, usize, [u8; 3], Option<(usize, usize)>)>,
    }
    let width = width.max(4);
    let mut lines: Vec<Line> = Vec::new();
    let (mut x, mut y, mut remaining) = (0usize, 0i32, width);
    let mut space = false;
    let mut color = base;
    let new_line = |lines: &mut Vec<Line>, px: usize, py: i32| lines.push(Line { px, py, parts: Vec::new() });
    for atom in atoms(text, links) {
        let (t, w, link) = match atom {
            Atom::Color(c) => {
                color = rgb(c);
                continue;
            }
            Atom::NewLine => {
                remaining = 0;
                continue;
            }
            Atom::Blank => {
                remaining = 0;
                x = 0;
                y += 1;
                continue;
            }
            Atom::Indent => {
                remaining = width;
                x = 4;
                y += 1;
                new_line(&mut lines, x, y);
                continue;
            }
            Atom::Space => {
                space = lines.last().is_some_and(|l| l.py == y && !l.parts.is_empty());
                continue;
            }
            Atom::Text(t, w) => (t, w, None),
            Atom::Link(k, j, w) => (String::new(), w, Some((k, j))),
        };
        let gap = usize::from(space);
        space = false;
        if remaining < w + gap {
            // 換行；標點不放行首：把上一行最後一個字一起帶下來
            let carry = if link.is_none() && NO_START.contains(t.as_str()) {
                lines.last_mut().filter(|l| l.parts.len() > 1).and_then(|l| l.parts.pop())
            } else {
                // 上一行以開括號結尾：括號移到下一行
                lines.last_mut()
                    .filter(|l| l.parts.len() > 1 && l.parts.last().is_some_and(|p| !p.0.is_empty() && NO_END.contains(p.0.as_str())))
                    .and_then(|l| l.parts.pop())
            };
            y += 1;
            x = 0;
            remaining = width;
            new_line(&mut lines, 0, y);
            if let Some(c) = carry {
                x += c.1;
                remaining -= c.1.min(remaining);
                lines.last_mut().unwrap().parts.push(c);
            }
            if w > remaining && link.is_none() {
                // 比整行還長的英數：硬切（連結字由遊戲照英文整個畫，不切）
                let mut chunk = String::new();
                for ch in t.chars() {
                    if remaining == 0 {
                        let n = chunk.chars().count();
                        lines.last_mut().unwrap().parts.push((std::mem::take(&mut chunk), n, color, None));
                        y += 1;
                        x = 0;
                        remaining = width;
                        new_line(&mut lines, 0, y);
                    }
                    chunk.push(ch);
                    remaining -= 1;
                }
                let n = chunk.chars().count();
                x += n;
                lines.last_mut().unwrap().parts.push((chunk, n, color, None));
                continue;
            }
        } else if lines.last().is_none_or(|l| l.py != y) {
            new_line(&mut lines, x, y);
        } else if gap == 1 {
            // 空白跟前一個字同色，才不會多拆出一段；空白也佔一格
            let line = lines.last_mut().unwrap();
            let prev = line.parts.last().map_or(color, |p| p.2);
            line.parts.push((" ".to_owned(), 1, prev, None));
            x += 1;
            remaining -= 1;
        }
        x += w;
        remaining = remaining.saturating_sub(w);
        lines.last_mut().unwrap().parts.push((t, w, color, link));
    }
    let mut out: Vec<Segment> = Vec::new();
    let mut placed: Vec<Placed> = Vec::new();
    for line in lines {
        let mut px = line.px as i32;
        // 這一段能不能接在 out 最後一段後面：同一行、中間沒有連結字
        let mut join = false;
        for (text, w, c, link) in line.parts {
            if let Some((k, j)) = link {
                placed.push((k, j, px, line.py));
                join = false;
            } else if join && out.last().is_some_and(|seg| seg.3 == c) {
                out.last_mut().unwrap().2.push_str(&text);
            } else if join || text != " " {
                // 連結字後面的空白不必畫
                out.push((px, line.py, text, c));
                join = true;
            }
            px += w as i32;
        }
    }
    (out, placed)
}

/// 排版函式跑完後呼叫：依譯文訂好這個段落每個單字的畫字計畫。回傳 true 表示這段有譯文。
pub unsafe fn rewrite(mtb: usize, dict: &Dict, note_miss: impl FnOnce(&[u8])) -> bool {
    let Some(l) = MTB_LAYOUT.get() else { return false };
    unsafe {
        let begin: usize = read(mtb, l.words);
        let end: usize = read(mtb, l.words + 8);
        if begin == 0 || end <= begin {
            return false;
        }
        let words = std::slice::from_raw_parts(begin as *const usize, (end - begin) / 8);
        let width: i32 = read(mtb, l.current_width);
        let mut states = STATES.lock();
        if let Some(s) = states.get(&mtb).filter(|s| s.words == words && s.width == width) {
            return s.translated; // 已排過、寬度沒變：計畫還有效
        }
        let pal = palette(l);
        // (每個單字的計畫, 有連結時每個單字的新座標)
        let planned = plain_text(words, l, pal.as_ref()).and_then(|(key, links)| {
            // 句型漏掉或重複了名字也算沒翻：連結字沒有位置可放
            let (t, complete) = dict.translate_paragraph(&key);
            let Some(t) = t.filter(|t| dict::link_order(t, links.len()).is_some()) else {
                if links.is_empty() {
                    note_miss(&key);
                } else {
                    note_miss(&readable(&key, words, &links, l));
                }
                return None;
            };
            if !complete {
                // 傳說頁有幾段翻不出來（照原文畫）：整段記下，之後補
                note_miss(&readable(&key, words, &links, l));
            }
            if links.is_empty() {
                plan_words(words, width, &t, dict, l, pal.as_ref()).map(|p| (p, None))
            } else {
                plan_links(words, width, &t, dict, l, pal.as_ref(), &links).map(|(p, pos)| (p, Some(pos)))
            }
        });
        let translated = planned.is_some();
        let mut table = WORDS.lock();
        if let Some(old) = states.remove(&mtb) {
            for w in old.words {
                table.remove(&(w + l.word_str));
            }
        }
        if table.len() + words.len() > WORDS_LIMIT {
            table.clear();
            states.clear();
        }
        for (i, &w) in words.iter().enumerate() {
            let plan = planned.as_ref().map_or(Plan::Plain, |(p, _)| p[i].clone());
            if let Some((_, Some(pos))) = &planned {
                write(w, l.word_px, pos[i].0);
                write(w, l.word_py, pos[i].1);
            }
            table.insert(w + l.word_str, (read(w, l.word_px), read(w, l.word_py), plan));
        }
        states.insert(mtb, State { words: words.to_vec(), width, translated });
        translated
    }
}

/// 依中文規則換行，把每段中文交給同一列第一個英文單字畫：回傳每個單字的計畫。
/// 載體和中文在同一列，捲動的清單裡遊戲依載體決定畫不畫，才會和中文那列一致。
/// 缺字圖或沒有能畫的單字時回傳 None（照畫英文）。
/// ponytail: 中文行數比英文多時，多出來的行掛在最後一個單字下面，會畫到段落框外；
/// 段落高度 max_y 會存進存檔所以不改，遇到再想辦法。
unsafe fn plan_words(words: &[usize], width: i32, translation: &str, dict: &Dict, l: &MtbLayout,
                     pal: Option<&Palette>) -> Option<Vec<Plan>> {
    unsafe {
        // 遊戲會畫的單字：(索引, px, py)
        let drawable: Vec<(usize, i32, i32)> = words
            .iter()
            .enumerate()
            .filter(|&(_, &w)| read::<u32>(w, l.word_flags) & MARKERS == 0 && !str_bytes(w + l.word_str).is_empty())
            .map(|(i, &w)| (i, read(w, l.word_px), read(w, l.word_py)))
            .collect();
        let &(first, ..) = drawable.first()?;
        // 底色取第一個單字的；譯文的 [C:…] 標記之後換成標記的顏色（沒有調色盤就不換色）
        let base: [u8; 3] = read(words[first], l.word_red);
        let segments = wrap(translation, width.max(1) as usize, base, |c| pal.map_or(base, |p| marker_rgb(p, c)));
        let mut pieces: Vec<Vec<Piece>> = (0..words.len()).map(|_| Vec::new()).collect();
        for (px, py, text, color) in segments {
            let entry = dict.text_entry(&text)?;
            // 那列英文是空行時往下找，再沒有就用最後一個單字
            let &(i, wx, wy) = drawable.iter().find(|d| d.2 == py).or_else(|| drawable.iter().find(|d| d.2 > py))
                .or(drawable.last())?;
            pieces[i].push(Piece { dx: px - wx, dy: py - wy, color: Some(color), entry });
        }
        Some(pieces.into_iter().map(|p| if p.is_empty() { Plan::Skip } else { Plan::Draw(p.into()) }).collect())
    }
}

/// 有連結的段落：連結字（遊戲照英文畫、依座標判斷點擊）搬到中文裡名字的位置，其他單字當載體畫中文，回傳計畫與每個單字的新座標。
/// 遊戲畫字與判斷點擊都照單字順序掃、碰到第一個列超出框底的單字就停，所以載體由上而下一列一個，
/// 而且後面的連結字列都不比它小；不畫的單字搬到第 -1 列（和換行標記一樣），不會擋住掃描。
/// 連結字頭尾黏著的標點（見 Link）不畫：那個字改由 hook 畫去掉標點的名字，點擊範圍照遊戲（多出標點那幾格）。
/// 缺字圖或沒有能當載體的單字時回傳 None（照畫英文）。
/// ponytail: 中文把名字的先後換過、又剛好分在兩列時，前面的連結字列比較大，框底剛好切在兩列之間時
/// 後面那列不畫、名字點不到（捲一列就恢復）；要完全正確得 hook 畫字與點擊的迴圈。
unsafe fn plan_links(words: &[usize], width: i32, translation: &str, dict: &Dict, l: &MtbLayout, pal: Option<&Palette>,
                     links: &[Link]) -> Option<(Vec<Plan>, Vec<(i32, i32)>)> {
    unsafe {
        let mut is_link = vec![false; words.len()];
        for &i in links.iter().flat_map(|k| &k.words) {
            is_link[i] = true;
        }
        // 可以搬的單字（不是連結字也不是換行標記）與其中能當載體的（字串不是空的）
        let movable: Vec<bool> = words.iter().enumerate().map(|(i, &w)| !is_link[i] && read::<u32>(w, l.word_flags) & MARKERS == 0).collect();
        let carrier = |i: usize| movable[i] && !str_bytes(words[i] + l.word_str).is_empty();
        let base: [u8; 3] = read(words[(0..words.len()).find(|&i| carrier(i))?], l.word_red);
        // 連結是世界生成的名稱時畫中文名（見 names.rs）：整個名字當一個字排，第一個連結字畫中文、
        // 其他字疊在同一處不畫（點擊範圍照遊戲的英文字長）
        let named: Vec<Option<String>> = links.iter()
            .map(|k| link_name(dict, &(0..k.words.len()).map(|j| k.name(j, words, l)).collect::<Vec<_>>().join(&b' ')))
            .collect();
        let widths: Vec<Vec<usize>> = links.iter().zip(&named).map(|(k, zh)| match zh {
            Some(zh) => vec![zh.chars().map(|c| if cp437::byte_of(c).is_some() { 1 } else { 2 }).sum()],
            None => (0..k.words.len()).map(|j| k.name(j, words, l).len()).collect(),
        }).collect();
        let (segments, placed) = wrap_links(translation, width.max(1) as usize, base, |c| pal.map_or(base, |p| marker_rgb(p, c)), &widths);
        let mut pos: Vec<(i32, i32)> = words.iter().map(|&w| (read(w, l.word_px), read(w, l.word_py))).collect();
        let mut link_plans: Vec<Option<Plan>> = vec![None; words.len()];
        for (k, j, px, py) in placed {
            let link = &links[k];
            if let Some(zh) = &named[k] {
                let entry = dict.text_entry(zh)?;
                for &i in &link.words {
                    pos[i] = (px, py);
                    link_plans[i] = Some(Plan::Skip);
                }
                link_plans[link.words[0]] = Some(Plan::Draw(vec![Piece { dx: 0, dy: 0, color: None, entry }].into()));
                continue;
            }
            let i = link.words[j];
            pos[i] = (px, py);
            let whole = (j > 0 || link.head == 0) && (j + 1 < link.words.len() || link.tail == 0);
            link_plans[i] = Some(if whole {
                Plan::Plain
            } else {
                // 去掉標點的名字由 hook 畫；顏色沿用遊戲畫這個連結字時設好的連結色
                let entry = dict.text_entry(&cp437::decode(link.name(j, words, l)))?;
                Plan::Draw(vec![Piece { dx: 0, dy: 0, color: None, entry }].into())
            });
        }
        // 每一列的中文：(列, [(px, 字, 顏色)])；換行後的段依列排好
        let mut rows: Vec<(i32, Vec<(i32, Entry, [u8; 3])>)> = Vec::new();
        for (px, py, text, color) in segments {
            let entry = dict.text_entry(&text)?;
            match rows.last_mut() {
                Some((row, pieces)) if *row == py => pieces.push((px, entry, color)),
                _ => rows.push((py, vec![(px, entry, color)])),
            }
        }
        // after[i]：第 i 個單字起，後面連結字最小的列
        let mut after = vec![i32::MAX; words.len() + 1];
        for i in (0..words.len()).rev() {
            after[i] = if is_link[i] { after[i + 1].min(pos[i].1) } else { after[i + 1] };
        }
        let mut pieces: Vec<Vec<Piece>> = (0..words.len()).map(|_| Vec::new()).collect();
        let mut rows = rows.into_iter().peekable();
        let mut last = None;
        for i in 0..words.len() {
            if !movable[i] {
                continue;
            }
            pos[i].1 = -1;
            if carrier(i) && rows.peek().is_some_and(|(row, _)| *row <= after[i + 1]) {
                let (row, segs) = rows.next().unwrap();
                let x0 = segs[0].0;
                pos[i] = (x0, row);
                pieces[i] = segs.into_iter().map(|(px, entry, color)| Piece { dx: px - x0, dy: 0, color: Some(color), entry }).collect();
                last = Some(i);
            }
        }
        // 後面只剩連結字時，剩下的列交給最後一個載體，畫在它下面
        for (row, segs) in rows {
            let i = last?;
            let (x0, y0) = pos[i];
            pieces[i].extend(segs.into_iter().map(|(px, entry, color)| Piece { dx: px - x0, dy: row - y0, color: Some(color), entry }));
        }
        let plans = pieces.into_iter().zip(link_plans)
            .map(|(p, link)| link.unwrap_or_else(|| if p.is_empty() { Plan::Skip } else { Plan::Draw(p.into()) }))
            .collect();
        Some((plans, pos))
    }
}

/// 連結字的中文名：整段是遊戲組過的世界生成名稱，或「the goblin＋名稱」這種稱呼加名稱（見 Dict::titled_name），
/// 或是沒有名字的建築、地下區域、族群（`a temple`、`the magma sea`）照字典翻。
fn link_name(dict: &Dict, text: &[u8]) -> Option<String> {
    crate::names::get(text).map(|zh| zh.to_string()).or_else(|| dict.titled_name(text, true)).or_else(|| dict.translate_fragment(text))
}

/// 畫字 hook 用：這個字串是段落單字時，回傳它的畫字計畫。
/// 單字釋放後位址可能給別的字串用，所以座標也要和排版時相同才算；讀的位置在原本單字結構的範圍內。
pub unsafe fn word_plan(s: usize) -> Option<Plan> {
    let l = MTB_LAYOUT.get()?;
    let table = WORDS.lock();
    let (px, py, plan) = table.get(&s)?;
    let w = s - l.word_str;
    unsafe { (read::<i32>(w, l.word_px) == *px && read::<i32>(w, l.word_py) == *py).then(|| plan.clone()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GRAY: [u8; 3] = [7, 0, 0];

    fn lines(text: &str, width: usize) -> Vec<String> {
        wrap(text, width, GRAY, |c| c).into_iter().map(|(px, py, t, _)| format!("{px},{py}:{t}")).collect()
    }

    #[test]
    fn chinese_wraps_at_width_without_spaces() {
        // 寬 8 格放 4 個中文字
        assert_eq!(lines("準備帶領你的矮人", 8), ["0,0:準備帶領", "0,1:你的矮人"]);
    }

    #[test]
    fn punctuation_never_starts_a_line() {
        // 寬 12 格：第一行剛好放滿六個字，「。」會落在第二行開頭，所以把「人」一起帶下去
        assert_eq!(lines("準備帶領矮人。出發", 12), ["0,0:準備帶領矮", "0,1:人。出發"]);
        assert_eq!(lines("准备带领矮人”。出发", 12), ["0,0:准备带领矮", "0,1:人”。出发"], "简中的后引号也不放行首");
    }

    #[test]
    fn latin_words_stay_whole_and_keep_spaces() {
        assert_eq!(lines("按 Esc 鍵離開", 20), ["0,0:按 Esc 鍵離開"]);
        assert_eq!(lines("按住 Shift 鍵", 6), ["0,0:按住", "0,1:Shift", "0,2:鍵"]);
        // 名字裡的空白也佔一格：寬 14 格放不下「來自The Razor」（13 格）再加「的」
        assert_eq!(lines("來自The Razor的商隊", 14), ["0,0:來自The Razor", "0,1:的商隊"]);
    }

    #[test]
    fn markers_follow_game_layout() {
        // [B] 空一行、[P] 換行縮排四格、[R] 換行
        assert_eq!(lines("第一段[B]第二段[P]縮排[R]下一行", 40),
                   ["0,0:第一段", "0,2:第二段", "4,3:縮排", "0,4:下一行"]);
    }

    #[test]
    fn color_markers_split_segments_and_carry_across_lines() {
        // 寬 12 格放 6 個中文字；綠色從「向」延續到下一行
        const GREEN: [u8; 3] = [2, 0, 1];
        assert_eq!(
            wrap("按[C:2:0:1]接受[C:7:0:0]鍵，[C:2:0:1]向大地進擊吧", 12, GRAY, |c| c),
            [
                (0, 0, "按".to_owned(), GRAY),
                (2, 0, "接受".to_owned(), GREEN),
                (6, 0, "鍵，".to_owned(), GRAY),
                (10, 0, "向".to_owned(), GREEN),
                (0, 1, "大地進擊吧".to_owned(), GREEN),
            ]
        );
    }

    #[test]
    fn link_words_are_laid_out_inside_the_chinese_text() {
        // 「由」＋連結兩字（Urist 5 格、McDwarf 7 格，字間一格）＋「製作的雕像」，寬 20 格
        let (segs, placed) = wrap_links("由☺0☻製作的雕像", 20, GRAY, |c| c, &[vec![5, 7]]);
        let segs: Vec<String> = segs.into_iter().map(|(px, py, t, _)| format!("{px},{py}:{t}")).collect();
        assert_eq!(segs, ["0,0:由", "15,0:製作", "0,1:的雕像"], "連結字後面另起一段，不和前面的中文接在一起");
        assert_eq!(placed, [(0, 0, 2, 0), (0, 1, 8, 0)]);
    }

    /// 假的段落：單字 56 bytes（MSVC std::string 32 bytes＋顏色、連結、座標、旗標），段落只用單字向量與寬度。
    struct Fake {
        mtb: usize,
        words: Vec<usize>,
    }

    /// 測試用調色盤：第 i 格是 (i, i, i)
    static PALETTE: Palette = {
        let mut p = [[0u8; 3]; 16];
        let mut i = 0;
        while i < 16 {
            p[i] = [i as u8 * 10; 3];
            i += 1;
        }
        p
    };

    impl Fake {
        /// (單字, 調色盤第幾格的顏色)
        fn new(texts: &[(&str, usize)], width: i32) -> Fake {
            let _ = MTB_LAYOUT.set(MtbLayout {
                words: 0, current_width: 48, word_str: 0, word_red: 32, word_link: 36, word_px: 40, word_py: 44,
                word_flags: 48, palette: PALETTE.as_ptr() as usize,
            });
            let mut x = 0;
            let words: Vec<usize> = texts
                .iter()
                .map(|&(t, color)| {
                    let w = Box::into_raw(Box::new([0u64; 7])) as usize;
                    unsafe {
                        std::ptr::copy_nonoverlapping(t.as_ptr(), w as *mut u8, t.len());
                        write(w, 16, t.len());
                        write(w, 24, 15usize);
                        write(w, 32, PALETTE[color]);
                        write(w, 36, -1i32);
                        write(w, 40, x);
                    }
                    x += t.len() as i32 + 1;
                    w
                })
                .collect();
            let mtb = Box::into_raw(Box::new([0u64; 8])) as usize;
            unsafe {
                write(mtb, 0, words.as_ptr() as usize);
                write(mtb, 8, words.as_ptr() as usize + words.len() * 8);
                write(mtb, 48, width);
            }
            Fake { mtb, words }
        }
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            unsafe {
                drop(Box::from_raw(self.mtb as *mut [u64; 8]));
                for &w in &self.words {
                    drop(Box::from_raw(w as *mut [u64; 7]));
                }
            }
        }
    }

    fn plan_of(w: usize) -> &'static str {
        match unsafe { word_plan(w) } {
            Some(Plan::Plain) => "plain",
            Some(Plan::Missing(_)) => "missing",
            Some(Plan::Skip) => "skip",
            Some(Plan::Draw(_)) => "draw",
            None => "none",
        }
    }

    unsafe fn set_link(p: &Fake, i: usize, link: i32) {
        unsafe { write(p.words[i], 36, link) }
    }

    fn pos(w: usize) -> (i32, i32) {
        unsafe { (read(w, 40), read(w, 44)) }
    }

    #[test]
    fn link_words_move_to_the_name_and_carriers_keep_scan_order() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("這是製作的杯子。", vec![0; 16]));
        d.load_rules_tsv("A mug by {1}.\t這是{1}製作的杯子。\n");
        unsafe {
            let p = Fake::new(&[("A", 7), ("mug", 7), ("by", 7), ("Urist", 7), (".", 7)], 10);
            set_link(&p, 3, 0);
            assert!(rewrite(p.mtb, &d, |_| {}));
            // 寬 10 格：第 0 列「這是Urist」、第 1 列「製作的杯」、第 2 列「子。」（「。」不放行首）
            assert_eq!((plan_of(p.words[3]), pos(p.words[3])), ("plain", (4, 0)), "連結字搬到「這是」後面，遊戲照這裡畫、判斷點擊");
            assert_eq!(str_bytes(p.words[3]), b"Urist", "字串不動");
            assert_eq!((plan_of(p.words[0]), pos(p.words[0])), ("draw", (0, 0)));
            // 畫字迴圈照單字順序掃、碰到列超出框底的就停：第 1 列的載體要在連結字（第 0 列）後面，
            // 連結字前面不畫的單字搬到第 -1 列
            assert_eq!((plan_of(p.words[1]), pos(p.words[1]).1), ("skip", -1));
            assert_eq!((plan_of(p.words[4]), pos(p.words[4])), ("draw", (0, 1)));
            let Some(Plan::Draw(rest)) = word_plan(p.words[4]) else { panic!("「.」要畫第 1 列") };
            assert_eq!(rest.iter().map(|p| (p.dx, p.dy)).collect::<Vec<_>>(), [(0, 0), (0, 1)], "後面沒有單字了，第 2 列也由它畫");
        }
    }

    #[test]
    fn untranslated_link_paragraphs_log_names_and_stay_in_place() {
        let d = Dict::default();
        unsafe {
            let p = Fake::new(&[("Made", 7), ("by", 7), ("Urist", 7), ("McDwarf", 7), (".", 7)], 40);
            set_link(&p, 2, 0);
            set_link(&p, 3, 0);
            let mut logged = String::new();
            assert!(!rewrite(p.mtb, &d, |k| logged = cp437::decode(k)));
            assert_eq!(logged, "Made by ☺Urist McDwarf☻.", "未翻譯紀錄寫名字，t -m 可以直接查");
            assert_eq!((plan_of(p.words[2]), pos(p.words[2])), ("plain", (8, 0)));
        }
    }

    #[test]
    fn punctuation_stuck_to_link_words_moves_out_of_the_name() {
        assert_eq!(link_trim(b"Mangemined."), (0, 1));
        assert_eq!(link_trim(b"Urist's"), (0, 2));
        assert_eq!(link_trim(b"(Urist),"), (1, 2));
        assert_eq!(link_trim(b"."), (0, 0), "整個字都是標點時不切");
        let mut d = Dict::default();
        assert!(d.set_glyphs("年，定居於。", vec![0; 12]));
        d.load_rules_tsv("In {#1}, {2} settled in {3}.\t{1}年，{2}定居於{3}。\n");
        unsafe {
            // 遊戲把 [/LPAGE] 後面的句號算進連結字「Mangemined.」：句型看到的句號在記號外面，中文版面裡不畫這個句號
            let p = Fake::new(&[("In", 7), ("1,", 7), ("Onsmost", 7), ("settled", 7), ("in", 7), ("Mangemined.", 7)], 40);
            set_link(&p, 2, 0);
            set_link(&p, 5, 1);
            assert!(rewrite(p.mtb, &d, |_| {}));
            // 「1年，」5 格、Onsmost 7 格、「定居於」6 格，Mangemined 10 格後面接「。」
            assert_eq!((plan_of(p.words[2]), pos(p.words[2])), ("plain", (5, 0)));
            let Some(Plan::Draw(name)) = word_plan(p.words[5]) else { panic!("帶句號的連結字改由 hook 畫名字") };
            assert_eq!((name[0].entry.text.as_str(), name[0].color, pos(p.words[5])), ("Mangemined", None, (18, 0)));
            let Some(Plan::Draw(row)) = word_plan(p.words[0]) else { panic!("第一個單字畫中文") };
            assert_eq!(row.iter().map(|p| (p.dx, p.entry.text.as_str())).collect::<Vec<_>>(), [(0, "1年，"), (12, "定居於"), (28, "。")]);
        }
    }

    #[test]
    fn untranslated_paragraph_words_are_drawn_as_english() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("全魚", vec![0; 4]));
        d.load_tsv("All\t全\nAll Fish\t全魚\n");
        unsafe {
            // 段落沒譯文：單字照畫英文，不拿 All 去查字典
            let plain = Fake::new(&[("Fish", 7), ("All", 7)], 40);
            assert!(!rewrite(plain.mtb, &d, |_| {}));
            assert_eq!(plan_of(plain.words[1]), "plain");
            // 單字釋放後同一位址給了別的字串（座標對不上）：不算
            write(plain.words[1], 40, 99i32);
            assert_eq!(plan_of(plain.words[1]), "none");
            // 遊戲依新寬度重排後重新登記
            write(plain.mtb, 48, 30i32);
            assert!(!rewrite(plain.mtb, &d, |_| {}));
            assert_eq!(plan_of(plain.words[1]), "plain");
        }
    }

    #[test]
    fn translated_paragraph_is_drawn_by_a_carrier_word_and_game_data_stays_english() {
        let mut d = Dict::default();
        assert!(d.set_glyphs("全魚", vec![0; 4]));
        d.load_tsv("All Fish\t全魚\n");
        unsafe {
            let p = Fake::new(&[("All", 7), ("Fish", 7)], 40);
            assert!(rewrite(p.mtb, &d, |_| {}));
            // 第一個單字畫整行中文，第二個不畫；遊戲的單字字串不動（會存進存檔）
            assert_eq!((plan_of(p.words[0]), plan_of(p.words[1])), ("draw", "skip"));
            assert_eq!(str_bytes(p.words[0]), b"All");
            assert_eq!(str_bytes(p.words[1]), b"Fish");
        }
    }

    #[test]
    fn color_markers_use_palette_indices_not_rgb() {
        // 原文標記是遊戲的 [C:前景:背景:亮度]（亮綠是第 2 色加亮 = 調色盤第 10 格），玩家換配色也不變
        let mut d = Dict::default();
        assert!(d.set_glyphs("開鑿大地", vec![0; 8]));
        d.load_tsv("Okay. [C:2:0:1]Strike\t開鑿[C:2:0:1]大地\n");
        unsafe {
            let p = Fake::new(&[("Okay", 7), (".", 7), ("Strike", 10)], 40);
            assert!(rewrite(p.mtb, &d, |_| {}));
            // 「開鑿」維持第一個字的顏色，「大地」換成調色盤第 10 格；兩段都在第 0 列，由第一個單字畫
            let Some(Plan::Draw(pieces)) = word_plan(p.words[0]) else { panic!("第一個單字要畫中文") };
            let colors: Vec<Option<[u8; 3]>> = pieces.iter().map(|p| p.color).collect();
            assert_eq!(colors, [Some(PALETTE[7]), Some(PALETTE[10])]);
            assert_eq!(pieces[1].dx, 4, "「大地」接在「開鑿」（4 格）後面");
        }
    }
}
