# 致謝與來源

## 遊戲

《Dwarf Fortress》（矮人要塞）版權屬 Bay 12 Games，發行商 Kitfox Games。本 MOD 不含任何遊戲素材。

## 平台

- [DFHack](https://github.com/DFHack/dfhack)（Zlib License）：遊戲啟動時執行本 MOD 的 Lua 載入器；載入器透過 DFHack 的結構定義取得畫面緩衝區欄位位置，並以 DFHack 的貼圖模組註冊字圖。整句貼圖依 df-structures 的 renderer 結構與 vtable 槽位找到遊戲畫格子的函式。
- 載入器覆寫 DFHack 分頁元件的繪製（`lua/loader.lua` 的 `install_tab`）：照 DFHack `gui/widgets/tab_bar.lua` 的 `Tab:onRenderBody` 用同樣的分頁框線畫筆畫兩列，換上中文標籤。
- 反編譯研究使用 DFHack `symbols.xml` 的全域變數與 vtable 位址標註函式。

## 程式庫

`dflang.dll` 靜態連結的開源 Rust 套件，主要有：字型解析與 hinting 的 skrifa、read-fonts、font-types（Google Fonts fontations）、點陣化的 zeno、PNG 編碼的 png、函式 hook 的 retour（BSD 2-Clause，內含 udis86 反組譯器，BSD 2-Clause）、parking_lot；其餘多為 MIT／Apache-2.0。完整清單與各自的授權全文見包內 `dflang/THIRD-PARTY-LICENSES.txt`（由 `scripts/build.py` 依 `cargo metadata` 產生）。

## 字型

- 思源黑體（Source Han Sans）的 Google 發行版 Noto Sans CJK TC／SC Bold、Regular，© Adobe，SIL Open Font License 1.1。
- 工作坊包內附的字型（`dflang/fonts/`）是只留本包用字的子集；遊戲啟動時引擎依當下文字格大小把它畫成字圖集。授權全文見 `dflang/LICENSE-fonts.txt`。

## 吉祥物

- 標題畫面左下角的貓娘是 Minidoracat 的角色，以 AI 影像模型依原有角色設定繪製成 Q 版（`art/mascot.png`、包內 `dflang/mascot.png`）。© Minidoracat，保留所有權利。

## 翻譯參考

- [矮人要塞中文維基](https://dfzh.huijiwiki.com/)翻譯組的簡體中文翻譯資料（CC BY-NC 4.0）。本 MOD 的譯文逐句自行翻譯；術語選擇參考其譯名的條目，在 `glossary/terms.tsv` 的 `ref` 欄標為 `dfzh-wiki`。本 MOD 未收錄、也不散布其資料。

## 研究參考

- [DFI18n](https://github.com/DFI18n/dfi18n)（Viz，MIT License）及其 [PR #2](https://github.com/DFI18n/dfi18n/pull/2)（CooperZhuang）：遊戲文字繪製函式（`addst` 等）的身分與定位方式，最初對照其公開的說明與特徵碼確認。本 MOD 的引擎程式碼與特徵碼（`engine/patterns.tsv`，由 `scripts/make_patterns.py` 從遊戲執行檔產生）為自行撰寫，未包含其程式碼。
- DFCN（Steam 工作坊 3811193379）：整句貼圖的構想（不受遊戲的文字格限制，用自己的字型把整句畫成貼圖）參考它設定檔（`config.ini`）說明的做法。本 MOD 的畫法（換遊戲 renderer 的 vtable，格子畫完後疊上貼圖）、排版與對齊的程式碼為自行撰寫，未包含它的程式碼。
