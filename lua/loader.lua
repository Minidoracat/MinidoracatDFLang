-- DFLang 中文化（@LANG@）載入器：DFHack 啟動時當模組執行，載入本包 dflang/ 底下的引擎、字典與字圖集。
-- scripts/build.py 依語言換掉 @LANG@、@MOD_ID@、@MASCOT_LABEL@、@AUTHOR@ 並改檔名；兩包檔名不同，DFHack 才會兩支都執行。
--@module = true

local LANG = '@LANG@'
local MOD_ID = '@MOD_ID@'
local PACKAGES = {['zh-Hant'] = 'cat_df_lang_zh_hant', ['zh-Hans'] = 'cat_df_lang_zh_hans'}
-- 會攔同一批寫字函式的其他翻譯引擎，依序檢查。DFCN 是遊戲啟動時由 dfhooks 載入的 DLL，看遊戲目錄有沒有它的入口檔
-- （check：dflang.lua 的 dfcn_installed；只訂閱、沒照它的說明複製入口檔就不會啟用）；放最前面：同時也裝了 DFI18n 時，
-- 只停用 DFI18n 解決不了 DFCN。DFHack 模組看它的載入腳本在不在（不只看 MOD ID）。
-- off：玩家在標題畫面的說明選「改用 DFLang」時呼叫的 dflang.lua 函式（見 notify），off_failed 是失敗時的訊息。
-- dfzh 沒有實測過本包字圖疊在它畫面上的樣子、也不知道怎麼安全停用，照英文說明請玩家自己移除
local CONFLICTS = {
    {check = 'dfcn_installed', name = 'DFCN', off = 'disable_dfcn', off_failed = 'Could not rename the DFCN files in the game folder.'},
    {id = 'dfi18n', script = 'scripts_modinstalled/dfi18n_loader.lua', name = 'DFI18n', off = 'hide_dfi18n',
        off_failed = 'Could not write to the mods folder.'},
    {id = 'dfzh_loader', script = 'scripts_modinstalled/dfzh_loader.lua', name = 'dfzh', notice = table.concat({
        'dfzh is also installed. Only one translation engine can run',
        'at a time, so DFLang is not running. To use DFLang, remove',
        'dfzh by following its own instructions, then restart the game.'}, '\n')},
}
local HOOKS = {'addst', 'addst_flag', 'top_addst', 'addcoloredst', 'add_anchored_tile'}
local GPS_FIELDS = {'screenx', 'screeny', 'dimx', 'dimy', 'clipx', 'clipy', 'screen', 'screen_limit',
                    'screentexpos', 'screentexpos_flag', 'screentexpos_lower', 'screen_top', 'screen_top_limit',
                    'screentexpos_top', 'screentexpos_top_flag', 'screentexpos_top_lower', 'screentexpos_anchored',
                    'screen_color_r', 'screen_color_g', 'screen_color_b'}
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

local function default_lang()
    return mod_dir(PACKAGES['zh-Hant']) and 'zh-Hant' or 'zh-Hans'
end

-- 兩包都裝時只啟用一包：dfhack-config/dflang.json 的 lang 優先，沒有就用繁體；off 是改用其他翻譯引擎（見 start）
local function chosen_lang()
    local pref = json.open(CONFIG_FILE).data.lang
    if pref == 'off' or pref and PACKAGES[pref] and mod_dir(PACKAGES[pref]) then return pref end
    return default_lang()
end

-- 本包的 dflang.lua（偵測、停用其他翻譯引擎的函式）。兩包、新舊版的 dflang.lua 可能同時在 DFHack 的腳本路徑上，
-- reqscript('dflang') 不一定找到本包這一份（實測找到舊版、沒有新函式），照路徑載入
local own_dflang
local function dflang()
    if not own_dflang then
        own_dflang = setmetatable({dfhack_flags = {module = true}}, {__index = _G})
        assert(loadfile(mod_dir(MOD_ID) .. 'scripts_modinstalled/dflang.lua', 't', own_dflang))()
    end
    return own_dflang
end

