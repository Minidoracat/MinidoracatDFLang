-- DFLang Chinese translation: settings window (Ctrl-Shift-L) and engine commands.
-- 第一行是 DFHack 的指令簡介（launcher、Ctrl-Shift-C 選單會顯示），和下面所有輸出一樣寫英文：DFHack 把輸出照 CP437 畫，
-- 寫中文會變亂碼；英文由引擎照字典（data/strings/dflang_ui.tsv、data/rules/dflang_ui.tsv）畫成中文。
-- 也當模組用：載入器用 dfcn_installed() 偵測 DFCN，在標題畫面的切換說明裡呼叫 hide_dfi18n()、disable_dfcn()，
-- 點標題畫面的吉祥物時呼叫 show_ui()。
--@ module = true

local USAGE = [[
Usage:
  dflang ui                         Open the settings window (hotkey: Ctrl-Shift-L)
  dflang status                     Show the engine status
  dflang on | off                   Turn the translation on or off
  dflang t <English text>           Look up the translation in the current language
  dflang lang zh-Hant|zh-Hans       Choose Traditional or Simplified Chinese; until you choose, it follows the Steam or Windows language
  dflang lang off                   Use DFI18n, DFCN or another translation mod and restore the ones DFLang turned off (after restarting the game); without one, DFLang stays on
  dflang misses on|off              Log text on screen that has no translation to dfhack-config/dflang-misses.tsv, also after restarting the game, until off
  dflang overlay on|off             Full-line text rendering (default on); off draws each Chinese character as a two-cell glyph
  dflang overlay size [13-16]       Chinese text size in 16ths of the cell height (default 14); without a number, show the setting
  dflang bold on|off                Bold Chinese text (default on); off uses the regular weight
]]
local CONFIG_FILE = 'dfhack-config/dflang.json'
local MISSES_FILE = 'dfhack-config/dflang-misses.tsv'

local json = require('json')

-- 停用 DFI18n：在 mods/ 放一個 ID 也是 dfi18n、版本號最大、沒有 DFI18n 腳本的 MOD。DFHack 每個 ID 只載入版本號最大的那一份，
-- DFI18n 的腳本就不會執行；DFI18n 自己的檔案不動，刪掉這個資料夾就恢復。info.txt 只能用 ASCII（遊戲的模組清單照 CP437 顯示）。
-- 版本號用 999999999：DFHack 挑版本時把版本號當字串比（script-manager.lua 的 get_mod_paths），遊戲照數字比，兩種比法都要贏過
-- DFI18n。以前的 2000000000 照字串比會輸給 21～29、3～9 這類版本號（DFI18n 在 2026-10 是 20）；九位數，留在 32 位元整數範圍內。
-- 資料夾裡的小腳本在 DFLang（含已合併的簡中項目）都不在之後刪掉這個資料夾，免得玩家退訂 DFLang 後 DFI18n 一直不載入
local HIDE_DIR = dfhack.filesystem.getBaseDir() .. 'mods/dflang-hide-dfi18n/'
local HIDE_FILES = {
    ['info.txt'] = [[
[ID:dfi18n]
[NUMERIC_VERSION:999999999]
[DISPLAYED_VERSION:off]
[EARLIEST_COMPATIBLE_NUMERIC_VERSION:1]
[EARLIEST_COMPATIBLE_DISPLAYED_VERSION:off]
[AUTHOR:DFLang]
[NAME:DFI18n turned off by DFLang]
[DESCRIPTION:Keeps DFI18n from loading while DFLang (Chinese translation) is in use. Type "dflang lang off" in the DFHack console, or delete this folder, to turn DFI18n back on.]
]],
    ['scripts_modinstalled/dflang_restore_dfi18n.lua'] = [[
-- DFLang 停用 DFI18n 用的 MOD：DFLang 已經移除時刪掉這個資料夾，DFI18n 下次啟動恢復
--@ module = true
local sm = require('script-manager')
if sm.getModSourcePath('cat_df_lang_zh_hant') or sm.getModSourcePath('cat_df_lang_zh_hans') then return end
local dir = sm.getModSourcePath('dfi18n')
if not dir or not dir:find('dflang%-hide%-dfi18n') then return end
if dir:sub(-1) ~= '/' then dir = dir .. '/' end
os.remove(dir .. 'scripts_modinstalled/dflang_restore_dfi18n.lua')
os.remove(dir .. 'info.txt')
dfhack.filesystem.rmdir(dir .. 'scripts_modinstalled')
dfhack.filesystem.rmdir(dir)
print('[DFLang] DFLang has been removed, so DFI18n will load again after restarting the game.')
]],
}

