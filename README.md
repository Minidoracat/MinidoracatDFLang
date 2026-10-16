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
  - 第十三輪：世界生成的名稱。引擎攔下遊戲組名字的函式，用 2,196 字的中文字表照同樣的結構組中文名（「The Planet of Destiny」→「命運之星」、堡壘名「Scorchaxe」→「焦斧」），原名照原文。
  - 第十四輪：傳說模式。歷史事件句（「1年仲夏，Onsmost在盜根攻擊了收割者之夢魘。」，含事件原因與日期的各種組合）、人物／勢力／據點／地區頁開頭的說明與相關清單、生成生物（遺忘巨獸、泰坦、夜行生物、天使、實驗體）的稱呼與說明、傳說介面（分類、清單、分頁標籤、載入進度）；被截短成「…」的分頁標籤也翻。
  - 第十五輪（一）：冒險模式。開局各分頁（命運、背景與宗教、屬性與技能、外觀、性格、裝備、坐騎）、教學與說明指南、資訊列、戰鬥與合成選單、物品欄、交談選單（關鍵字照英文，搜尋框才找得到）、交談紀錄的講者、日誌與各分頁的說明、程序生成的職位名（「消防專員Medi Wuspindothan」）；遊戲截成「…」的清單字串改用截短前的原文翻（「與人類男爵Pestrat Lideadis展開新交談」）。
  - 第十六輪：DFHack 自己的視窗與 overlay：控制面板、launcher、快捷鍵選單、快速入門指南、確認框、建築規劃、商隊與交易、生物清單的排序與篩選、工作指令、quickfort 與藍圖，以及 gui/ 底下的工具視窗（共 163 個 Lua 檔），另有 528 個工具簡介；指令與工具名照英文。引擎在 Lua 攔 DFHack 的畫字，說明段落依中文重新換行。
  - 第十五輪（二）：冒險模式的交談內容。玩家說的話與 NPC 的回答（問候、近況、統治者、麻煩與紛爭、傳聞、委託與求職、商人推銷、陰謀、價值觀與看法、預言）；交談紀錄由遊戲自己切行，引擎接手訊息框與交談畫面的這兩處，整則逐句翻、依中文重新換行（「男爵Pestrat Lideadis：你好。見到你真好。大業萬歲！一個人在荒野裡闖蕩？！你明明知道不該這樣。」）。
  - 第十七輪：設定的按鍵分頁（要塞、冒險、建築、文字輸入、巨集的按鍵用途；按鍵名照英文）。
  - 第十八輪：已翻畫面裡之前沒走到的分支：常規命令切換鈕的其他狀態、場所的開放對象與等級、難度設定、人物頁的人際關係、物品與建築頁、放置建築的提示、協議標題。
  - 第十九輪：建立世界的詳細模式（進階參數 200 多項）、世界生成被拒絕時的說明與拒絕原因、標題畫面的模組管理員、競技場與存檔清單。
  - 第二十輪：全面補漏，依函式位址分 24 批掃執行檔裡還翻不出來、會畫在畫面上的字串（競技場、天氣選單、地形與樹木名稱、建造錯誤、房間等級、軍事排程、外交對話、陰謀、交易、擊殺紀錄、死因短語、舞蹈形式、商人說的話等），另寫 40 條依畫面譯文。
  - 第二十一輪：書名。作品的英文書名由樣板、主題與 raws 字表的隨機字組成，引擎在畫的時候拆回去翻（書名用詞 1,045 條、書名樣板 216 條）；名稱表還沒登記的名字在書名裡照英文，遊戲組過那個名字後換成中文。
  - 第二十二輪：冒險模式實機補漏（商人以物易物與叫賣、交談紀錄裡小寫的「種族 職業 名字」、寵物互動公告），天選者開場的神明台詞，以及世界生成的神造材料與神話物質名稱（470 條）。
  - 第二十三輪：上架前收尾。簡中實機抽查主要畫面（標題、設定、建立世界、出發準備、要塞、傳說、冒險、競技場）；中文字改小一號並對齊英文字身，分頁標籤這類下移半格的文字照英文位置畫，出發準備畫面的分頁標籤不再壓到下緣；補世界生成參數組名稱、工作名「Fish」（釣魚）、勞動分頁排序鈕；簡中引號不放行首；工作坊說明改寫。
  - 第二十四輪：標題畫面存檔清單的世界名、堡壘名（存檔只存英文，引擎把英文拆回名稱字表組中文）、交談畫面頭像下每個字都大寫的稱呼（「哥布林猛獸獵人 名字」）、攻擊選單的無名生物（「洞穴鱷魚」）。
  - 第二十五輪（0.2.0）：整句貼圖。畫面上的字改由引擎照字型的實際字寬整行畫成貼圖（中文不再一字佔兩格，英數也用同一套字型），換解析度、介面縮放後照當下格子大小重畫、不會糊；段落照字寬重新換行、填滿框寬；按鈕、說明視窗標題、分頁標籤依遊戲擺英文的位置置中或靠左；出發準備畫面的分頁標籤、只有一列高的排序按鈕也照字寬畫；DFHack 讀畫面找英文字定位的疊加介面（人物頁的心情）照樣找得到位置。同時裝 DFCN 時說明改成中文、可一鍵停用，字型換成粗體。
  - 第二十六輪（0.3.0）：`Ctrl-Shift-L` 開「DFLang 設定」視窗（標題畫面點吉祥物或名牌下方的提示也開）：暫時改看英文、切換繁簡、整句貼圖、中文字身（預設改大一號，13～16 可調）、粗體或一般粗細、未翻譯紀錄（預設關、跨次啟動去重、有上限）；`dflang` 指令的輸出在遊戲裡的 launcher 顯示中文。補木匠工坊的陷阱零件、工坊的「[需要…]」紅字、工匠工坊的象牙／牙質任務，並照遊戲組工作名的方式把所有材料類別盤點一次。修正 `dflang off` 關掉翻譯後記憶體一直增加。
  - 0.3.1：設定視窗標題顯示版本號（例「DFLang 設定 v0.3.1」），回報問題時請一起附上。
  - 整句 44,147 條、句型 25,344 條（其中 10,395 條由事件句樣板產生）、依畫面譯文 319 條、名稱字表 3,301 條，繁簡分開翻。
