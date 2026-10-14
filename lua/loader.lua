-- DFLang 中文化（@LANG@）載入器：DFHack 啟動時當模組執行，載入本包 dflang/ 底下的引擎、字典與字圖集。
-- scripts/build.py 依語言換掉 @LANG@、@MOD_ID@、@MASCOT_LABEL@、@AUTHOR@ 並改檔名；兩包檔名不同，DFHack 才會兩支都執行。
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
-- 標題畫面左下角的吉祥物：本包 dflang/mascot.png 依目前格子大小縮成高 MASCOT_ROWS 格，底部疊上名牌（語言標籤＋作者名）
local MASCOT_FILE = 'dfhack-config/dflang-mascot-%s.png'
local MASCOT_ROWS = 12
local MASCOT_LABEL = '@MASCOT_LABEL@'
local AUTHOR = '@AUTHOR@'

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

-- 本包的字型檔，檔名開頭的序號就是補字順序
local function font_files(dir)
    local fonts = {}
    for _, name in ipairs(dfhack.filesystem.listdir(dir .. 'fonts') or {}) do
        if name:match('%.[ot]tf$') then table.insert(fonts, dir .. 'fonts/' .. name) end
    end
    table.sort(fonts)
    return fonts
end

local function load_glyphs(engine, dir)
    local chars = (read_file(dir .. 'glyphs.txt') or ''):match('^([^\n]*)')
    if not chars or chars == '' then return nil, 'glyphs.txt 是空的' end
    local fonts = font_files(dir)
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

-- 段落排版（教學、說明、彈出視窗的長文）用的結構位移；這版 DFHack 沒有這些定義就回傳 nil，段落維持英文
local function mtb_layout()
    local ok, layout = pcall(function()
        local box, word = df.markup_text_boxst._fields, df.markup_text_wordst._fields
        return {word = box.word.offset, current_width = box.current_width.offset, max_y = box.max_y.offset,
                str = word.str.offset, red = word.red.offset, link_index = word.link_index.offset,
                px = word.px.offset, py = word.py.offset, flags = word.flags.offset}
    end)
    return ok and layout or nil
end

local function attach_hooks(engine, dir, hooks)
    local patterns = {}
    for name, pattern in (read_file(dir .. 'patterns.tsv') or ''):gmatch('([%w_]+)\t([^\t\n]+)') do
        patterns[name] = pattern
    end
    for _, name in ipairs(hooks) do
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

-- 吉祥物圖不是必要的；讀不到就略過，翻譯照常啟用
local function load_mascot(engine, dir)
    local src = dir .. 'mascot.png'
    if not dfhack.filesystem.isfile(src) then return nil end
    local w, h = cell_size()
    local png = MASCOT_FILE:format(LANG)
    local cols, err = engine.render_mascot(src, png, w, h, MASCOT_ROWS, MASCOT_LABEL, AUTHOR, font_files(dir))
    if not cols then
        fail('吉祥物圖片讀取失敗：%s', err)
        return nil
    end
    return {handles = dfhack.textures.loadTileset(png, w, h, true), cols = cols}
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
    local hooks = {table.unpack(HOOKS)}
    local mtb = mtb_layout()
    if mtb and engine.set_mtb_layout(mtb) then
        table.insert(hooks, 'mtb_set_width')
    else
        fail('這版 DFHack 沒有段落結構定義，教學與說明文字維持英文')
    end

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

    ok, err = attach_hooks(engine, dir, hooks)
    if not ok then return fail('%s', err) end
    ok, err = engine.enable()
    if not ok then return fail('%s', err) end
    if json.open(CONFIG_FILE).data.misses then
        ok, err = engine.log_misses(MISSES_FILE)
        if not ok then fail('無法記錄未翻譯字串：%s', err) end
    end

    local first = dfhack.textures.getTexposByHandle(handles[1])
    dfhack.BASE_G.DFLANG = {engine = engine, lang = LANG, dir = dir, config = CONFIG_FILE,
                            mascot = load_mascot(engine, dir)}
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

-- DFHack overlay 會掃描所有模組腳本的 OVERLAY_WIDGETS；玩家可在 gui/overlay 移動或關閉這個圖示。
-- 兩包都裝時兩個小工具都會註冊，只有被選用的那包畫圖。
local overlay = require('plugins.overlay')

MascotOverlay = defclass(MascotOverlay, overlay.OverlayWidget)
MascotOverlay.ATTRS{
    desc = 'DFLang 中文化吉祥物，顯示在標題畫面左下角。',
    default_pos = {x = 21, y = -5},
    default_enabled = true,
    viewscreens = 'title/Default',
    frame = {w = 16, h = MASCOT_ROWS},
}

-- 畫在頂層（screentexpos_top）：主層有貼圖的格子會先填底色，圖片透明處會變成黑框；頂層直接疊在標題背景上。
-- 標題畫面本來沒開頂層，要設 top_in_use；遊戲每幀會清空頂層，離開標題畫面後不會殘留。
function MascotOverlay:onRenderBody()
    local state = dfhack.BASE_G.DFLANG
    local mascot = state and state.lang == LANG and state.mascot
    if not mascot then return end
    local gps, rect = df.global.gps, self.frame_rect
    gps.top_in_use = true
    for i, handle in ipairs(mascot.handles) do
        local x, y = rect.x1 + (i - 1) % mascot.cols, rect.y1 + (i - 1) // mascot.cols
        if x < gps.dimx and y < gps.dimy then
            local index = x * gps.dimy + y
            gps.screentexpos_top[index] = dfhack.textures.getTexposByHandle(handle)
            gps.screentexpos_top_flag[index] = 0
        end
    end
end

OVERLAY_WIDGETS = {mascot = MascotOverlay}
