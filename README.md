# MinidoracatDFLang

《矮人要塞》（Dwarf Fortress，Steam 版）的繁體中文與簡體中文化。翻譯引擎、字典與字型都是本專案自己的，繁中、簡中各發布成一個工作坊項目。

## 現況

- 遊戲版本：Dwarf Fortress 53.16＋DFHack 53.16-r2。
- 已翻譯：
  - 第一輪：新玩家流程（標題、設定五個分頁、建立世界與生成、選出發地點、出發準備、進入要塞的上方資訊列與資訊面板分頁），含自動換行的段落（歡迎視窗、提示、抵達要塞）。
  - 第二輪：要塞模式的資訊面板（生物、任務、設施、勞動、工作指令、貴族、物品、司法）、建造／區域／倉儲區／活動區選單、人物頁、世界畫面、存讀檔畫面，以及說明選單裡的互動教學、全部指南與各面板的說明彈窗。
  - 第三輪：工具列與各選單的提示框（約 575 句）、職業與職位名稱、「人名，職業」與「技能等級＋技能」句型、人物頁的性格、價值觀、健康狀態與需求標籤。
  - 第四輪：生物、植物、材料、物品、身體部位等名稱約 6,900 條，以及把名稱接起來的句型（「銅戰斧」「花豹皮革」「5 肉盔菇菌種」「流浪貓，♀ (馴服)」）；讀檔、存檔與建立世界的進度訊息。
  - 第五輪：工作名稱（生物清單的目前工作、工坊任務，「製作 4 個花崗岩磚塊」「鍛造一雙鐵護手」）、地圖游標的地形說明（「磨平的燧石地板」「茂密的針茅上坡」）、建築資訊頁與人物想法的情緒詞。
  - 第六輪：人物頁的整段文字——想法（「他和某人交談時感到共鳴。」「她經歷創傷後悲痛不已。」）、記憶、性格的特質、價值觀、偏好、需求、身心屬性與習慣動作，顏色照原文；訪客說明；工作名稱「喝」與資訊列「酒類」分開。
  - 第七輪：要塞的公告與彈出訊息（取消工作、死亡、移民、商隊、天氣、季節、婚禮、職位繼承、巨獸來襲…，「Rigòth Nildum，木工取消了「喝」：被禿鷲打斷。」）、人物說的話（概況引言「「戰況正激烈……我太開心了！」」、交談公告），以及冒險模式的訊息列。
  - 第八輪：戰鬥報告——攻擊、命中部位與傷勢、閃避、格擋、摔角、衝鋒、動脈與肌腱受傷（「流浪狗咬了禿鷲的左小腿，撕裂了肌肉！」「伐木工用右前臂鎖住了禿鷲的右髖部！」）。
  - 第九輪：人物頁健康分頁——狀態、傷勢、治療、病歷（「無法站立」「右大腿，皮膚」）與描述：生物說明（全部 951 條，「矮小結實的生物，愛喝酒，也愛做工。」）、身體屬性（「他很少生病，但容易疲勞，而且相當笨拙。」，好壞分綠紅兩色）、外觀與傷疤（「他的頭髮編成了雙辮。」「他黃銅色的眼睛很圓。」）。
  - 第十輪：設定的公告種類清單（「（戰鬥）噁心」「撞上障礙物（粉碎）」）、工作分組的勞動清單（「木材加工」「石面雕刻」）、世界地圖的派遣與退休／放棄要塞的確認視窗、工作物品條件（「需要 1 個空的可存放食物的桶」）、司法的審訊紀錄與陰謀談判、墓碑銘文與傳說的死因、冒險模式的名聲與交談話題、表演與狩獵的評論。
  - 第十一輪：音樂、詩歌、舞蹈形式的說明（「這種音樂演奏時不偏好特定音階，採用自由節奏。」）、樂器說明、學者的研究活動（「思索…」「講授…」）。
  - 第十二輪：物品頁的說明（「這是Besmar Avuztaron製作的大師級巨型蝙蝠皮革長褲。物品是沙灰褐色，以窩杯染料和蘋果葉染料混合染成。」）、裝飾與藝術圖像、書寫內容、物品頁標籤（主人、重量）；引擎支援傳說模式的連結（人名、地名照中文排版，點得到）。
  - 整句 23,055 條、句型 8,852 條、依畫面譯文 5 條，繁簡分開翻。
- 尚未發布到 Steam 工作坊；翻譯接近完成後才發布。上架前還要完成：冒險模式、傳說模式的歷史事件句、世界生成的名稱（地名、文明名、書名）、DFHack 視窗文字。

## 玩家安裝（發布後）

