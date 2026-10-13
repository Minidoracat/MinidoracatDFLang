# 致謝與來源

## 遊戲

《Dwarf Fortress》（矮人要塞）版權屬 Bay 12 Games，發行商 Kitfox Games。本 MOD 不含任何遊戲素材。

## 平台

- [DFHack](https://github.com/DFHack/dfhack)（Zlib License）：遊戲啟動時執行本 MOD 的 Lua 載入器；載入器透過 DFHack 的結構定義取得畫面緩衝區欄位位置，並以 DFHack 的貼圖模組註冊字圖。
- 反編譯研究使用 DFHack `symbols.xml` 的全域變數與 vtable 位址標註函式。

## 字型

- 思源黑體（Source Han Sans）的 Google 發行版 Noto Sans CJK TC／SC Medium，© Adobe，SIL Open Font License 1.1。
- 工作坊包內附的字型（`dflang/fonts/`）是只留本包用字的子集；遊戲啟動時引擎依當下文字格大小把它畫成字圖集。授權全文見 `dflang/LICENSE-fonts.txt`。

## 翻譯參考

- [矮人要塞中文維基](https://dfzh.huijiwiki.com/)翻譯組的簡體中文翻譯資料（CC BY-NC 4.0）。本 MOD 的譯文逐句自行翻譯；術語選擇參考其譯名的條目，在 `glossary/terms.tsv` 的 `ref` 欄標為 `dfzh-wiki`。本 MOD 未收錄、也不散布其資料。

## 研究參考

- [DFI18n](https://github.com/DFI18n/dfi18n)（Viz，MIT License）及其 [PR #2](https://github.com/DFI18n/dfi18n/pull/2)（CooperZhuang）：遊戲文字繪製函式（`addst` 等）的身分與定位方式，最初對照其公開的說明與特徵碼確認。本 MOD 的引擎程式碼與特徵碼（`engine/patterns.tsv`，由 `scripts/make_patterns.py` 從遊戲執行檔產生）為自行撰寫，未包含其程式碼。