- 已發布到 Steam 工作坊（0.3.1，2026-10-17）：[繁體中文](https://steamcommunity.com/sharedfiles/filedetails/?id=3820630135)、[简体中文](https://steamcommunity.com/sharedfiles/filedetails/?id=3820630318)。

## 玩家安裝

1. Steam 安裝 [DFHack](https://store.steampowered.com/app/2346660/)（免費）。
2. 訂閱[繁中](https://steamcommunity.com/sharedfiles/filedetails/?id=3820630135)或[簡中](https://steamcommunity.com/sharedfiles/filedetails/?id=3820630318)項目，正常啟動遊戲。
3. 兩個都訂閱時預設繁中；按 `Ctrl-Shift-L` 打開「DFLang 設定」視窗切換語言，重開遊戲生效（也可以在 DFHack 主控台輸入 `dflang lang zh-Hans` 或 `dflang lang zh-Hant`）。

遊戲中任何畫面按 `Ctrl-Shift-L`（或在標題畫面點吉祥物）開「DFLang 設定」視窗，再按一次關掉：視窗標題顯示目前的版本號（例「DFLang 設定 v0.3.1」，回報問題時請一起附上），顯示引擎狀態，可以暫時改看英文、切換語言、開關整句貼圖、調整中文字身、切換粗體字、開關未翻譯紀錄。語言、整句貼圖、字身、粗體字重開遊戲才生效，改了視窗會以黃字提示。快捷鍵只在遊戲執行時註冊，不寫進 DFHack 的設定檔；想換鍵用 DFHack 的 `gui/keybinds`。原本的 `dflang` 指令照樣能用（`dflang` 不加參數看用法）。

翻譯引擎一次只能有一個在運作。同時裝了 DFCN 或 DFI18n 時，本 MOD 讓位，標題畫面跳出中文說明讓你選要用哪一個，重開遊戲生效：

- 選「關閉 DFI18n」：在遊戲的 `mods` 資料夾放一個 `dflang-hide-dfi18n`（ID 和 DFI18n 相同、版本較高，DFHack 就不載入 DFI18n；DFI18n 本身的檔案不動）。遊戲裡找不到本 MOD 時，這個資料夾會在下次啟動時自己刪掉。
- 選「關閉 DFCN」：把遊戲目錄的 `dfhooks_dfcn.ini`（舊版 DFCN 是 `dfhooks_dfcn.dll`）改名成加上 `.disabled-by-dflang` 的檔名，dfhooks 就不載入 DFCN。退訂本 MOD 前想用回 DFCN，先輸入 `dflang lang off`，或自己把檔名改回來。
- 選「繼續用」，或之後在 DFHack 主控台輸入 `dflang lang off`：改用另一個翻譯 MOD，之前停用的會恢復。偵測不到其他翻譯 MOD 時，本 MOD 照常啟用。

裝了 dfzh 時本 MOD 不啟動，標題畫面用英文說明怎麼移除。`dflang status` 查看狀態。

畫面上的字預設由本 MOD 照字型實際字寬整行畫（整句貼圖）。想改回一字兩格的字圖（例如和別的畫面 MOD 衝突時），在設定視窗（`Ctrl-Shift-L`）關掉「整句貼圖」或在 DFHack 主控台輸入 `dflang overlay off`，重開遊戲生效；`dflang overlay on` 改回來。中文字的大小跟著遊戲的介面縮放；想只調整中文，在設定視窗或用 `dflang overlay size 15`（13～16，格高的十六分之幾，預設 14），重開遊戲生效，數字越大相鄰兩列的字越靠近。字預設是思源黑體粗體（Bold）；覺得太粗，在設定視窗關掉「粗體字」或輸入 `dflang bold off` 改用一般粗細（Regular），重開遊戲生效，`dflang bold on` 改回來。整句貼圖和一字兩格的字圖都照這個設定，標題畫面吉祥物的名牌一律是粗體。

標題畫面左下角會出現本 MOD 的貓娘吉祥物和「繁體中文／简体中文」名牌，名牌下方提示設定視窗的快捷鍵（`Ctrl-Shift-L：中文化設定`），點吉祥物或提示都會開設定視窗。要移動或關掉，在 DFHack 主控台輸入 `gui/overlay`，找 `dflang_zh_hant.mascot`、`dflang_zh_hant.hint`（簡中包是 `dflang_zh_hans.*`）。

回報沒翻到的字：在設定視窗打開「記錄未翻譯的字」（預設關；立即生效，之後每次啟動都會繼續記錄，直到關掉），照平常玩到沒翻的畫面，再按「開啟紀錄所在資料夾」找到遊戲目錄的 `dfhack-config/dflang-misses.tsv`，附到 [GitHub Issues](https://github.com/Minidoracat/MinidoracatDFLang/issues)，或把內容貼到工作坊的「問題回報」討論串（[繁中](https://steamcommunity.com/workshop/filedetails/discussion/3820630135/574929940135291802/)、[簡中](https://steamcommunity.com/workshop/filedetails/discussion/3820630318/574929940135291861/)；討論串不能附檔，內容太長時貼相關畫面的幾行）。紀錄會記下畫面上所有沒翻到的英文，包括你世界裡的名字；回報完可以按「清除紀錄」。

## 運作方式

```mermaid
flowchart LR
  A[DFHack 啟動] --> B[scripts_modinstalled/dflang_zh_hant.lua]
  B --> C[package.loadlib dflang/dflang.dll]
  B --> D[用字型依目前文字格大小畫字圖集<br>註冊成遊戲貼圖]
  C --> E[特徵碼找到 DF 的四個畫字函式、錨定貼圖、段落排版與切行函式並 hook]
  E --> F{依畫面譯文、整句字典或句型規則有這句？}
  F -->|是| G[讓遊戲畫替身空白<br>版面、點擊範圍照英文]
  F -->|否| H[照原樣記下英文]
  G --> I[每幀畫完格子後<br>整句貼圖照字型字寬整行畫上]
  H --> I
```

- DF 本身不載入 MOD 的原生程式；DFHack 會執行每個已安裝 MOD 的 `scripts_modinstalled/`，引擎就從這裡以 Lua `package.loadlib` 載入，只用 Lua C API，不綁 DFHack 版本。
- 畫面上的字由引擎畫（整句貼圖）：遊戲的格子裡照英文的版面寫替身空白，渲染器每幀畫完格子後，引擎把每一行字照思源黑體（Noto Sans CJK Bold）的實際字寬整行畫成貼圖疊上去，用 SDL 照當下的格子大小畫，調整視窗、介面縮放也清楚。對齊照遊戲擺英文的位置（按鈕、視窗標題置中，清單靠左）。`dflang overlay off` 改回舊的畫法：每個中文字佔兩格、遊戲自己畫字圖集的字圖（字圖集在遊戲啟動時依當下的文字格大小畫成，縮放後會糊）。
- 遊戲執行時拼出的句子（名字、數字、隨機字詞）用句型規則翻譯，例如 `Histories of {1} and {2}` →「{1}與{2}的歷史」。同一個英文在不同畫面意思不同時，用只在該畫面套用的依畫面譯文。
- 名稱（生物、植物、材料、物品）各自翻好，遊戲拼出的物品名由句型接起來；遊戲把名稱改成大寫（`Stray One-humped Camel`）時，整句查不到就改查全部小寫的寫法。
- 自動換行的段落（說明、教學、彈出視窗）與提示框整句查字典、依中文規則重新換行，畫字時才換成中文；遊戲資料與存檔維持英文，移除本 MOD 不留痕跡。
- 函式位置用 `engine/patterns.tsv` 的特徵碼尋找，由 `scripts/make_patterns.py` 從遊戲執行檔產生；遊戲改版時重產即可。
- 標題畫面的吉祥物是 DFHack overlay 小工具：引擎啟動時把 `art/mascot.png` 縮成當下的格子大小、疊上名牌，畫在畫面頂層，透明處看得到標題背景。

## 目錄

```
data/strings/*.tsv         整句譯文：en, zh-Hant, zh-Hans, align, note（規則見 docs/translation-guide.md）
data/rules/*.tsv           句型規則：同上，en 含 {1}～{9} 佔位
data/events/*.tsv          傳說事件句樣板：scripts/gen_events.py 展開成 data/rules/_events.tsv、zz_events.tsv（產物，別手改）
data/names/*.tsv           世界生成名稱的中文字表（字的代號、詞性、中文）
data/screens/*.tsv         依畫面譯文：檔名是 DFHack 焦點字串的開頭，只在該畫面套用
glossary/terms.tsv         術語表；locale.tsv 兩岸用語規則
art/mascot.png             標題畫面吉祥物（透明背景 RGBA PNG；名牌文字由引擎在遊戲裡畫）
engine/                    Rust 引擎（cdylib dflang.dll）與特徵碼 patterns.tsv
lua/                       DFHack 載入器範本與 dflang 指令
scripts/
  build.py                 組出 dist/ 下的兩個工作坊包（含編譯引擎、裁字型）
  gen_events.py            由 data/events 產生事件句的句型（日期、原因的各種組合）
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
uv run scripts/gen_events.py                  # 改過 data/events 後重產事件句型
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

發布（要先取得維護者同意；版本號在 `scripts/workshop.json`）：

```bash
uv run scripts/publish_workshop.py --item zh-Hant --note "繁中更新說明" [--screenshots] --yes   # 簡中另跑一次 --item zh-Hans
uv run scripts/publish_workshop.py --item all --description-only --yes                     # 只改了 docs/steam/*.bbcode：只換說明，不算發布新版
git tag -a v0.3.1 -m "0.3.1" && git push origin v0.3.1                                    # 每次發布都打 tag，版本號照 workshop.json
```

## 授權

- 譯文與術語表（`data/`、`glossary/`）：[CC BY 4.0](LICENSE-CC-BY-4.0.txt)。轉載或改作請註明出處。
- 其餘程式與文件：[MIT](LICENSE)。
- 吉祥物圖（`art/`）與工作坊封面（`docs/steam/cover-*.gif`）：© Minidoracat，保留所有權利，不在上述授權範圍內。
- 字型：思源黑體（Noto Sans CJK TC／SC Bold、Regular），SIL Open Font License 1.1，工作坊包內附授權全文。
- 引擎靜態連結的開源 Rust 套件（skrifa、zeno、png、retour 等）各依原授權，包內 `dflang/THIRD-PARTY-LICENSES.txt` 附授權全文（建置時產生）。
- 致謝與參考來源見 [CREDITS.md](CREDITS.md)。