function hide_dfi18n()
    if not dfhack.filesystem.mkdir_recursive(HIDE_DIR .. 'scripts_modinstalled') then return false end
    for name, text in pairs(HIDE_FILES) do
        local f = io.open(HIDE_DIR .. name, 'wb')
        if not f then return false end
        f:write(text)
        f:close()
    end
    return true
end

-- 以前寫的遮蔽資料夾（版本號 2000000000）換成現在的寫法，DFI18n 之後升版也蓋得住。載入器在本 MOD 照常啟動時呼叫：
-- 那時遮蔽正在生效（或 DFI18n 已經移除），換掉不影響這一次遊戲裡已經在跑的腳本
function update_hide_dfi18n()
    local f = io.open(HIDE_DIR .. 'info.txt', 'rb')
    if not f then return end
    local old = f:read('a')
    f:close()
    if old ~= HIDE_FILES['info.txt'] then hide_dfi18n() end
end

function unhide_dfi18n()
    if not dfhack.filesystem.isdir(HIDE_DIR) then return false end
    for name in pairs(HIDE_FILES) do os.remove(HIDE_DIR .. name) end
    dfhack.filesystem.rmdir(HIDE_DIR .. 'scripts_modinstalled')
    return dfhack.filesystem.rmdir(HIDE_DIR)
end

-- 停用 DFCN：dfhooks 只載入遊戲目錄裡主檔名以 dfhooks_ 開頭、副檔名是 .ini（第一行寫 DLL 路徑）或 .dll 的檔案
-- （DFHack/dfhooks 的 ChainLoader.cpp），DFCN 的入口檔加上 DFCN_OFF 這個副檔名就不會載入；改回原檔名就恢復，DFCN 其他檔案不動。
-- 舊版 DFCN 直接把 dfhooks_dfcn.dll 放在遊戲目錄，一起處理
local DFCN_FILES = {'dfhooks_dfcn.ini', 'dfhooks_dfcn.dll'}
local DFCN_OFF = '.disabled-by-dflang'

local function install_dir()
    return (dfhack.filesystem.getInstallDir():gsub('[/\\]?$', '/'))
end

function dfcn_installed()
    local dir = install_dir()
    for _, name in ipairs(DFCN_FILES) do
        if dfhack.filesystem.isfile(dir .. name) then return true end
    end
    return false
end

function disable_dfcn()
    local dir, ok = install_dir(), true
    for _, name in ipairs(DFCN_FILES) do
        if dfhack.filesystem.isfile(dir .. name) then
            os.remove(dir .. name .. DFCN_OFF)  -- 之前停用過、玩家又照 DFCN 的說明重新複製一份
            ok = os.rename(dir .. name, dir .. name .. DFCN_OFF) and ok
        end
    end
    return ok
end

function enable_dfcn()
    local dir, any = install_dir(), false
    for _, name in ipairs(DFCN_FILES) do
        if dfhack.filesystem.isfile(dir .. name .. DFCN_OFF) and not dfhack.filesystem.isfile(dir .. name) then
            any = os.rename(dir .. name .. DFCN_OFF, dir .. name) and true or any
        end
    end
    return any
end

function set_lang(lang)
    local config = json.open(CONFIG_FILE)
    config.data.lang = lang
    config:write()
end

-- 整句貼圖：預設開，只記關。這些設定改了以後由 apply_now 立即套用
function set_overlay(on)
    local config = json.open(CONFIG_FILE)
    if on then config.data.overlay = nil else config.data.overlay = false end
    config:write()
end

-- 整句貼圖的字身（十六分之幾格高），段落量字寬也照它。預設 14（engine/src/overlay.rs 的 DEFAULT_EM），只記其他值
local DEFAULT_SIZE = 14
function set_overlay_size(n)
    local config = json.open(CONFIG_FILE)
    config.data.overlay_size = n ~= DEFAULT_SIZE and n or nil
    config:write()
end

-- 字重：dflang/fonts/bold/ 或 regular/，字圖集與整句貼圖都用它。預設粗體，只記關
function set_bold(on)
    local config = json.open(CONFIG_FILE)
    if on then config.data.bold = nil else config.data.bold = false end
    config:write()
end

