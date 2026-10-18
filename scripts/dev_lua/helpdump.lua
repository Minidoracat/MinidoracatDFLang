-- 開發用（不放進工作坊包）：把目前說明視窗每一頁的段落原文，用和引擎相同的格式追加到 dflang-misses.tsv。
-- 用法：`dev_game.py up` 會裝成 dflang-dev-helpdump；開著說明指南時 `dev_game.py run dflang-dev-helpdump`，每頁以 mtb 記進未翻譯紀錄。
-- 說明視窗只排版正在看的那一頁，引擎只記得到那一頁；這裡直接讀每一頁的單字清單。
local h = df.global.game.main_interface.help
local pal = df.global.gps.uccolor
local function index_of(c)
    for i = 0, 15 do
        if pal[i][0] == c[1] and pal[i][1] == c[2] and pal[i][2] == c[3] then return i end
    end
end
local out = io.open('dfhack-config/dflang-misses.tsv', 'a')
local n = 0
for p = 0, #h.text - 1 do
    local box = h.text[p]
    local parts, color, ok = {}, nil, #box.word > 0
    for _, w in ipairs(box.word) do
        if w.link_index ~= -1 then ok = false break end
        local f = w.flags
        local marker = f.NEW_LINE or f.BLANK_LINE or f.INDENT
        local tok = f.NEW_LINE and '[R]' or f.BLANK_LINE and '[B]' or f.INDENT and '[P]' or w.str
        if tok ~= '' then
            local sep = (#parts > 0 and not tok:match('^[.,?!]$')) and ' ' or ''
            local mark = ''
            if not marker then
                local c = {w.red, w.green, w.blue}
                if color and (color[1] ~= c[1] or color[2] ~= c[2] or color[3] ~= c[3]) then
                    local i = index_of(c)
                    if not i then ok = false break end
                    mark = ('[C:%d:0:%d]'):format(i % 8, i // 8)
                end
                color = c
            end
            table.insert(parts, sep .. mark .. tok)
        end
    end
    if ok then
        out:write('mtb\t' .. dfhack.df2utf(table.concat(parts)) .. '\n')
        n = n + 1
    end
end
out:close()
print(('dumped %d pages: %s'):format(n, dfhack.df2utf(h.header)))
