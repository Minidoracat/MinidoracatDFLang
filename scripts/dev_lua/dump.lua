-- 讀檔後列舉遊戲裡實際存在的名稱，逐條用引擎查譯文，翻不出來的寫進 dfhack-config/dflang-dump.tsv。
-- 用法：`dev_game.py up` 會裝成 dfhack-config/scripts/dflang-dev-dump.lua；進要塞或傳說後 `dev_game.py run dflang-dev-dump`，
-- `down` 把輸出另存到 temp/dumps/。MISS 的條數就是這份存檔裡名稱的漏翻數。
-- 欄位：來源、原文、譯文或 MISS。物品描述照遊戲的 getItemDescription（DFHack items.getDescription），
-- 建築名照遊戲的 getName，職業照 units.getProfessionName，職位名取勢力的 positions。
local engine = dfhack.BASE_G.DFLANG and dfhack.BASE_G.DFLANG.engine
if not engine then qerror('DFLang is not loaded') end

-- 遊戲的字串是 CP437 位元組，engine.translate 收 UTF-8：先用 dfhack.df2utf 轉（♂、☼、« 這類字元直接傳會查不到）
local seen, out, misses, total = {}, {}, 0, 0
local function record(source, text)
    if type(text) ~= 'string' or text == '' or seen[text] then return end
    seen[text] = true
    total = total + 1
    local utf = dfhack.df2utf(text)
    local zh = engine.translate(utf)
    if not zh then
        misses = misses + 1
        out[#out + 1] = source .. '\t' .. utf .. '\tMISS'
    end
end

for _, item in ipairs(df.global.world.items.all) do
    for _, mode in ipairs{0, 1, 2} do
        local ok, s = pcall(dfhack.items.getDescription, item, mode, false)
        if ok then record('item' .. mode, s) end
    end
    local ok, s = pcall(dfhack.items.getDescription, item, 0, true)
    if ok then record('item0d', s) end
end

for _, unit in ipairs(df.global.world.units.all) do
    local ok, s = pcall(dfhack.units.getProfessionName, unit)
    if ok then record('profession', s) end
    ok, s = pcall(dfhack.units.getProfessionName, unit, true)
    if ok then record('profession_plural', s) end
end

for _, ent in ipairs(df.global.world.entities.all) do
    for _, pos in ipairs(ent.positions.own) do
        for _, field in ipairs{'name', 'name_male', 'name_female', 'spouse', 'spouse_male', 'spouse_female'} do
            local v = pos[field]
            if v then
                record('position', v[0])
                record('position_plural', v[1])
            end
        end
    end
end

for _, bld in ipairs(df.global.world.buildings.all) do
    local ok, s = pcall(dfhack.buildings.getName, bld)
    if ok then record('building', s) end
end

local link = df.global.world.jobs.list.next
while link do
    if link.item then
        local ok, s = pcall(dfhack.job.getName, link.item)
        if ok then record('job', s) end
    end
    link = link.next
end

for _, wc in ipairs(df.global.world.written_contents.all) do
    record('title', wc.title)
end

local f = io.open('dfhack-config/dflang-dump.tsv', 'w')
f:write(table.concat(out, '\n'), '\n')
f:close()
print(('%d strings, %d MISS -> dfhack-config/dflang-dump.tsv'):format(total, misses))
