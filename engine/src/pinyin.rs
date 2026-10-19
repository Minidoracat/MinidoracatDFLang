//! 拼音搜尋：遊戲與 DFHack 視窗的搜尋框打拼音也找得到顯示成中文的項目（「鐵礦」打 tiekuang、tk、tiek、tkuang 都中）。
//! 讀音表 engine/pinyin.tsv 由 scripts/make_pinyin.py 從 Unicode Unihan 產生（多音字收全部讀音），編進 DLL。
//! 搜尋框比對的是英文（遊戲先轉小寫、去掉重音），所以先照字典把清單項目翻成中文再比讀音；譯文記在快取裡。

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

use crate::dict::Dict;

/// 字 → 讀音（不帶聲調，ü 寫 v，常用的在前）
static TABLE: LazyLock<HashMap<char, Vec<&'static str>>> = LazyLock::new(|| {
    include_str!("../pinyin.tsv")
        .lines()
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| {
            let (c, readings) = line.split_once('\t')?;
            Some((c.chars().next()?, readings.split(' ').filter(|r| !r.is_empty()).collect()))
        })
        .collect()
});

/// 打的字正規化成小寫字母：空白、隔音符號、連字號去掉；有數字或其他符號就不是拼音
pub fn normalize(typed: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(typed.len());
    for &c in typed {
        match c {
            b'a'..=b'z' => out.push(c),
            b'A'..=b'Z' => out.push(c.to_ascii_lowercase()),
            b' ' | b'\'' | b'-' => {}
            _ => return None,
        }
    }
    (!out.is_empty()).then_some(out)
}

/// 讀音的字母 r 對得上打的字母 t（ü 寫成 v，也可以打成 u）
fn same(r: u8, t: u8) -> bool {
    r == t || (r == b'v' && t == b'u')
}

/// a 是 b 的開頭（a、b 一邊是讀音、一邊是打的字，照 same 比）
fn leads(a: &[u8], b: &[u8], reading_first: bool) -> bool {
    a.len() <= b.len() && a.iter().zip(b).all(|(&x, &y)| if reading_first { same(x, y) } else { same(y, x) })
}

/// 一段連續的中文字（每個字的讀音）合不合打的字：從任一字起，每個字打全拼或首字母，最後一個字可以只打開頭幾個字母
fn segment_matches(seg: &[&[&str]], typed: &[u8]) -> bool {
    let n = typed.len();
    // reach[i][p]：比到第 i 個字之前，已經對上打的字的前 p 個字母（從任一字起算，所以每個字的 p = 0 都成立）
    let mut reach = vec![false; (seg.len() + 1) * (n + 1)];
    for i in 0..seg.len() {
        reach[i * (n + 1)] = true;
    }
    for (i, readings) in seg.iter().enumerate() {
        for p in 0..n {
            if !reach[i * (n + 1) + p] {
                continue;
            }
            let rest = &typed[p..];
            for r in readings.iter().map(|r| r.as_bytes()).filter(|r| !r.is_empty()) {
                // 打的字在這個字的讀音中間結束（tiek 的 k、只打一個首字母）
                if leads(rest, r, false) {
                    return true;
                }
                if leads(r, rest, true) {
                    reach[(i + 1) * (n + 1) + p + r.len()] = true;
                }
                if same(r[0], rest[0]) {
                    reach[(i + 1) * (n + 1) + p + 1] = true;
                }
            }
        }
    }
    false
}

/// 中文 zh 的讀音合不合打的字（已正規化）；讀音表沒有的字元（英文、數字、標點）把譯文切成幾段，不跨段比
pub fn matches(zh: &str, typed: &[u8]) -> bool {
    let mut seg: Vec<&[&str]> = Vec::new();
    for c in zh.chars().chain(std::iter::once('\0')) {
        match TABLE.get(&c) {
            Some(readings) => seg.push(readings),
            None => {
                if !seg.is_empty() && segment_matches(&seg, typed) {
                    return true;
                }
                seg.clear();
            }
        }
    }
    false
}

/// 清單項目的譯文：翻好了（翻不出來是 None），或交給背景執行緒還沒翻好
enum Cached {
    Done(Option<Box<str>>),
    Pending,
}

