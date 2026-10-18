-- 用遊戲自己的工坊按鈕標籤函式（dfhack.job.getName）列出工作名稱；參數：起訖 job_type 編號
-- 用法：`dev_game.py up` 會裝成 dflang-dev-jobnames；`dev_game.py run dflang-dev-jobnames 0 29`（一次 30 種），
-- 結果附加到 dfhack-config/dflang-jobnames.tsv，`down` 另存到 temp/dumps/。
-- item_subtype 用 0（-1 會讓需要物品子類的工作讀到不存在的定義而當掉）
local args = {...}
local first, last = tonumber(args[1]), tonumber(args[2])
local mats = {}
for _, token in ipairs{'INORGANIC:GRANITE', 'INORGANIC:IRON', 'INORGANIC:GOLD', 'INORGANIC:HEMATITE', 'PLANT:OAK:WOOD',
                       'CREATURE:YAK:LEATHER', 'PLANT:GRASS_TAIL_PIG:THREAD'} do
    local mi = dfhack.matinfo.find(token)
    if mi then mats[#mats + 1] = {mi.type, mi.index} end
end
local seen, out = {}, {}
for jt = first, last do
    if df.job_type[jt] and df.job_type[jt] ~= 'CustomReaction' then
        for _, m in ipairs(mats) do
            local job = df.job:new()
            job.job_type = jt
            job.mat_type, job.mat_index = m[1], m[2]
            job.item_subtype = 0
            local ok, name = pcall(dfhack.job.getName, job)
            job:delete()
            if ok and name and name ~= '' and not seen[name] then
                seen[name] = true
                out[#out + 1] = tostring(df.job_type[jt]) .. '\t' .. name
            end
        end
    end
end
local f = io.open('dfhack-config/dflang-jobnames.tsv', 'a')
f:write(table.concat(out, '\n'), '\n')
f:close()
print(first .. '-' .. last .. ': ' .. #out .. ' job names')