-- 也翻成中文的選項與預設值（names 原生語言名字音譯、keys 按鍵名、music 配樂曲名、partial 段落缺句時其餘照翻）：
-- 遊戲啟動時與改了以後交給引擎（lua/loader.lua 的 wanted）。名字音譯與按鍵名預設開，其他預設關；dflang.json 只記和預設不同的
local OPTIONS = {names = true, keys = true, music = false, partial = false}
function option_on(config, name)
    if config[name] == nil then return OPTIONS[name] end
    return config[name] == true
end
function set_option(name, on)
    if OPTIONS[name] == nil then return end
    local config = json.open(CONFIG_FILE)
    if on == OPTIONS[name] then config.data[name] = nil else config.data[name] = on end
    config:write()
end

-- 改過 dflang.json 之後立即套用（載入器的 apply，見 lua/loader.lua）。回傳 true 表示有設定要重開遊戲才生效：改用其他翻譯 MOD，
-- 或引擎沒在跑、舊版載入器不能即時套用。套用到一半出錯時錯誤寫進 stderr.log，也當成要重開。
-- 遊戲讀取中沒辦法換的語言、字重記在 DFLANG.pending，讀完自動套用（見 pending）
function apply_now()
    local state = dfhack.BASE_G.DFLANG
    if not (state and state.apply) then return true end
    local ok, restart = pcall(state.apply)
    if not ok then
        dfhack.printerr('[DFLang] ' .. tostring(restart))
        return true
    end
    return restart
end

-- 語言或字重的變更還擱著：DFHack 在遊戲讀取 raws、存檔時（開過建立世界的設定畫面後也常是）不能註冊新字圖，載入或建立世界後自動套用
local PENDING_NOTE = 'Language and bold text will change after a world loads.'
local function pending()
    local state = dfhack.BASE_G.DFLANG
    return state ~= nil and state.pending == true
end

-- 未翻譯紀錄立即開關（引擎沒啟用時只改設定）；設定會記住，之後每次啟動都自動記錄，直到關閉
function set_misses(on)
    local state = dfhack.BASE_G.DFLANG
    if state then
        local ok, err = state.engine.log_misses(on and MISSES_FILE or nil)
        if not ok then return nil, err end
    end
    local config = json.open(CONFIG_FILE)
    config.data.misses = on or nil
    config:write()
    return true
end

-- 清除紀錄：記錄中要先關檔才刪得掉，刪完重新開檔（引擎的去重也跟著清空，畫面上沒翻到的字會重新記下）
local function clear_misses()
    local state = dfhack.BASE_G.DFLANG
    local on = state and json.open(CONFIG_FILE).data.misses
    if on then state.engine.log_misses(nil) end
    os.remove(MISSES_FILE)
    if on then return state.engine.log_misses(MISSES_FILE) end
    return true
end

-- 紀錄檔大小（KB，無條件進位）；還沒有檔案時回傳 nil
local function misses_kb()
    local f = io.open(MISSES_FILE, 'rb')
    if not f then return nil end
    local size = f:seek('end')
    f:close()
    return (size + 1023) // 1024
end

-- 用檔案總管打開紀錄所在的資料夾（有紀錄檔時選取它）。路徑交給 cmd，遊戲目錄有非 ASCII 字元時可能打不開
local function open_misses_folder()
    local dir = (install_dir() .. 'dfhack-config'):gsub('/', '\\')
    if dfhack.filesystem.isfile(MISSES_FILE) then
        os.execute(('explorer /select,"%s\\dflang-misses.tsv"'):format(dir))
    else
        os.execute(('explorer "%s"'):format(dir))
    end
end

local LANG_NAMES = {['zh-Hant'] = 'Traditional Chinese', ['zh-Hans'] = 'Simplified Chinese', off = 'Other translation mod'}

-- 設定視窗語言那一列目前的值：玩家選過的，沒選過就是引擎現在用的（照系統判斷，見 lua/loader.lua 的 chosen_lang）
local function current_lang(config)
    if LANG_NAMES[config.lang] then return config.lang end
    local state = dfhack.BASE_G.DFLANG
    return state and state.lang or 'zh-Hant'
end

-- 設定視窗（dflang ui、Ctrl-Shift-L、點標題畫面的吉祥物）。字都寫英文，引擎照 data/strings/dflang_ui.tsv 畫成中文；
-- 引擎沒啟用時照英文
local gui = require('gui')
local widgets = require('gui.widgets')
local dialogs = require('gui.dialogs')