/// 搜尋用的譯文快取：清單項目的英文 → 中文。字典換了（換語言）整張清掉；滿了也整張清
struct Cache {
    dict: usize,
    map: HashMap<Vec<u8>, Cached>,
    /// 這一陣連續比對（間隔不到 GAP）從什麼時候開始、最後一次當場翻譯是什麼時候
    burst: Instant,
    last: Instant,
    worker: Option<Sender<(usize, Vec<u8>)>>,
}
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(|| {
    let now = Instant::now();
    Mutex::new(Cache { dict: 0, map: HashMap::new(), burst: now, last: now, worker: None })
});
/// 傳說模式的歷史人物清單（大要塞 4.6 萬人）打一次字就全部比過一遍，要裝得下
const CACHE_LIMIT: usize = 60_000;
/// 一陣比對當場翻譯最多花多久：傳說模式的人名一個約 0.05 ms，4.6 萬個要 2.5 秒，打英文時也會比到（英文比不到的項目都會再試拼音）。
/// 超過的交給背景執行緒翻，這一次當作比不到，翻好以後下一次打字就比得到
const BURST: Duration = Duration::from_millis(150);
const GAP: Duration = Duration::from_millis(100);

fn translated(dict: &Dict, key: &[u8]) -> Option<Box<str>> {
    crate::ctb::translate(key, 7, dict).0.map(|zh| crate::ctb::strip_colors(&zh).into_boxed_str())
}

/// 背景翻譯的執行緒：收到（字典位址, 英文）就翻好放進快取。字典換下來也不釋放（lib.rs 的 LOADED），位址一直有效
fn spawn_worker() -> Sender<(usize, Vec<u8>)> {
    let (tx, rx) = channel::<(usize, Vec<u8>)>();
    std::thread::spawn(move || {
        for (id, key) in rx {
            if CACHE.lock().dict != id {
                continue;
            }
            let zh = translated(unsafe { &*(id as *const Dict) }, &key);
            let mut cache = CACHE.lock();
            if cache.dict == id {
                cache.map.insert(key, Cached::Done(zh));
            }
        }
    });
    tx
}

/// 清單項目的英文 english 照字典翻成中文後，讀音合不合玩家打的字 typed。第一次比一個項目要翻譯，之後查快取
pub fn search(dict: &Dict, english: &[u8], typed: &[u8]) -> bool {
    let Some(typed) = normalize(typed) else { return false };
    let key = english.trim_ascii();
    if key.is_empty() || key.len() > 512 || !key.iter().any(u8::is_ascii_alphabetic) {
        return false;
    }
    let mut cache = CACHE.lock();
    let id = dict as *const Dict as usize;
    if cache.dict != id {
        cache.map.clear();
        cache.dict = id;
    }
    match cache.map.get(key) {
        Some(Cached::Done(zh)) => return zh.as_deref().is_some_and(|zh| matches(zh, &typed)),
        Some(Cached::Pending) => return false,
        None => {}
    }
    if cache.map.len() >= CACHE_LIMIT {
        cache.map.clear();
    }
    let now = Instant::now();
    if now.duration_since(cache.last) > GAP {
        cache.burst = now;
    }
    if now.duration_since(cache.burst) > BURST {
        cache.map.insert(key.to_vec(), Cached::Pending);
        let tx = cache.worker.get_or_insert_with(spawn_worker).clone();
        drop(cache);
        let _ = tx.send((id, key.to_vec()));
        return false;
    }
    let zh = translated(dict, key);
    let hit = zh.as_deref().is_some_and(|zh| matches(zh, &typed));
    cache.map.insert(key.to_vec(), Cached::Done(zh));
    cache.last = Instant::now();
    hit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_pinyin_initials_and_mixes_match_from_any_syllable() {
        let t = |s: &str| normalize(s.as_bytes()).unwrap();
        assert!(matches("鐵礦", &t("tiekuang")) && matches("铁矿", &t("tiekuang")), "全拼，繁簡都有讀音");
        assert!(matches("鐵礦", &t("tk")), "首字母");
        assert!(matches("鐵礦", &t("tiek")) && matches("鐵礦", &t("tkuang")), "全拼與首字母混打、最後一字打一半");
        assert!(matches("鐵礦", &t("kuang")) && matches("巨型鐵礦", &t("tie kuang")), "從中間的字起、空白不論");
        assert!(!matches("鐵礦", &t("iekuang")), "要從字的開頭打");
        assert!(!matches("鐵礦", &t("tiekuangs")), "多打的字母對不上");
        assert!(matches("銀行", &t("yinhang")) && matches("行走", &t("xingzou")), "多音字每個讀音都算");
        assert!(matches("綠色", &t("lvse")) && matches("綠色", &t("luse")), "ü 打 v 或 u 都可以");
        assert!(!matches("鐵 礦", &t("tk")), "不跨過中文以外的字元");
        assert_eq!(normalize(b"Tie-kuang"), Some(b"tiekuang".to_vec()), "大小寫、連字號不論");
        assert_eq!(normalize(b"tk2"), None, "有數字就不是拼音");
    }
}
