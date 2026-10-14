-- dflang：DFLang 中文化引擎指令（在 DFHack 主控台或 dfhack-run 執行）。

local USAGE = [[
用法：
  dflang status                 引擎狀態
  dflang on | off               開關翻譯
  dflang t <英文原文>            查目前語言的譯文（整句字典、再句型規則）
  dflang lang zh-Hant|zh-Hans   繁、簡兩包都裝時選擇語言，重開遊戲生效
  dflang misses on|off          把畫面上沒翻到的字串去重記錄到 dfhack-config/dflang-misses.tsv；
                                設定會記住，之後每次啟動都自動記錄，直到 off
]]
local CONFIG_FILE = 'dfhack-config/dflang.json'
local MISSES_FILE = 'dfhack-config/dflang-misses.tsv'

local json = require('json')

local args = {...}
local cmd = args[1]
local state = dfhack.BASE_G.DFLANG

if cmd == 'lang' then
    local lang = args[2]
    if lang ~= 'zh-Hant' and lang ~= 'zh-Hans' then qerror(USAGE) end
    local config = json.open(CONFIG_FILE)
    config.data.lang = lang
    config:write()
    print(('已設定為 %s，重開遊戲後生效。'):format(lang))
    return
end

if not state then qerror('DFLang 引擎沒有啟用；請看 DFHack 主控台開頭 [DFLang] 開頭的訊息。') end
local engine = state.engine

local function check(ok, err)
    if not ok then qerror(err) end
end

if cmd == 'status' or not cmd then
    local s = engine.status()
    print(('DFLang %s：%s；hook %d 個、譯文 %d 條、句型 %d 條、字圖 %d 個、啟動時切好的提示 %d 句有譯文；本次命中 %d 次、未命中 %d 次'):format(
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
else
    print(USAGE)
end