1. Steam 安裝 [DFHack](https://store.steampowered.com/app/2346660/)（免費）。
2. 訂閱繁中或簡中項目，正常啟動遊戲。
3. 兩個都訂閱時預設繁中；在 DFHack 主控台輸入 `dflang lang zh-Hans` 或 `dflang lang zh-Hant` 切換，重開遊戲生效。

不能和 DFI18n、dfzh 等其他翻譯引擎同時使用，偵測到時本 MOD 不啟動。`dflang status` 查看狀態。

標題畫面左下角會出現本 MOD 的貓娘吉祥物和「繁體中文／简体中文」名牌。要移動或關掉，在 DFHack 主控台輸入 `gui/overlay`，找 `dflang_zh_hant.mascot`（簡中包是 `dflang_zh_hans.mascot`）。

## 運作方式

```mermaid
flowchart LR
  A[DFHack 啟動] --> B[scripts_modinstalled/dflang_zh_hant.lua]
  B --> C[package.loadlib dflang/dflang.dll]
  B --> D[用字型依目前文字格大小畫字圖集<br>註冊成遊戲貼圖]
  C --> E[特徵碼找到 DF 的四個畫字函式、錨定貼圖、段落排版與切行函式並 hook]
  E --> F{依畫面譯文、整句字典或句型規則有這句？}
  F -->|是| G[讓遊戲畫等寬空白<br>再把中文字圖放進那些格子]
  F -->|否| H[照原樣畫英文]
```

- DF 本身不載入 MOD 的原生程式；DFHack 會執行每個已安裝 MOD 的 `scripts_modinstalled/`，引擎就從這裡以 Lua `package.loadlib` 載入，只用 Lua C API，不綁 DFHack 版本。
- 中文字由遊戲自己的繪圖流程畫出：每個中文字佔兩格，字圖在遊戲啟動時依當下的文字格大小從思源黑體（Noto Sans CJK Medium）畫成，套用字型的 hinting，縮放不會糊。
- 遊戲執行時拼出的句子（名字、數字、隨機字詞）用句型規則翻譯，例如 `Histories of {1} and {2}` →「{1}與{2}的歷史」。同一個英文在不同畫面意思不同時，用只在該畫面套用的依畫面譯文。
- 名稱（生物、植物、材料、物品）各自翻好，遊戲拼出的物品名由句型接起來；遊戲把名稱改成大寫（`Stray One-humped Camel`）時，整句查不到就改查全部小寫的寫法。
- 自動換行的段落（說明、教學、彈出視窗）與提示框整句查字典、依中文規則重新換行，畫字時才換成中文；遊戲資料與存檔維持英文，移除本 MOD 不留痕跡。
- 函式位置用 `engine/patterns.tsv` 的特徵碼尋找，由 `scripts/make_patterns.py` 從遊戲執行檔產生；遊戲改版時重產即可。
- 標題畫面的吉祥物是 DFHack overlay 小工具：引擎啟動時把 `art/mascot.png` 縮成當下的格子大小、疊上名牌，畫在畫面頂層，透明處看得到標題背景。

## 目錄

```
data/strings/*.tsv         整句譯文：en, zh-Hant, zh-Hans, align, note（規則見 docs/translation-guide.md）
data/rules/*.tsv           句型規則：同上，en 含 {1}～{9} 佔位
data/screens/*.tsv         依畫面譯文：檔名是 DFHack 焦點字串的開頭，只在該畫面套用
glossary/terms.tsv         術語表；locale.tsv 兩岸用語規則
art/mascot.png             標題畫面吉祥物（透明背景 RGBA PNG；名牌文字由引擎在遊戲裡畫）
engine/                    Rust 引擎（cdylib dflang.dll）與特徵碼 patterns.tsv
lua/                       DFHack 載入器範本與 dflang 指令
scripts/
  build.py                 組出 dist/ 下的兩個工作坊包（含編譯引擎、裁字型）
  verify_mod.py            發版前驗證閘門
  terms.py                 術語一致與兩岸用語檢查
  lookup.py                查一句原文的現有譯文、術語、參考譯名與遊戲字串
  inventory.py             全文盤點：分類全部遊戲文字、對照實機紀錄，排出待翻清單（temp/inventory/）
  dev_game.py              本機實機測試：掛包、啟動、下指令、還原
  publish_workshop.py      兩包各自上傳工作坊（預設只檢查）
  make_patterns.py         產生／驗證特徵碼
  df_snapshot.py           遊戲文字盤點與 Ghidra 反編譯快照（產物在 repo 外，不公開）
  workshop.json            兩個工作坊項目的設定
docs/                      分析、翻譯指南、工作坊說明文
dist/                      建置產物（不進 git）
```

## 開發

需要 Windows、[uv](https://docs.astral.sh/uv/)、Rust（stable，MSVC 工具鏈）、Steam 版 Dwarf Fortress 與 DFHack。遊戲與 DFHack 位置從 Steam 自動偵測，可用環境變數 `DF_PATH`、`DFHACK_PATH` 指定。

```bash
uv run scripts/build.py --fetch-fonts         # 首次：下載思源黑體到 temp/fonts/
uv run scripts/build.py                       # 建置兩包（遊戲要先關：執行中會鎖住 DLL）
uv run scripts/verify_mod.py                  # 必須全部 PASS
uv run scripts/terms.py check                 # 術語與兩岸用語
cargo test --manifest-path engine/Cargo.toml --lib   # 句型比對的單元測試
uv run scripts/inventory.py                   # 待翻清單（需要反編譯快照）
uv run scripts/dev_game.py up --lang both     # 掛進遊戲並啟動
uv run scripts/dev_game.py run dflang status
uv run scripts/dev_game.py down               # 關遊戲、還原 mods/ 與設定
```

遊戲更新後先跑 `uv run scripts/make_patterns.py --check`。特徵碼找不到函式時引擎不啟動、遊戲維持英文，DFHack 主控台顯示 `[DFLang zh-Hant] addst 的特徵碼命中 0 次，這版遊戲可能還不支援`。重新定位的步驟見 [docs/analysis.md](docs/analysis.md) 的「遊戲更新」。

## 授權

- 譯文與術語表（`data/`、`glossary/`）：[CC BY 4.0](LICENSE-CC-BY-4.0.txt)。轉載或改作請註明出處。
- 其餘程式與文件：[MIT](LICENSE)。
- 吉祥物圖（`art/`）：© Minidoracat，保留所有權利，不在上述授權範圍內。
- 字型：思源黑體（Noto Sans CJK TC／SC Medium），SIL Open Font License 1.1，工作坊包內附授權全文。
- 致謝與參考來源見 [CREDITS.md](CREDITS.md)。