local function conflict()
    for _, c in ipairs(CONFLICTS) do
        if c.check then
            if dflang()[c.check]() then return c end
        else
            local dir = mod_dir(c.id)
            if dir and dfhack.filesystem.isfile(dir .. c.script) then return c end
        end
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

-- 本包某個字重（bold、regular，見 scripts/build.py 的 WEIGHTS）的字型檔，檔名開頭的序號就是補字順序
local function font_files(dir, weight)
    local fonts, sub = {}, dir .. 'fonts/' .. weight .. '/'
    for _, name in ipairs(dfhack.filesystem.listdir(sub) or {}) do
        if name:match('%.[ot]tf$') then table.insert(fonts, sub .. name) end
    end
    table.sort(fonts)
    return fonts
end

local function load_glyphs(engine, dir)
    local chars = (read_file(dir .. 'glyphs.txt') or ''):match('^([^\n]*)')
    if not chars or chars == '' then return nil, 'glyphs.txt 是空的' end
    -- 字重（dflang bold on|off，預設粗體、只記 bold = false）：字圖集與整句貼圖都用這組字型
    local config = json.open(CONFIG_FILE).data
    local fonts = font_files(dir, config.bold == false and 'regular' or 'bold')
    local w, h = cell_size()
    local png = ATLAS_FILE:format(LANG)
    -- 整句貼圖的字身（dflang overlay size，十六分之幾格高）：段落量字寬也照這個大小，所以在畫字圖集時一起交給引擎
    local tiles, missing = engine.render_atlas(png, w, h, chars, fonts, config.overlay_size)
    if not tiles then return nil, missing end
    if missing ~= '' then fail('沒有字型能畫：%s', missing) end
    -- 字圖集最後一列可能沒排滿；每字取八張：左右兩格、半格位移的上下列各左右兩格（分頁標籤這類文字）、錨定貼圖標籤用的小一號字，
    -- 最後再一張透明記號（整句貼圖排段落時的佔位格，見 engine/src/atlas.rs）
    local loaded = dfhack.textures.loadTileset(png, w, h, true)
    local handles = table.move(loaded, 1, utf8.len(chars) * 8 + 1, 1, {})
    local n, err = engine.set_glyphs(chars, resolve(handles))
    if not n then return nil, err end
    return handles, n
end

-- 段落排版（教學、說明、彈出視窗的長文）用的結構位移與調色盤位址；這版 DFHack 沒有這些定義就回傳 nil，段落維持英文
local function mtb_layout()
    local ok, layout = pcall(function()
        local box, word = df.markup_text_boxst._fields, df.markup_text_wordst._fields
        return {word = box.word.offset, current_width = box.current_width.offset,
                str = word.str.offset, red = word.red.offset, link_index = word.link_index.offset,
                px = word.px.offset, py = word.py.offset, flags = word.flags.offset,
                palette = select(2, df.global.gps.uccolor:sizeof())}
    end)
    return ok and layout or nil
end

-- 彩色文字框（人物頁的想法、記憶、性格）用的結構位移；這版 DFHack 沒有這些定義就回傳 nil，這些文字維持英文
local function ctb_layout()
    local ok, layout = pcall(function()
        return {width = df.color_text_boxst._fields.width.offset, color = df.color_text_linest._fields.color.offset}
    end)
    return ok and layout or nil
end

