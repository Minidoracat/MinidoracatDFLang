-- dflang：DFLang 中文化引擎指令（在 DFHack 主控台或 dfhack-run 執行）。也當模組用：載入器用 dfcn_installed() 偵測 DFCN，
-- 在標題畫面的切換說明裡呼叫 hide_dfi18n()、disable_dfcn()。
--@ module = true

local USAGE = [[
用法：
  dflang status                     引擎狀態
  dflang on | off                   開關翻譯
  dflang t <英文原文>                查目前語言的譯文（整句字典、再句型規則）
  dflang lang zh-Hant|zh-Hans       繁、簡兩包都裝時選擇語言，重開遊戲生效
  dflang lang off                   改用 DFI18n、DFCN 等其他翻譯（之前停用的 DFI18n、DFCN 會恢復），重開遊戲生效；
                                    偵測不到其他翻譯 MOD 時本 MOD 照常啟用
  dflang misses on|off              把畫面上沒翻到的字串去重記錄到 dfhack-config/dflang-misses.tsv；
                                    設定會記住，之後每次啟動都自動記錄，直到 off
  dflang overlay on|off             整句貼圖（預設開）：字改用本包字型照字寬整句畫；off 改回一字兩格的字圖，
                                    重開遊戲生效
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

if dfhack_flags.module then return end

local args = {...}
local cmd = args[1]
local state = dfhack.BASE_G.DFLANG

if cmd == 'lang' then
    local lang = args[2]
    if lang ~= 'zh-Hant' and lang ~= 'zh-Hans' and lang ~= 'off' then qerror(USAGE) end
    set_lang(lang)
    if lang == 'off' then
        local back = {}
        if unhide_dfi18n() then table.insert(back, 'DFI18n') end
        if enable_dfcn() then table.insert(back, 'DFCN') end
        print(#back > 0 and ('已設定為改用其他翻譯，並恢復 %s；重開遊戲後生效。'):format(table.concat(back, '、'))
            or '已設定為改用其他翻譯，重開遊戲後生效（偵測不到其他翻譯 MOD 時本 MOD 照常啟用）。')
    else
        print(('已設定為 %s，重開遊戲後生效。'):format(lang))
    end
    return
end

if not state then qerror('DFLang 引擎沒有啟用；請看 DFHack 主控台開頭 [DFLang] 開頭的訊息。') end
local engine = state.engine

local function check(ok, err)
    if not ok then qerror(err) end
end

if cmd == 'status' or not cmd then
    local s = engine.status()
    print(('DFLang %s：%s；hook %d 個、譯文 %d 條、句型 %d 條、字圖 %d 個、啟動時切好的提示 %d 處有譯文；本次命中 %d 次、未命中 %d 次'):format(
        state.lang, s.enabled and '啟用中' or '已關閉', s.hooks, s.entries, s.rules, s.glyphs, s.wraps, s.hits, s.misses))
elseif cmd == 'on' then
    check(engine.enable())
elseif cmd == 'off' then
    check(engine.disable())
elseif cmd == 't' then
    local text = table.concat(args, ' ', 2)
    print(engine.translate(text) or '（字典與句型都沒有這句）')
elseif cmd == 'misses' and (args[2] == 'on' or args[2] == 'off') then
    local on = args[2] == 'on'
    check(engine.log_misses(on and MISSES_FILE or nil))
    local config = json.open(CONFIG_FILE)
    config.data.misses = on or nil
    config:write()
    print(on and ('記錄到 ' .. MISSES_FILE .. '，之後每次啟動都會自動記錄') or '已停止記錄')
elseif cmd == 'overlay' and (args[2] == 'on' or args[2] == 'off') then
    -- 整句貼圖會改段落的換行與替身字串，遊戲啟動時決定（見 engine/src/metrics.rs），中途切換要重開遊戲。預設開，只記關
    local on = args[2] == 'on'
    local config = json.open(CONFIG_FILE)
    if on then config.data.overlay = nil else config.data.overlay = false end
    config:write()
    print(on and '整句貼圖：開（重開遊戲後生效）' or '整句貼圖：關，改回一字兩格的字圖（重開遊戲後生效）')
else
    print(USAGE)
end