-- 最長的英文標籤（Partly translated paragraphs:）29 字再多兩格：標籤和值之間要有兩格以上空白，整句貼圖才不會把值接在中文標籤後面
local LABEL_WIDTH = 31
local REPORT_HELP = 'Logging stays on after restarting the game until you turn it off. To report untranslated text, '
    .. 'attach dflang-misses.tsv to a GitHub issue, or paste its lines into the bug report discussion on the '
    .. 'Workshop page. The log may contain names from your world.'

local function engine_status()
    local state = dfhack.BASE_G.DFLANG
    if not state then return 'Not running (see stderr.log in the game folder)', COLOR_LIGHTRED end
    return ('Running (%s)'):format(LANG_NAMES[state.lang]), COLOR_LIGHTGREEN
end

-- 也翻成中文的選項開關（見 set_option）：改了立即套用
local function option_toggle(window, t, name, key, label)
    return widgets.ToggleHotkeyLabel{
        frame = {t = t, l = 0},
        key = key,
        label = label,
        label_width = LABEL_WIDTH,
        initial_option = option_on(json.open(CONFIG_FILE).data, name),
        on_change = function(on) set_option(name, on) window:changed() end,
    }
end

DFLangWindow = defclass(DFLangWindow, widgets.Window)
DFLangWindow.ATTRS{
    frame_title = 'DFLang Settings',
    -- 高度要放得下英文的回報說明（翻譯關掉時照英文畫，四到五列）
    frame = {w = 70, h = 29},
}

