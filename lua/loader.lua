-- DFLang 中文化（@LANG@）載入器：DFHack 啟動時當模組執行，載入本包 dflang/ 底下的引擎、字典與字圖集。
-- scripts/build.py 依語言換掉 @LANG@、@MOD_ID@ 並改檔名；兩包檔名不同，DFHack 才會兩支都執行。
--@module = true

local LANG = '@LANG@'
local MOD_ID = '@MOD_ID@'
local PACKAGES = {['zh-Hant'] = 'cat_df_lang_zh_hant', ['zh-Hans'] = 'cat_df_lang_zh_hans'}
-- 會攔同一批寫字函式的其他翻譯引擎；看它的載入腳本在不在，不只看 MOD ID
local CONFLICTS = {
    {id = 'dfi18n', script = 'scripts_modinstalled/dfi18n_loader.lua', name = 'DFI18n'},
    {id = 'dfzh_loader', script = 'scripts_modinstalled/dfzh_loader.lua', name = 'dfzh'},
}
local HOOKS = {'addst', 'addst_flag', 'top_addst', 'addcoloredst'}
local GPS_FIELDS = {'screenx', 'screeny', 'dimx', 'dimy', 'clipx', 'clipy', 'screen', 'screen_limit',
                    'screentexpos', 'screentexpos_flag', 'screen_top', 'screen_top_limit',
                    'screentexpos_top', 'screentexpos_top_flag'}
local CONFIG_FILE = 'dfhack-config/dflang.json'
-- 每次啟動依當下格子大小重畫的字圖集（相對於遊戲目錄）；可直接打開檢查字形
local ATLAS_FILE = 'dfhack-config/dflang-atlas-%s.png'
-- dflang misses on 之後每次啟動都把畫面上沒翻到的字串記到這裡（dflang.lua 共用同一個路徑）
local MISSES_FILE = 'dfhack-config/dflang-misses.tsv'

local json = require('json')
local scriptmanager = require('script-manager')

local function log(fmt, ...) print(('[DFLang %s] ' .. fmt):format(LANG, ...)) end
local function fail(fmt, ...) dfhack.printerr(('[DFLang %s] ' .. fmt):format(LANG, ...)) end

local function mod_dir(id)
    local path = scriptmanager.getModSourcePath(id)
    if not path then return nil end
    path = path:gsub('\\', '/')
    return path:sub(-1) == '/' and path or path .. '/'
end

local function read_file(path)
    local f = io.open(path, 'rb')
    if not f then return nil end
    local s = f:read('a')
    f:close()
    return s
end

-- 兩包都裝時只啟用一包：dfhack-config/dflang.json 的 lang 優先，沒有就用繁體
local function chosen_lang()
    local pref = json.open(CONFIG_FILE).data.lang
    if pref and PACKAGES[pref] and mod_dir(PACKAGES[pref]) then return pref end
    return mod_dir(PACKAGES['zh-Hant']) and 'zh-Hant' or 'zh-Hans'
end

local function conflict()
    for _, c in ipairs(CONFLICTS) do
        local dir = mod_dir(c.id)
        if dir and dfhack.filesystem.isfile(dir .. c.script) then return c.name end
    end
end

local function gps_layout()
    local gps = df.global.gps
    local _, base = df.sizeof(gps)
    local layout = {}
    for _, name in ipairs(GPS_FIELDS) do
        local ok, field = pcall(gps._field, gps, name)
        if not ok then return nil, name end
        local _, addr = df.sizeof(field)
        layout[name] = addr - base
    end
    return layout
end

local function resolve(handles)
    local texpos = {}
    for i, handle in ipairs(handles) do texpos[i] = dfhack.textures.getTexposByHandle(handle) end
    return texpos
end

-- 目前畫面上一格的像素大小；字圖照這個大小畫，遊戲就不必縮放（最近鄰縮放會讓筆畫斷掉）
local function cell_size()
    local ok, w, h = pcall(function()
        local r = df.global.enabler.renderer
        return r.dispx_z, r.dispy_z
    end)
    if ok and w and w >= 4 and h >= 6 then return w, h end
    return 16, 24
end