-- 世界生成名稱（見引擎 names.rs）：名字的字號與詞性欄位、字表的位址；這版 DFHack 沒有這些定義就回傳 nil，名稱維持英文
local function name_layout()
    local ok, layout = pcall(function()
        local name, word = df.language_name._fields, df.language_word._fields
        return {words = name.words.offset, parts = name.parts_of_speech.offset,
                word_vec = select(2, df.global.world.raws.language.words:sizeof()),
                word_token = word.word.offset, word_forms = word.forms.offset}
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
    -- 名牌是設計過的樣子，一律用粗體，不跟著字重設定
    local cols, err = engine.render_mascot(src, png, w, h, MASCOT_ROWS, MASCOT_LABEL, AUTHOR, font_files(dir, 'bold'))
    if not cols then
        fail('吉祥物圖片讀取失敗：%s', err)
        return nil
    end
    return {handles = dfhack.textures.loadTileset(png, w, h, true), cols = cols}
end

-- 玩家在 DFHack 輸入框打的字（搜尋框、gm-editor 的欄位、gui/journal）照原樣畫：畫輸入框內容與切行時為 true，不查字典（見 install_typing）
local typing = false
-- DFHack 依英文長度置中的字（視窗標題、分頁標籤）畫的時候為 true：譯文在原文寬度的正中間（見 engine/src/hooks.rs dfhack_translate）
local centered = false

-- 回傳貼字圖的函式：把 engine.dh_translate 查到的字圖貼在 (x, y)；DFHack 在頂層開著時只寫頂層，字圖跟著貼頂層
local function stamper(engine)
    local gps = df.global.gps
    local _, gps_addr = df.sizeof(gps)
    return function(x, y) engine.dh_stamp(gps_addr, x, y, gps.top_in_use and 1 or 0) end
end

local function install_paint_string(engine, stamp)
    local screen = dfhack.screen
    if screen.dflang_paint_string then return end -- 只包一層
    local orig = screen.paintString
    screen.dflang_paint_string = orig
    -- 沒有譯文、照原樣畫的字也告訴引擎：整句貼圖用本包的字型重畫，不用遊戲的點陣字（見 engine/src/overlay.rs）
    local gps = df.global.gps
    local _, gps_addr = df.sizeof(gps)
    local function plain(x, y, text) engine.dh_plain(gps_addr, x, y, gps.top_in_use and 1 or 0, text) end
    screen.paintString = function(pen, x, y, text, map)
        if map or type(text) ~= 'string' then return orig(pen, x, y, text, map) end
        local shadow = not typing and engine.dh_translate(text, y, centered and 1)
        if not shadow then
            local ok = orig(pen, x, y, text, map)
            plain(x, y, text)
            return ok
        end
        local ok = orig(pen, x, y, shadow, map)
        stamp(x, y)
        return ok
    end
end

-- DFHack 的視窗框（gui.paint_frame）依英文標題長度把標題置中、「PAUSE FORCED」畫成有底色的小框：畫框時 centered 為 true。
-- 函式不在就略過，標題照原文起點畫
local function install_frame()
    local gui = require('gui')
    local orig = gui.paint_frame
    if type(orig) ~= 'function' or gui.dflang_paint_frame then return end
    gui.dflang_paint_frame = orig
    gui.paint_frame = function(...)
        centered = true
        return dfhack.with_finalize(function() centered = false end, orig, ...)
    end
end

-- DFHack 有些腳本讀畫面上的字找版面位置（overall-mood 找人物頁的「Overview」分頁、stressicons 找頂列的「Pop」）：
-- 翻成中文的格子回報原本的英文字元，腳本才找得到（見 engine/src/hooks.rs 的 English）。讀地圖緩衝區時照原樣
local function install_read_tile(engine)
    local screen = dfhack.screen
    if screen.dflang_read_tile then return end
    local orig = screen.readTile
    screen.dflang_read_tile = orig
    screen.readTile = function(x, y, map, ...)
        local pen = orig(x, y, map, ...)
        local ch = pen and not map and engine.english_at(x, y)
        if ch then pen.ch = ch end
        return pen
    end
end

-- 圖形模式的分頁標籤一格一字畫兩列（上列標 top_of_text、下列標 bottom_of_text，字往下位移半格），不經 paintString：
-- 標籤有譯文時兩列都畫影子字串，上下列各貼一次字圖，引擎依格子的旗標貼字的上半與下半（見 engine/src/hooks.rs paint）。
-- 分頁寬度照英文標籤算，譯文在標籤寬度裡置中。函式不在就略過，分頁照英文
local function install_tab(engine, stamp)
    local ok, TabBar = pcall(require, 'gui.widgets.tab_bar')
    local Tab = ok and type(TabBar) == 'table' and TabBar.Tab
    if type(Tab) ~= 'table' or type(Tab.onRenderBody) ~= 'function' or Tab.dflang_render then return end
    local orig = Tab.onRenderBody
    Tab.dflang_render = orig
    Tab.onRenderBody = function(self, dc)
        local shadow = dfhack.screen.inGraphicsMode() and type(self.label) == 'string' and engine.dh_translate(self.label, -1, 1)
        if not shadow or #shadow ~= #self.label then return orig(self, dc) end
        -- 標籤整個畫得出來才換（分頁列捲動時左右會被切掉一截，影子字串畫了字圖卻貼不上），否則照英文
        dc:seek(2, 0)
        local x, y = dc.x, dc.y
        if y < dc.clip_y1 or y + 1 > dc.clip_y2 or x < dc.clip_x1 or x + #shadow - 1 > dc.clip_x2 then return orig(self, dc) end
        local pens = self.get_pens()
        for row, p in ipairs{{pens.lt, pens.lt2, pens.t, pens.rt2, pens.rt}, {pens.lb, pens.lb2, pens.b, pens.rb2, pens.rb}} do
            dc:seek(0, row - 1)
            dc:char(nil, p[1]):char(nil, p[2])
            for i = 1, #shadow do dc:char(shadow:sub(i, i), p[3]) end
            dc:char(nil, p[4]):char(nil, p[5])
        end
        stamp(x, y)
        engine.dh_translate(self.label, -1, 1) -- 貼過的字圖會被清掉，下列再查一次
        stamp(x, y + 1)
    end
end

-- DFHack 的說明段落（WrappedLabel、確認對話框…）在 Lua 先用 string:wrap 切成英文行再一行行畫：原函式切完後把整段與英文行交給引擎，
-- 整段有譯文就依中文規則以同樣寬度重新換行，登記每一行要畫的中文（見 engine/src/hooks.rs 的 dfhack_wrap）。函式不在就略過，這些段落照英文
local function install_wrap(engine)
    local orig = string.wrap
    if type(orig) ~= 'function' or string.dflang_wrap then return end
    string.dflang_wrap = orig
    string.wrap = function(self, width, opts)
        local out = orig(self, width, opts)
        if typing then return out end
        local lines = out
        if type(out) == 'string' then
            lines = {}
            for line in (out .. '\n'):gmatch('([^\n]*)\n') do lines[#lines + 1] = line end
        end
        if type(lines) == 'table' and #lines > 0 and self:find('%a') then engine.dh_wrap(self, width or 72, lines) end
        return out
    end
end

-- 輸入框（EditField 也是 TextArea）畫內容、依框寬切行的時候把 typing 設成 true：玩家打的字照原樣，也不拿去查字典登記段落。
-- 函式不在就略過
local function install_typing()
    for _, spec in ipairs{{'gui.widgets.text_area.text_area_content', 'onRenderBody'}, {'gui.widgets.text_area.wrapped_text', 'update'}} do
        local ok, class = pcall(require, spec[1])
        local name = spec[2]
        local fn = ok and type(class) == 'table' and class[name]
        if type(fn) == 'function' and not class['dflang_' .. name] then
            class['dflang_' .. name] = fn
            class[name] = function(...)
                local was = typing
                typing = true
                return dfhack.with_finalize(function() typing = was end, fn, ...)
            end
        end
    end
end

-- 本 MOD 讓位時在標題畫面跳出說明：主控台的訊息玩家看不到，也不知道怎麼停用別的 MOD。
-- DFCN、DFI18n（都實測過）：只包住 DFHack 的畫字函式、不掛遊戲的 hook，用本包的字圖把說明畫成中文，讓玩家選要用哪一個；
-- 選「改用 DFLang」就呼叫 dflang.lua 對應的停用函式，重開遊戲生效。DFI18n 的字畫在 DFHack 視窗之後，
-- 標題選單的中文會透到說明框上，所以說明開著時用它自己的 dfi18n disable 暫停翻譯，關掉後恢復、拆掉本包的包裝
-- （DFCN 沒有從 Lua 暫停的方法，它自己會避開 DFHack 視窗蓋住的格子）
local NOTICE_TITLE = 'DFLang: another translation mod is active'

local function notice_engine(dir)
    local open = package.loadlib(dir .. 'dflang.dll', 'luaopen_dflang')
    local engine = open and open()
    local layout = engine and gps_layout()
    if not layout or not engine.set_layout(layout) or not load_glyphs(engine, dir)
            or not engine.load(dir .. 'strings.tsv') or not engine.enable() then
        return nil
    end
    install_paint_string(engine, stamper(engine))
    install_frame()
    install_wrap(engine)
    dfhack.run_command_silent('dfi18n', 'disable')
    return engine
end

local function release_notice(engine)
    if not engine then return end
    engine.disable()
    local screen = dfhack.screen
    if screen.dflang_paint_string then screen.paintString, screen.dflang_paint_string = screen.dflang_paint_string, nil end
    if string.dflang_wrap then string.wrap, string.dflang_wrap = string.dflang_wrap, nil end
    local gui = require('gui')
    if gui.dflang_paint_frame then gui.paint_frame, gui.dflang_paint_frame = gui.dflang_paint_frame, nil end
    dfhack.run_command_silent('dfi18n', 'enable')
end

local function notify(c, dir)
    local dialogs = require('gui.dialogs')
    local function show()
        if not dfhack.gui.matchFocusString('title/Default') then return dfhack.timeout(10, 'frames', show) end
        if c.notice then return dialogs.showMessage(NOTICE_TITLE, c.notice, COLOR_WHITE) end
        local engine = notice_engine(dir)
        local text = table.concat({
            ('%s is also installed. Only one translation engine'):format(c.name),
            'can run at a time, so DFLang is not running.',
            '',
            'Choose the one to use; it takes effect after you',
            'restart the game. To switch again later, type',
            ('dflang lang off (back to %s) or'):format(c.name),
            ('dflang lang %s (DFLang) in the DFHack console.'):format(LANG)}, '\n')
        dialogs.showListPrompt(NOTICE_TITLE, text, COLOR_WHITE,
            {('Turn off %s and use DFLang'):format(c.name), ('Keep using %s'):format(c.name)},
            function(choice)
                if choice ~= 1 then
                    dflang().set_lang('off')
                    return release_notice(engine)
                end
                dialogs.showMessage('DFLang', dflang()[c.off]() and ('%s has been turned off.\nRestart the game to use DFLang.'):format(c.name)
                    or c.off_failed, COLOR_WHITE, function() release_notice(engine) end)
            end,
            function() release_notice(engine) end)
    end
    dfhack.timeout(1, 'frames', show)
end

local function start()
    if dfhack.BASE_G.DFLANG then return end
    local other = conflict()
    local lang = chosen_lang()
    if lang == 'off' then
        if other then return log('已設定為改用 %s，本 MOD 不啟用（dflang lang %s 可改回）', other.name, LANG) end
        -- 選了改用的翻譯 MOD 已經移除：照常啟用，免得玩家以為本 MOD 壞了
        lang = default_lang()
    end
    if lang ~= LANG then
        log('另一個語言包啟用中，本包不載入（dflang lang %s 可切換）', LANG)
        return
    end
    local dir = mod_dir(MOD_ID)
    if not dir then return fail('找不到本 MOD 的目錄（%s）', MOD_ID) end
    dir = dir .. 'dflang/'
    if other then
        fail('偵測到 %s：兩個翻譯引擎會攔同一批函式，本 MOD 不啟動；標題畫面會說明怎麼切換。', other.name)
        return notify(other, dir)
    end

    local open, err = package.loadlib(dir .. 'dflang.dll', 'luaopen_dflang')
    if not open then return fail('載入引擎失敗：%s', err) end
    local engine = open()
    if not engine then return fail('引擎找不到 DFHack 的 lua53.dll') end

    local layout, missing = gps_layout()
    if not layout then return fail('這版 DFHack 的 gps 沒有欄位 %s', missing) end
    local ok
    ok, err = engine.set_layout(layout)
    if not ok then return fail('%s', err) end
    -- 出發準備畫面的分頁標籤逐字畫成字型表的貼圖；引擎要知道字型表在哪才認得出來。拿不到就只有那幾個標籤維持英文
    local fonts_ok, small, large = pcall(function()
        local font = df.global.init.font
        return select(2, font.small_font_texpos:sizeof()), select(2, font.large_font_texpos:sizeof())
    end)
    if fonts_ok then fonts_ok, err = engine.set_fonts(small, large) else err = small end
    if not fonts_ok then fail('找不到遊戲字型表（%s），出發準備畫面的分頁標籤維持英文', err) end
    local hooks = {table.unpack(HOOKS)}
    local mtb = mtb_layout()
    if mtb and engine.set_mtb_layout(mtb) then
        table.insert(hooks, 'mtb_set_width')
    else
        fail('這版 DFHack 沒有段落結構定義，教學與說明文字維持英文')
    end
    local ctb = ctb_layout()
    local ctb_ok = ctb and engine.set_ctb_layout(ctb)
    if not ctb_ok then fail('這版 DFHack 沒有彩色文字框的結構定義，人物頁的想法與性格維持英文') end
    local name = name_layout()
    local names_ok = name and engine.set_name_layout(name)
    if not names_ok then fail('這版 DFHack 沒有名稱的結構定義，世界生成的名稱維持英文') end

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
    local screens, bad_screens, missing_screen_glyphs = engine.load_screens(dir .. 'screens.tsv')
    if not screens then return fail('%s', bad_screens) end
    if bad_screens > 0 or missing_screen_glyphs > 0 then
        fail('依畫面的譯文有 %d 條原文無法比對、%d 個字沒有字圖', bad_screens, missing_screen_glyphs)
    end
    if names_ok then
        local words
        words, err = engine.load_names(dir .. 'names.tsv')
        if not words then
            fail('%s；世界生成的名稱維持英文', err)
            names_ok = false
        end
    end

    ok, err = attach_hooks(engine, dir, hooks)
    if not ok then return fail('%s', err) end
    -- 提示框等自動換行的文字：wrap_lines 實際切行（wrap_text 只登記位址，enable 時用來補做遊戲啟動時就切好的提示）；找不到只有這些維持英文
    ok, err = attach_hooks(engine, dir, {'wrap_text', 'wrap_lines'})
    if not ok then fail('%s；提示框文字維持英文', err) end
    -- 遊戲截成「開頭...」的清單字串：記下截短前的原文來翻；找不到只有截短的字串維持英文
    ok, err = attach_hooks(engine, dir, {'string_resize'})
    if not ok then fail('%s；截短的清單字串維持英文', err) end
    -- 冒險者開局畫面的外觀、性格段落（遊戲自己切行）；找不到只有這幾段維持英文
    ok, err = attach_hooks(engine, dir, {'adv_appearance', 'adv_personality'})
    if not ok then fail('%s；冒險者開局畫面的外觀與性格維持英文', err) end
    -- 冒險模式上方的訊息框（交談紀錄，遊戲自己切行）；找不到只有這個框維持英文
    ok, err = attach_hooks(engine, dir, {'adv_log', 'adv_talk_log'})
    if not ok then fail('%s；冒險模式的交談紀錄維持英文', err) end
    -- 彩色文字框（人物頁的想法、記憶、性格）；找不到只有這些維持英文
    if ctb_ok then
        ok, err = attach_hooks(engine, dir, {'ctb_add_string'})
        if not ok then fail('%s；人物頁的想法與性格維持英文', err) end
    end
    -- 世界生成的名稱（攔遊戲把名字轉成英文的函式）；找不到只有名稱維持英文
    if names_ok then
        ok, err = attach_hooks(engine, dir, {'translate_name'})
        if not ok then fail('%s；世界生成的名稱維持英文', err) end
    end
    -- 整句貼圖：畫面上的字改用本包的字型、照字寬整句畫（見 engine/src/overlay.rs）。預設開啟；
    -- dflang overlay off 改回一字兩格的字圖（重開遊戲生效）。掛不上時照一字兩格
    ok, err = attach_hooks(engine, dir, {'render_things'})
    if ok then ok, err = engine.overlay_install(select(2, df.sizeof(df.global.enabler.renderer))) end
    if not ok then
        fail('%s；整句貼圖不能用，改用一字兩格的字圖', err)
    elseif json.open(CONFIG_FILE).data.overlay ~= false then
        engine.set_overlay(1)
    end
    ok, err = engine.enable()
    if not ok then return fail('%s', err) end
    -- DFHack 自己的視窗（overlay、gui/ 腳本）不經遊戲的畫字函式：包住 dfhack.screen.paintString，有譯文就畫影子字串
    -- （格數照英文，版面、點擊不變）再貼字圖。引擎啟用時就裝，趕在腳本把它存成區域變數之前（gui/load-screen.lua）
    local stamp = stamper(engine)
    install_paint_string(engine, stamp)
    install_wrap(engine)
    install_tab(engine, stamp)
    install_frame()
    install_read_tile(engine)
    install_typing()
    -- 依畫面套用的譯文要知道現在在哪個畫面：每幀把 DFHack 的焦點字串告訴引擎（'frames' 計時器在世界卸載後也繼續）。
    -- DFHack 的視窗開著時也加上底下遊戲畫面的焦點：遊戲照樣畫的上方資訊列（「酒類」）才不會換成別的譯法
    if screens > 0 then
        local function track_focus()
            local focus = dfhack.gui.getCurFocus(true)
            local dfvs = dfhack.gui.getDFViewscreen(true)
            if dfvs and dfvs ~= dfhack.gui.getCurViewscreen(true) then
                for _, f in ipairs(dfhack.gui.getFocusStrings(dfvs)) do table.insert(focus, f) end
            end
            engine.set_focus(table.concat(focus, '\n'))
            dfhack.timeout(1, 'frames', track_focus)
        end
        track_focus()
    end
    if json.open(CONFIG_FILE).data.misses then
        ok, err = engine.log_misses(MISSES_FILE)
        if not ok then fail('無法記錄未翻譯字串：%s', err) end
    end

    -- 每字八張，保留範圍（約 8,800 張可用）放不下，後面的落在 DFHack 的動態範圍：遊戲重載貼圖後 texpos 會變，
    -- 最後一張一定在動態範圍（或全部都在保留範圍、永遠不變），看它就知道要不要重查
    local last = dfhack.textures.getTexposByHandle(handles[#handles])
    -- started：這次啟動時的整句貼圖、字身、字重設定，設定視窗（dflang ui）用來判斷改過的設定要不要提示重開
    local config = json.open(CONFIG_FILE).data
    dfhack.BASE_G.DFLANG = {engine = engine, lang = LANG, dir = dir, config = CONFIG_FILE,
                            mascot = load_mascot(engine, dir),
                            started = {overlay = config.overlay ~= false, overlay_size = config.overlay_size or 14,
                                       bold = config.bold ~= false}}
    dfhack.onStateChange.dflang = function(sc)
        if sc ~= SC_VIEWSCREEN_CHANGED and sc ~= SC_WORLD_LOADED and sc ~= SC_WORLD_UNLOADED then return end
        local now = dfhack.textures.getTexposByHandle(handles[#handles])
        if now == last then return end
        last = now
        engine.update_texpos(resolve(handles))
    end
    log('已啟用：%d 條譯文、%d 條句型、%d 個字圖', loaded, rules, glyphs)
end

local ok, err = pcall(start)
if not ok then fail('啟動失敗：%s', err) end

-- 設定視窗的快捷鍵：只在記憶體註冊、不寫進 init 檔，MOD 移除後下次啟動就沒有（玩家在 gui/keybinds 存過檔的除外，
-- 按了只會顯示找不到指令）。已經有綁到同一個指令的（另一包先註冊了、或玩家自己存的鍵）就不再加。
-- 讓位給其他翻譯 MOD 時也註冊：視窗裡可以改回 DFLang
local HOTKEY, HOTKEY_COMMAND = 'Ctrl-Shift-L', 'dflang ui'
local function bind_hotkey()
    for _, bind in ipairs(dfhack.hotkey.listAllKeybinds()) do
        if bind.command == HOTKEY_COMMAND then return end
    end
    if not dfhack.hotkey.addKeybind(HOTKEY, HOTKEY_COMMAND) then fail('無法註冊快捷鍵 %s', HOTKEY) end
end
ok, err = pcall(bind_hotkey)
if not ok then fail('無法註冊快捷鍵 %s：%s', HOTKEY, err) end

-- DFHack overlay 會掃描所有模組腳本的 OVERLAY_WIDGETS；玩家可在 gui/overlay 移動或關閉這個圖示。
-- 兩包都裝時兩個小工具都會註冊，只有被選用的那包畫圖。
local overlay = require('plugins.overlay')

-- 只有被選用的那包（DFLANG.lang 是本包語言）才畫、才收點擊
local function active()
    local state = dfhack.BASE_G.DFLANG
    return state and state.lang == LANG and state
end

MascotOverlay = defclass(MascotOverlay, overlay.OverlayWidget)
MascotOverlay.ATTRS{
    -- gui/overlay 照 CP437 畫說明：寫英文，譯文在 data/strings/dflang_ui.tsv
    desc = 'DFLang mascot on the title page; click it to open the Chinese translation settings.',
    default_pos = {x = 21, y = -5},
    default_enabled = true,
    viewscreens = 'title/Default',
    frame = {w = 16, h = MASCOT_ROWS},
}

-- 畫在頂層（screentexpos_top）：主層有貼圖的格子會先填底色，圖片透明處會變成黑框；頂層直接疊在標題背景上。
-- 標題畫面本來沒開頂層，要設 top_in_use；遊戲每幀會清空頂層，離開標題畫面後不會殘留。
function MascotOverlay:onRenderBody()
    local state = active()
    local mascot = state and state.mascot
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

-- 點吉祥物開設定視窗（同 Ctrl-Shift-L）。呼叫本包的 dflang.lua，不經腳本路徑（兩包、新舊版可能同時在路徑上）
function MascotOverlay:onInput(keys)
    local state = active()
    if keys._MOUSE_L and state and state.mascot and self:getMousePos() then
        dflang().show_ui()
        return true
    end
end

-- 名牌正下方提示設定視窗的快捷鍵，樣式照 DFHack 標題畫面的「?: 快速入門指南」（綠色按鍵名＋白字、黑底）；點一下也開視窗。
-- 顯示實際綁到 dflang ui 的鍵（玩家可能改綁），沒有綁定就不畫。三段各自畫，說明那段由引擎查字典畫成中文；
-- 黑底照英文的格數畫，英文寫短一點，中文後面才不會拖一長段黑底
local HINT_TEXT = 'DFLang settings'

local function bound_key()
    for _, bind in ipairs(dfhack.hotkey.listAllKeybinds()) do
        if bind.command == HOTKEY_COMMAND then return (bind.spec:gsub('@.*', '')) end
    end
end

HintOverlay = defclass(HintOverlay, overlay.OverlayWidget)
HintOverlay.ATTRS{
    desc = 'DFLang hotkey hint on the title page: the key that opens the Chinese translation settings.',
    default_pos = {x = 21, y = -4},
    default_enabled = true,
    viewscreens = 'title/Default',
    frame = {w = 48, h = 1},
}

function HintOverlay:onRenderBody(dc)
    local key = active() and bound_key()
    if not key then return end
    dc:pen(COLOR_LIGHTGREEN):string(key):pen(COLOR_WHITE):string(': '):string(HINT_TEXT)
end

function HintOverlay:onInput(keys)
    if keys._MOUSE_L and active() and self:getMousePos() then
        dflang().show_ui()
        return true
    end
end

OVERLAY_WIDGETS = {mascot = MascotOverlay, hint = HintOverlay}