function DFLangWindow:init()
    local config = json.open(CONFIG_FILE).data
    local state = dfhack.BASE_G.DFLANG
    local status = state and state.engine.status()
    -- 標題帶版本號（舊版載入器沒有 version，照原本的標題）
    if state and state.version then self.frame_title = ('DFLang Settings v%s'):format(state.version) end
    local langs = {{label = LANG_NAMES['zh-Hant'], value = 'zh-Hant'}, {label = LANG_NAMES['zh-Hans'], value = 'zh-Hans'}}
    if config.lang == 'off' then table.insert(langs, {label = LANG_NAMES.off, value = 'off'}) end
    self.restart = false
    self:refresh_kb()
    local running = function() return dfhack.BASE_G.DFLANG ~= nil end

    self:addviews{
        widgets.Label{
            frame = {t = 0, l = 0},
            text = {'Translation engine:', ' ',
                    {text = function() return (engine_status()) end, pen = function() return select(2, engine_status()) end}},
        },
        -- 暫時看英文（同 dflang on|off）：立即生效、不記住，下次啟動照常翻譯
        widgets.ToggleHotkeyLabel{
            view_id = 'enabled',
            frame = {t = 1, l = 0},
            key = 'CUSTOM_T',
            label = 'Show translation:',
            label_width = LABEL_WIDTH,
            initial_option = not status or status.enabled,
            enabled = running,
            on_change = function(on)
                local st = dfhack.BASE_G.DFLANG
                local ok, err = (on and st.engine.enable or st.engine.disable)()
                if not ok then
                    self.subviews.enabled:setOption(not on)
                    dialogs.showMessage('DFLang', tostring(err), COLOR_LIGHTRED)
                end
                if st.sync_buttons then st.sync_buttons() end
            end,
        },
        widgets.CycleHotkeyLabel{
            frame = {t = 3, l = 0},
            key = 'CUSTOM_L',
            label = 'Language:',
            label_width = LABEL_WIDTH,
            options = langs,
            initial_option = current_lang(config),
            on_change = function(lang) set_lang(lang) self:changed() end,
        },
        widgets.ToggleHotkeyLabel{
            view_id = 'overlay',
            frame = {t = 4, l = 0},
            key = 'CUSTOM_O',
            label = 'Full-line text rendering:',
            label_width = LABEL_WIDTH,
            initial_option = config.overlay ~= false,
            on_change = function(on) set_overlay(on) self:changed() end,
        },
        widgets.CycleHotkeyLabel{
            frame = {t = 5, l = 0},
            key = 'CUSTOM_S',
            label = 'Chinese text size:',
            label_width = LABEL_WIDTH,
            options = {{label = '13/16', value = 13}, {label = '14/16 (default)', value = 14},
                       {label = '15/16', value = 15}, {label = '16/16', value = 16}},
            initial_option = config.overlay_size or DEFAULT_SIZE,
            enabled = function() return self.subviews.overlay:getOptionValue() end,
            on_change = function(n) set_overlay_size(n) self:changed() end,
        },
        widgets.ToggleHotkeyLabel{
            frame = {t = 6, l = 0},
            key = 'CUSTOM_B',
            label = 'Bold text:',
            label_width = LABEL_WIDTH,
            initial_option = config.bold ~= false,
            on_change = function(on) set_bold(on) self:changed() end,
        },
        -- 平常照原文的部分也翻成中文：原生語言的名字（音譯）、按鍵名、配樂曲名，以及段落缺句時其餘照翻（預設值見 OPTIONS）
        option_toggle(self, 7, 'names', 'CUSTOM_N', 'Transliterate native names:'),
        option_toggle(self, 8, 'keys', 'CUSTOM_K', 'Translate key names:'),
        option_toggle(self, 9, 'music', 'CUSTOM_U', 'Translate music titles:'),
        option_toggle(self, 10, 'partial', 'CUSTOM_P', 'Partly translated paragraphs:'),
        widgets.Label{
            frame = {t = 11, l = 3},
            text = {{text = 'Restart the game to apply these changes.', pen = COLOR_YELLOW}},
            visible = function() return self.restart end,
        },
        widgets.Label{
            frame = {t = 11, l = 3},
            text = {{text = PENDING_NOTE, pen = COLOR_YELLOW}},
            visible = function() return not self.restart and pending() end,
        },
        widgets.ToggleHotkeyLabel{
            view_id = 'misses',
            frame = {t = 13, l = 0},
            key = 'CUSTOM_M',
            label = 'Log untranslated text:',
            label_width = LABEL_WIDTH,
            initial_option = config.misses == true,
            enabled = running,
            on_change = function(on)
                local ok, err = set_misses(on)
                if not ok then
                    self.subviews.misses:setOption(not on)
                    dialogs.showMessage('DFLang', tostring(err), COLOR_LIGHTRED)
                end
                self:refresh_kb()
            end,
        },
        widgets.Label{
            frame = {t = 14, l = 3},
            text = {'Log file (in the game folder):', ' ', MISSES_FILE},
        },
        widgets.Label{
            frame = {t = 15, l = 3},
            text = {{text = function()
                        local kb = self:kb_now()
                        return kb and ('Log size: %d KB'):format(kb) or 'No log file yet'
                    end},
                    {gap = 1, text = function() return self.full and '(full; clear the log to keep logging)' or '' end,
                     pen = COLOR_YELLOW}},
        },
        widgets.HotkeyLabel{
            frame = {t = 16, l = 0},
            key = 'CUSTOM_F',
            label = 'Open the log folder',
            auto_width = true,
            on_activate = open_misses_folder,
        },
        widgets.HotkeyLabel{
            frame = {t = 17, l = 0},
            key = 'CUSTOM_C',
            label = 'Clear the log',
            auto_width = true,
            enabled = function() return self:kb_now() ~= nil end,
            on_activate = function()
                dialogs.showYesNoPrompt('Clear untranslated text log', 'Delete everything in dflang-misses.tsv?', COLOR_WHITE,
                    function()
                        local ok, err = clear_misses()
                        if not ok then dialogs.showMessage('DFLang', tostring(err), COLOR_LIGHTRED) end
                        self:refresh_kb()
                    end)
            end,
        },
        widgets.WrappedLabel{
            frame = {t = 19, l = 0, r = 0},
            text_to_wrap = REPORT_HELP,
            text_pen = COLOR_GREY,
        },
    }
end

-- 立即套用剛改的設定（見 apply_now）；要重開遊戲才生效的（改用其他翻譯 MOD、引擎沒在跑）顯示黃字
function DFLangWindow:changed()
    self.restart = apply_now()
end

-- 紀錄檔大小與是否到了引擎的上限（misses_full：舊版引擎沒有這個欄位，當成沒滿）
function DFLangWindow:refresh_kb()
    local state = dfhack.BASE_G.DFLANG
    self.kb, self.kb_at = misses_kb(), dfhack.getTickCount()
    self.full = state and state.engine.status().misses_full or false
end

-- 記錄中檔案會變大：每秒最多重讀一次大小
function DFLangWindow:kb_now()
    if dfhack.getTickCount() - self.kb_at > 1000 then self:refresh_kb() end
    return self.kb
end

DFLangScreen = defclass(DFLangScreen, gui.ZScreen)
DFLangScreen.ATTRS{focus_path = 'dflang'}

function DFLangScreen:init()
    self:addviews{DFLangWindow{}}
end