local function load_glyphs(engine, dir)
    local chars = (read_file(dir .. 'glyphs.txt') or ''):match('^([^\n]*)')
    if not chars or chars == '' then return nil, 'glyphs.txt 是空的' end
    local fonts = {}
    for _, name in ipairs(dfhack.filesystem.listdir(dir .. 'fonts') or {}) do
        if name:match('%.[ot]tf$') then table.insert(fonts, dir .. 'fonts/' .. name) end
    end
    table.sort(fonts) -- 檔名開頭的序號就是補字順序
    local w, h = cell_size()
    local png = ATLAS_FILE:format(LANG)
    local tiles, missing = engine.render_atlas(png, w, h, chars, fonts)
    if not tiles then return nil, missing end
    if missing ~= '' then fail('沒有字型能畫：%s', missing) end
    -- 字圖集最後一列可能沒排滿；只取每字左右兩格
    local loaded = dfhack.textures.loadTileset(png, w, h, true)
    local handles = table.move(loaded, 1, utf8.len(chars) * 2, 1, {})
    local n, err = engine.set_glyphs(chars, resolve(handles))
    if not n then return nil, err end
    return handles, n
end

local function attach_hooks(engine, dir)
    local patterns = {}
    for name, pattern in (read_file(dir .. 'patterns.tsv') or ''):gmatch('([%w_]+)\t([^\t\n]+)') do
        patterns[name] = pattern
    end
    for _, name in ipairs(HOOKS) do
        if not patterns[name] then return nil, ('patterns.tsv 沒有 %s'):format(name) end
        local addr, hits = engine.scan(patterns[name])
        if hits ~= 1 then
            return nil, ('%s 的特徵碼命中 %d 次，這版遊戲可能還不支援'):format(name, hits)
        end
        local ok, err = engine.attach(name, addr)
        if not ok then return nil, err end
    end
    return true
end

local function start()
    if dfhack.BASE_G.DFLANG then return end
    if chosen_lang() ~= LANG then
        log('另一個語言包啟用中，本包不載入（dflang lang %s 可切換）', LANG)
        return
    end
    local other = conflict()
    if other then
        fail('偵測到 %s：兩個翻譯引擎會攔同一批函式。請退訂其中一個再重開遊戲。', other)
        return
    end
    local dir = mod_dir(MOD_ID)
    if not dir then return fail('找不到本 MOD 的目錄（%s）', MOD_ID) end
    dir = dir .. 'dflang/'

    local open, err = package.loadlib(dir .. 'dflang.dll', 'luaopen_dflang')
    if not open then return fail('載入引擎失敗：%s', err) end
    local engine = open()
    if not engine then return fail('引擎找不到 DFHack 的 lua53.dll') end

    local layout, missing = gps_layout()
    if not layout then return fail('這版 DFHack 的 gps 沒有欄位 %s', missing) end
    local ok
    ok, err = engine.set_layout(layout)
    if not ok then return fail('%s', err) end

    local handles, glyphs = load_glyphs(engine, dir)
    if not handles then return fail('%s', glyphs) end
    local loaded, bad, missing_glyphs = engine.load(dir .. 'strings.tsv')
    if not loaded then return fail('%s', bad) end
    if bad > 0 or missing_glyphs > 0 then
        fail('字典有 %d 條原文無法比對、%d 個字沒有字圖', bad, missing_glyphs)
    end
    local rules, bad_rules, missing_rule_glyphs = engine.load_rules(dir .. 'rules.tsv')
    if not rules then return fail('%s', bad_rules) end
    if bad_rules > 0 or missing_rule_glyphs > 0 then
        fail('句型有 %d 條寫法不對、%d 個字沒有字圖', bad_rules, missing_rule_glyphs)
    end

    ok, err = attach_hooks(engine, dir)
    if not ok then return fail('%s', err) end
    ok, err = engine.enable()
    if not ok then return fail('%s', err) end
    if json.open(CONFIG_FILE).data.misses then
        ok, err = engine.log_misses(MISSES_FILE)
        if not ok then fail('無法記錄未翻譯字串：%s', err) end
    end

    local first = dfhack.textures.getTexposByHandle(handles[1])
    dfhack.BASE_G.DFLANG = {engine = engine, lang = LANG, dir = dir, config = CONFIG_FILE}
    -- 字圖放在 DFHack 的保留範圍，texpos 不會變；保留範圍不夠時 DFHack 會改用動態範圍，遊戲重載貼圖後要重查
    dfhack.onStateChange.dflang = function(sc)
        if sc ~= SC_VIEWSCREEN_CHANGED and sc ~= SC_WORLD_LOADED and sc ~= SC_WORLD_UNLOADED then return end
        local now = dfhack.textures.getTexposByHandle(handles[1])
        if now == first then return end
        first = now
        engine.update_texpos(resolve(handles))
    end
    log('已啟用：%d 條譯文、%d 條句型、%d 個字圖', loaded, rules, glyphs)
end

local ok, err = pcall(start)
if not ok then fail('啟動失敗：%s', err) end
