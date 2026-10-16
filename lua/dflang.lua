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
  dflang lang zh-Hant|zh-Hans       With both packs installed, choose the language (after restarting the game)
  dflang lang off                   Use DFI18n, DFCN or another translation mod and restore the ones DFLang turned off (after restarting the game); without one, DFLang stays on
  dflang misses on|off              Log text on screen that has no translation to dfhack-config/dflang-misses.tsv, also after restarting the game, until off
  dflang overlay on|off             Full-line text rendering (default on); off draws each Chinese character as a two-cell glyph (after restarting the game)
  dflang overlay size [13-16]       Chinese text size in 16ths of the cell height (default 14; after restarting the game); without a number, show the setting
  dflang bold on|off                Bold Chinese text (default on); off uses the regular weight (after restarting the game)
]]
local CONFIG_FILE = 'dfhack-config/dflang.json'
local MISSES_FILE = 'dfhack-config/dflang-misses.tsv'

local json = require('json')

-- 停用 DFI18n：在 mods/ 放一個 ID 也是 dfi18n、版本極大、沒有 DFI18n 腳本的 MOD。DFHack 每個 ID 只載入最高版本，
-- DFI18n 的腳本就不會執行；DFI18n 自己的檔案不動，刪掉這個資料夾就恢復。info.txt 只能用 ASCII（遊戲的模組清單照 CP437 顯示）。
-- 資料夾裡的小腳本在 DFLang 兩包都不在之後刪掉這個資料夾，免得玩家退訂 DFLang 後 DFI18n 一直不載入
local HIDE_DIR = dfhack.filesystem.getBaseDir() .. 'mods/dflang-hide-dfi18n/'
local HIDE_FILES = {
    ['info.txt'] = [[
[ID:dfi18n]
[NUMERIC_VERSION:2000000000]
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

-- 整句貼圖會改段落的換行與替身字串，遊戲啟動時決定（見 engine/src/metrics.rs），中途切換要重開遊戲。預設開，只記關
function set_overlay(on)
    local config = json.open(CONFIG_FILE)
    if on then config.data.overlay = nil else config.data.overlay = false end
    config:write()
end

-- 字身也決定段落的換行（見 engine/src/metrics.rs），遊戲啟動時畫字圖集前交給引擎（lua/loader.lua 的 load_glyphs）。
-- 預設 14（engine/src/overlay.rs 的 DEFAULT_EM），只記其他值
local DEFAULT_SIZE = 14
function set_overlay_size(n)
    local config = json.open(CONFIG_FILE)
    config.data.overlay_size = n ~= DEFAULT_SIZE and n or nil
    config:write()
end

-- 字重：遊戲啟動時選 dflang/fonts/bold/ 或 regular/ 交給引擎畫字圖集與整句貼圖（lua/loader.lua 的 load_glyphs）。預設粗體，只記關
function set_bold(on)
    local config = json.open(CONFIG_FILE)
    if on then config.data.bold = nil else config.data.bold = false end
    config:write()
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

local PACKAGES = {['zh-Hant'] = 'cat_df_lang_zh_hant', ['zh-Hans'] = 'cat_df_lang_zh_hans'}
local LANG_NAMES = {['zh-Hant'] = 'Traditional Chinese', ['zh-Hans'] = 'Simplified Chinese', off = 'Other translation mod'}

local function installed(lang)
    return require('script-manager').getModSourcePath(PACKAGES[lang]) ~= nil
end

-- 下次啟動用的語言，和 lua/loader.lua 的 chosen_lang 同一套規則
local function next_lang()
    local pref = json.open(CONFIG_FILE).data.lang
    if pref == 'off' or pref and PACKAGES[pref] and installed(pref) then return pref end
    return installed('zh-Hant') and 'zh-Hant' or 'zh-Hans'
end

-- 設定視窗（dflang ui、Ctrl-Shift-L、點標題畫面的吉祥物）。字都寫英文，引擎照 data/strings/dflang_ui.tsv 畫成中文；
-- 引擎沒啟用時照英文
local gui = require('gui')
local widgets = require('gui.widgets')
local dialogs = require('gui.dialogs')

-- 最長的英文標籤（Full-line text rendering:）25 字再多兩格：標籤和值之間要有兩格以上空白，整句貼圖才不會把值接在中文標籤後面
local LABEL_WIDTH = 27
local REPORT_HELP = 'Logging stays on after restarting the game until you turn it off. To report untranslated text, '
    .. 'attach dflang-misses.tsv to a GitHub issue, or paste its lines into the bug report discussion on the '
    .. 'Workshop page. The log may contain names from your world.'

local function engine_status()
    local state = dfhack.BASE_G.DFLANG
    if not state then return 'Not running (see stderr.log in the game folder)', COLOR_LIGHTRED end
    return ('Running (%s)'):format(LANG_NAMES[state.lang]), COLOR_LIGHTGREEN
end

DFLangWindow = defclass(DFLangWindow, widgets.Window)
DFLangWindow.ATTRS{
    frame_title = 'DFLang Settings',
    -- 高度要放得下英文的回報說明（翻譯關掉時照英文畫，四到五列）
    frame = {w = 70, h = 25},
}

function DFLangWindow:init()
    local config = json.open(CONFIG_FILE).data
    local state = dfhack.BASE_G.DFLANG
    local status = state and state.engine.status()
    -- 標題帶版本號（舊版載入器沒有 version，照原本的標題）
    if state and state.version then self.frame_title = ('DFLang Settings v%s'):format(state.version) end
    local langs = {}
    for _, lang in ipairs{'zh-Hant', 'zh-Hans'} do
        if installed(lang) then table.insert(langs, {label = LANG_NAMES[lang], value = lang}) end
    end
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
                local engine = dfhack.BASE_G.DFLANG.engine
                local ok, err = (on and engine.enable or engine.disable)()
                if not ok then
                    self.subviews.enabled:setOption(not on)
                    dialogs.showMessage('DFLang', tostring(err), COLOR_LIGHTRED)
                end
            end,
        },
        widgets.CycleHotkeyLabel{
            frame = {t = 3, l = 0},
            key = 'CUSTOM_L',
            label = 'Language:',
            label_width = LABEL_WIDTH,
            options = langs,
            initial_option = next_lang(),
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
        widgets.Label{
            frame = {t = 7, l = 3},
            text = {{text = 'Restart the game to apply these changes.', pen = COLOR_YELLOW}},
            visible = function() return self.restart end,
        },
        widgets.ToggleHotkeyLabel{
            view_id = 'misses',
            frame = {t = 9, l = 0},
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
            frame = {t = 10, l = 3},
            text = {'Log file (in the game folder):', ' ', MISSES_FILE},
        },
        widgets.Label{
            frame = {t = 11, l = 3},
            text = {{text = function()
                        local kb = self:kb_now()
                        return kb and ('Log size: %d KB'):format(kb) or 'No log file yet'
                    end},
                    {gap = 1, text = function() return self.full and '(full; clear the log to keep logging)' or '' end,
                     pen = COLOR_YELLOW}},
        },
        widgets.HotkeyLabel{
            frame = {t = 12, l = 0},
            key = 'CUSTOM_F',
            label = 'Open the log folder',
            auto_width = true,
            on_activate = open_misses_folder,
        },
        widgets.HotkeyLabel{
            frame = {t = 13, l = 0},
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
            frame = {t = 15, l = 0, r = 0},
            text_to_wrap = REPORT_HELP,
            text_pen = COLOR_GREY,
        },
    }
end

-- 語言、整句貼圖、字身、字重要重開遊戲才生效：在視窗裡改過、而且和這次啟動時的設定不同（或引擎沒在跑）時顯示黃字
function DFLangWindow:changed()
    local state = dfhack.BASE_G.DFLANG
    local started = state and state.started
    if not started then
        self.restart = true
        return
    end
    local config = json.open(CONFIG_FILE).data
    self.restart = next_lang() ~= state.lang or (config.overlay ~= false) ~= started.overlay
        or (config.overlay_size or DEFAULT_SIZE) ~= started.overlay_size or (config.bold ~= false) ~= (started.bold ~= false)
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
    else
        print(('Language set to %s. Restart the game to apply.'):format(lang))
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
elseif cmd == 'off' then
    check(engine.disable())
elseif cmd == 't' then
    local text = table.concat(args, ' ', 2)
    print(engine.translate(text) or '(Neither the dictionary nor the patterns have this text.)')
elseif cmd == 'misses' and (args[2] == 'on' or args[2] == 'off') then
    local on = args[2] == 'on'
    check(set_misses(on))
    print(on and 'Logging untranslated text to dfhack-config/dflang-misses.tsv, also after restarting the game.'
        or 'Stopped logging untranslated text.')
elseif cmd == 'overlay' and (args[2] == 'on' or args[2] == 'off') then
    local on = args[2] == 'on'
    set_overlay(on)
    print(on and 'Full-line text rendering: on (after restarting the game).'
        or 'Full-line text rendering: off, back to two-cell glyphs (after restarting the game).')
elseif cmd == 'bold' and (args[2] == 'on' or args[2] == 'off') then
    local on = args[2] == 'on'
    set_bold(on)
    print(on and 'Bold text: on (after restarting the game).'
        or 'Bold text: off, using the regular weight (after restarting the game).')
elseif cmd == 'overlay' and args[2] == 'size' then
    if not args[3] then
        print(('Chinese text size: %d/16 of the cell height.'):format(json.open(CONFIG_FILE).data.overlay_size or DEFAULT_SIZE))
        return
    end
    local n = math.tointeger(tonumber(args[3]))
    if not n or n < 13 or n > 16 then qerror(USAGE) end
    set_overlay_size(n)
    print(('Chinese text size: %d/16 of the cell height (after restarting the game).'):format(n))
else
    print(USAGE)
end