function DFLangScreen:onDismiss()
    dfhack.BASE_G.DFLANG_UI = nil
end

-- 開著時再按一次快捷鍵：視窗有焦點就關掉，沒有（玩家點了底下的畫面）就拉到最上層。
-- 記在 BASE_G：快捷鍵執行的指令和吉祥物呼叫的模組是不同的執行環境
function show_ui()
    local view = dfhack.BASE_G.DFLANG_UI
    if view and view:isActive() then
        if view:hasFocus() then view:dismiss() else view:raise() end
        return
    end
    dfhack.BASE_G.DFLANG_UI = DFLangScreen{}:show()
end

if dfhack_flags.module then return end

local args = {...}
local cmd = args[1]
local state = dfhack.BASE_G.DFLANG

if cmd == 'ui' then
    show_ui()
    return
end

if cmd == 'lang' then
    local lang = args[2]
    if lang ~= 'zh-Hant' and lang ~= 'zh-Hans' and lang ~= 'off' then qerror(USAGE) end
    set_lang(lang)
    if lang == 'off' then
        local dfi18n, dfcn = unhide_dfi18n(), enable_dfcn()
        local back = dfi18n and dfcn and 'DFI18n and DFCN' or dfi18n and 'DFI18n' or dfcn and 'DFCN'
        print(back and ('Switched to the other translation mod (%s restored). Restart the game to apply.'):format(back)
            or 'Switched to the other translation mod. Restart the game to apply. Without one, DFLang stays on.')
    elseif apply_now() then
        print(('Language set to %s. Restart the game to apply.'):format(lang))
    else
        print(('Language set to %s.'):format(lang))
        if pending() then print(PENDING_NOTE) end
    end
    return
end

if not state then qerror('The DFLang engine is not running; see the [DFLang] lines in stderr.log in the game folder.') end
local engine = state.engine

local function check(ok, err)
    if not ok then qerror(err) end
end

if cmd == 'status' or not cmd then
    local s = engine.status()
    print(('DFLang %s: translation %s'):format(state.lang, s.enabled and 'on' or 'off'))
    print(('Hooks: %d, translations: %d, patterns: %d, glyphs: %d'):format(s.hooks, s.entries, s.rules, s.glyphs))
    print(('Tooltips translated at startup: %d'):format(s.wraps))
    print(('This session: %d hits, %d misses'):format(s.hits, s.misses))
elseif cmd == 'on' then
    check(engine.enable())
    if state.sync_buttons then state.sync_buttons() end
elseif cmd == 'off' then
    check(engine.disable())
    if state.sync_buttons then state.sync_buttons() end
elseif cmd == 't' then
    local text = table.concat(args, ' ', 2)
    print(engine.translate(text) or '(Neither the dictionary nor the patterns have this text.)')
elseif cmd == 'misses' and (args[2] == 'on' or args[2] == 'off') then
    local on = args[2] == 'on'
    check(set_misses(on))
    print(on and 'Logging untranslated text to dfhack-config/dflang-misses.tsv, also after restarting the game.'
        or 'Stopped logging untranslated text.')
-- 外觀的設定改了立即套用；要重開遊戲才生效時（引擎太舊不能即時套用、套用失敗）另外提示
elseif cmd == 'overlay' and (args[2] == 'on' or args[2] == 'off') then
    local on = args[2] == 'on'
    set_overlay(on)
    print(on and 'Full-line text rendering: on.' or 'Full-line text rendering: off, back to two-cell glyphs.')
    if apply_now() then print('Restart the game to apply these changes.') end
elseif cmd == 'bold' and (args[2] == 'on' or args[2] == 'off') then
    local on = args[2] == 'on'
    set_bold(on)
    print(on and 'Bold text: on.' or 'Bold text: off, using the regular weight.')
    if apply_now() then print('Restart the game to apply these changes.') elseif pending() then print(PENDING_NOTE) end
elseif cmd == 'overlay' and args[2] == 'size' then
    if not args[3] then
        print(('Chinese text size: %d/16 of the cell height.'):format(json.open(CONFIG_FILE).data.overlay_size or DEFAULT_SIZE))
        return
    end
    local n = math.tointeger(tonumber(args[3]))
    if not n or n < 13 or n > 16 then qerror(USAGE) end
    set_overlay_size(n)
    print(('Chinese text size: %d/16 of the cell height.'):format(n))
    if apply_now() then print('Restart the game to apply these changes.') end
else
    print(USAGE)
end
