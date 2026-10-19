-- 量這個世界的名字翻不翻得出來、放不放得下：改了名字、音譯或寬度判斷之後跑，回報它印的數字。
-- 用法：`dev_game.py up --save perf-fort`，進要塞後 `dev_game.py run dflang-dev-namefit`。名字音譯打開，整句貼圖開、關各量一次，
-- 量完把 dflang.json 的 names、overlay 放回原值。照英文的例子寫進 dfhack-config/dflang-namefit.tsv（`down` 另存到 temp/dumps/）。
-- 量的是畫字時同一套判斷：靠左的名字用 engine.dh_translate（放不下時壓窄或改窄字，見 engine/src/dict.rs 的 rule_entry），
-- 置中的句型（出發與收復準備、傳說標題、圖像編輯器標題）照一字兩格和原文比。
-- 最後一行 PASS：兩種畫法都沒有因為太寬照英文的，人物原名也都翻得出來（音譯開著時每個原名都音譯得出來；狗頭人這類沒有語言的名字
-- 沒登記成原名時就會照英文）。英文名沒有譯文的另外列數字（名稱字表缺字的稱號，見 docs/analysis.md 的名字音譯），不算 FAIL。
local state = dfhack.BASE_G.DFLANG
if not state then qerror('DFLang is not loaded') end
if not dfhack.isWorldLoaded() then qerror('load a save first') end
local engine, json, tr = state.engine, require('json'), dfhack.translation.translateName
local w = df.global.world

-- 遊戲畫過的原名才登記（hook_native_name）；人物的原名先全部登記，當成遊戲都組過
for _, f in ipairs(w.history.figures) do
    if f.name.has_name and f.name.language >= 0 then engine.note_native(tr(f.name, false)) end
end

-- 一字兩格的格數：CP437 有的字一格，其他（中文、全形標點）兩格
local function cells(zh)
    local n = 0
    for _, cp in utf8.codes(zh) do
        n = n + ((cp < 0x80 or dfhack.utf2df(utf8.char(cp)) ~= '?') and 1 or 2)
    end
    return n
end

local samples = {}
local function sample(list, kind, mode, text, result)
    if #list < 20 then table.insert(samples, ('%s\t%s\t%s\t%s'):format(kind, mode, dfhack.df2utf(text), result)) list[#list + 1] = true end
end

local function measure(mode)
    local rows, order = {}, {}
    local function row(kind)
        if not rows[kind] then rows[kind] = {n = 0, none = 0, wide = 0, ex_none = {}, ex_wide = {}}; order[#order + 1] = kind end
        return rows[kind]
    end
    local function left(kind, text)
        local r = row(kind)
        r.n = r.n + 1
        local zh = engine.translate(text)
        if not zh then r.none = r.none + 1; sample(r.ex_none, kind, mode, text, 'MISS')
        elseif not engine.dh_translate(text, -1, 0) then r.wide = r.wide + 1; sample(r.ex_wide, kind, mode, text, 'WIDE ' .. zh) end
    end
    local function centered(kind, text)
        local r = row(kind)
        r.n = r.n + 1
        local zh = engine.translate(text)
        if not zh then r.none = r.none + 1; sample(r.ex_none, kind, mode, text, 'MISS')
        elseif cells(zh) > #text then r.wide = r.wide + 1; sample(r.ex_wide, kind, mode, text, 'WIDE ' .. zh) end
    end
    for _, f in ipairs(w.history.figures) do
        if f.name.has_name then
            left('人物原名', tr(f.name, false))
            left('人物英文名', tr(f.name, true))
        end
    end
    for _, u in ipairs(dfhack.units.getCitizens()) do
        left('居民「名字，職業」', tr(u.name, false) .. ', ' .. dfhack.units.getProfessionName(u))
    end
    for _, r in ipairs(w.world_data.regions) do
        if r.name.has_name then
            local n = tr(r.name, true)
            centered('置中：出發準備（地區）', '  Prepare for the Journey to ' .. n .. '  ')
            centered('置中：收復準備（地區）', '  Prepare for the Expedition to ' .. n .. '  ')
        end
    end
    for _, s in ipairs(w.world_data.sites) do
        if s.name.has_name then centered('置中：收復準備（據點）', '  Prepare for the Expedition to ' .. tr(s.name, true) .. '  ') end
    end
    centered('置中：傳說標題（世界）', 'Explore the history of ' .. tr(w.world_data.name, true) .. '.')
    for _, en in ipairs(w.entities.all) do
        if en.type == df.historical_entity_type.Civilization and en.name.has_name then
            centered('置中：圖像編輯器（文明）', 'The symbol of ' .. tr(en.name, true))
        end
    end
    local wide = 0
    print(('== 名字音譯開、整句貼圖%s'):format(mode))
    for _, kind in ipairs(order) do
        local r = rows[kind]
        print(('%s 共 %d：沒有譯文 %d、太寬照英文 %d'):format(kind, r.n, r.none, r.wide))
        wide = wide + r.wide
    end
    return wide, rows['人物原名'].none
end

local function set(names, overlay)
    local c = json.open('dfhack-config/dflang.json')
    c.data.names, c.data.overlay = names, overlay
    c:write()
    state.apply()
end

local saved = json.open('dfhack-config/dflang.json').data
local names0, overlay0 = saved.names, saved.overlay
local ok, result = pcall(function()
    set(true, true)
    local wide_on, none_on = measure('開')
    set(true, false)
    local wide_off, none_off = measure('關')
    return {wide = wide_on + wide_off, none = none_on + none_off}
end)
set(names0, overlay0)
if not ok then error(result, 0) end

local f = io.open('dfhack-config/dflang-namefit.tsv', 'w')
f:write('kind\toverlay\ttext\tresult\n', table.concat(samples, '\n'), '\n')
f:close()
if result.wide == 0 and result.none == 0 then
    print('PASS：兩種畫法都沒有太寬照英文的，人物原名都翻得出來')
else
    print(('FAIL：太寬照英文 %d 個、人物原名沒有譯文 %d 個，例子在 dfhack-config/dflang-namefit.tsv'):format(result.wide, result.none))
end
