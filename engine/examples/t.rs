//! 離線查譯文（翻譯時檢查句型用）：用引擎同一套字典與句型，翻 data/strings、data/rules 的內容。
//!
//!   cargo run --manifest-path engine/Cargo.toml --example t -- zh-Hant "He feels joy while in conflict."
//!
//! 沒給句子時從標準輸入一行讀一句。原文可以含換色標記（`[C:7:0:1]`）與 `[P]`、`[R]`、`[B]`，
//! 照人物頁彩色文字框的方式處理：去掉標記、分段、整段查不到就逐句查。
//! 每句輸出一行：譯文（原文有換色標記時譯文也標出顏色），或「缺：」加查不到的句子（以 ｜ 分隔）。
//! 語言後面加 `-w`：照畫字 hook（addst 等）與彈出訊息只查整句字典與句型、不切句，給介面字串做前後比對。
//! 加 `-s`：照警報清單、提示這類 wrap_text 的段落：整段查不到就逐句查（含引號包住的引言），標記不處理。

use std::io::BufRead;
use std::path::Path;

use dflang::{cp437, ctb, dict::Dict};

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

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(lang) = args.first().filter(|l| *l == "zh-Hant" || *l == "zh-Hans").cloned() else {
        eprintln!("用法：t zh-Hant|zh-Hans [-w|-s] [原文...]（沒給原文就從標準輸入讀）");
        std::process::exit(2);
    };
    let whole = args.get(1).is_some_and(|a| a == "-w");
    let wrap = args.get(1).is_some_and(|a| a == "-s");
    let args: Vec<String> = args.into_iter().filter(|a| a != "-w" && a != "-s").collect();
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
