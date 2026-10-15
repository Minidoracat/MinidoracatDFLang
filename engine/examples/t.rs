//! 離線查譯文（翻譯時檢查句型用）：用引擎同一套字典與句型，翻 data/strings、data/rules 的內容。
//!
//!   cargo run --manifest-path engine/Cargo.toml --example t -- zh-Hant "He feels joy while in conflict."
//!
//! 沒給句子時從標準輸入一行讀一句。原文可以含換色標記（`[C:7:0:1]`）與 `[P]`、`[R]`、`[B]`，
//! 照人物頁彩色文字框的方式處理：去掉標記、分段、整段查不到就逐句查。
//! 每句輸出一行：譯文（原文有換色標記時譯文也標出顏色），或「缺：」加查不到的句子（以 ｜ 分隔）。
//! 語言後面加 `-w`：照畫字 hook（addst 等）只查整句字典與句型、不切句，給介面字串做前後比對。
//! 加 `-m`：照段落框（mtb：彈出訊息、說明頁）：整段查不到就在 [B]／[R]／[P] 分段，單空白句界還原成兩個空白後逐句查。
//!   可以點的連結（人名、地名）寫成 `☺名稱☻`：照引擎換成記號再查，譯文裡換回名稱；句型漏掉或重複名稱時算缺。
//! 加 `-s`：照警報清單、提示這類 wrap_text 的段落：整段查不到就逐句查（含引號包住的引言），標記不處理。

use std::io::BufRead;
use std::path::Path;

use dflang::dict::{self, Dict, LINK_CLOSE, LINK_OPEN};
use dflang::{cp437, ctb};

/// data/<kind>/*.tsv（欄位 en, zh-Hant, zh-Hans, align, note）換成引擎讀的「原文<TAB>譯文<TAB>對齊」
fn engine_tsv(dir: &Path, lang: &str) -> String {
    let mut files: Vec<_> = std::fs::read_dir(dir).expect("data 目錄").filter_map(|e| e.ok().map(|e| e.path())).collect();
    files.retain(|p| p.extension().is_some_and(|x| x == "tsv"));
    files.sort();
    let mut out = String::new();
    for path in files {
        let content = std::fs::read_to_string(&path).expect("UTF-8 TSV");
        let mut lines = content.lines();
        let header: Vec<&str> = lines.next().unwrap_or("").split('\t').collect();
        let col = |name: &str| header.iter().position(|h| *h == name);
        let (Some(en), Some(zh), Some(align)) = (col("en"), col(lang), col("align")) else { continue };
        for line in lines {
            let cells: Vec<&str> = line.split('\t').collect();
            let get = |i: usize| cells.get(i).copied().unwrap_or("");
            if !get(zh).trim().is_empty() {
                out.push_str(&format!("{}\t{}\t{}\n", get(en), get(zh), get(align)));
            }
        }
    }
    out
}

/// 樣本裡的 `☺名稱☻` 換成引擎的連結記號 `☺序號☻`（見 dict::LINK_OPEN）：回傳記號化的原文與名稱。
fn link_tokens(raw: &[u8]) -> (Vec<u8>, Vec<String>) {
    let mut key = Vec::with_capacity(raw.len());
    let mut names = Vec::new();
    let mut rest = raw;
    while let Some(i) = rest.iter().position(|&c| c == LINK_OPEN) {
        let Some(j) = rest[i..].iter().position(|&c| c == LINK_CLOSE) else { break };
        key.extend_from_slice(&rest[..i]);
        key.push(LINK_OPEN);
        key.extend_from_slice(names.len().to_string().as_bytes());
        key.push(LINK_CLOSE);
        names.push(cp437::decode(&rest[i + 1..i + j]));
        rest = &rest[i + j + 1..];
    }
    key.extend_from_slice(rest);
    (key, names)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(lang) = args.first().filter(|l| *l == "zh-Hant" || *l == "zh-Hans").cloned() else {
        eprintln!("用法：t zh-Hant|zh-Hans [-w|-m|-s] [原文...]（沒給原文就從標準輸入讀）");
        std::process::exit(2);
    };
    let whole = args.get(1).is_some_and(|a| a == "-w");
    let para = args.get(1).is_some_and(|a| a == "-m");
    let wrap = args.get(1).is_some_and(|a| a == "-s");
    let args: Vec<String> = args.into_iter().filter(|a| a != "-w" && a != "-m" && a != "-s").collect();
    let data = Path::new(env!("CARGO_MANIFEST_DIR")).join("../data");
    let mut dict = Dict::default();
    dict.load_tsv(&engine_tsv(&data.join("strings"), &lang));
    let report = dict.load_rules_tsv(&engine_tsv(&data.join("rules"), &lang));
    if report.bad_source > 0 {
        eprintln!("警告：{} 條句型寫法不合規定，沒有載入", report.bad_source);
    }
    let texts: Vec<String> = if args.len() > 1 {
        args[1..].to_vec()
    } else {
        std::io::stdin().lock().lines().map_while(Result::ok).collect()
    };
    for text in texts {
        let Some(raw) = cp437::encode(&text) else {
            println!("缺：（原文有 CP437 以外的字）{text}");
            continue;
        };
        if para {
            let (key, names) = link_tokens(&raw);
            let named = |t: String| names.iter().enumerate().fold(t, |t, (k, n)| t.replace(&format!("☺{k}☻"), &format!("☺{n}☻")));
            match dict.translate_paragraph(&key) {
                (None, _) => println!("缺："),
                (Some(t), _) if dict::link_order(&t, names.len()).is_none() => println!("缺：連結名稱漏掉或重複：{t}"),
                (Some(t), false) => println!("缺：有幾段照原文：{}", named(t)),
                (Some(t), true) => println!("{}", named(t)),
            }
            continue;
        }
        if whole {
            println!("{}", dict.translate_text(&raw).unwrap_or_else(|| "缺：".to_owned()));
            continue;
        }
        if wrap {
            match dict.translate_sentences(&raw) {
                Ok(p) => println!("{}", p.into_iter().map(|(t, _)| t).collect::<String>()),
                Err(missing) => println!("缺：{}", missing.iter().map(|m| cp437::decode(m)).collect::<Vec<_>>().join("｜")),
            }
            continue;
        }
        match ctb::translate(&raw, 7, &dict) {
            Ok(zh) if text.contains("[C:") => println!("{zh}"),
            Ok(zh) => println!("{}", ctb::strip_colors(&zh)),
            Err(missing) => {
                let missing: Vec<String> = missing.iter().map(|m| cp437::decode(m)).collect();
                println!("缺：{}", missing.join("｜"));
            }
        }
    }
}
