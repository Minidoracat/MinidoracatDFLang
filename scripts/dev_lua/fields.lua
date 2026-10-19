-- 列出一個 DFHack 結構在某段位移裡有哪些欄位（位移、名稱、值）：hook 寫死記憶體位移之前，先用它對 DFHack 的欄位名。
-- 用法：`dev_game.py run dflang-dev-fields "<Lua 運算式>" [起] [迄]`，起訖可寫 0x 開頭的十六進位，省略時列出全部。
-- 例：`dflang-dev-fields "dfhack.gui.getDFViewscreen(true)" 0x8300 0x8400`（世界畫面）、`dflang-dev-fields df.global.enabler 0x380 0x390`
local args = {...}
if not args[1] then qerror('usage: dflang-dev-fields "<lua expression>" [from] [to]') end
local fn, err = load('return ' .. args[1], 'expression', 't', _ENV)
if not fn then qerror(err) end
local obj = fn()
if type(obj) ~= 'userdata' or not obj._type then qerror('not a DFHack object: ' .. tostring(obj)) end
local from, to = tonumber(args[2] or '0'), tonumber(args[3] or '0x7fffffff')
local _, base = df.sizeof(obj)
local rows = {}
for name, value in pairs(obj) do
    local ok, _, addr = pcall(function() return df.sizeof(obj:_field(name)) end)
    if ok and addr and addr - base >= from and addr - base <= to then
        rows[#rows + 1] = {addr - base, ('0x%05x  %s  %s'):format(addr - base, name, tostring(value):sub(1, 60))}
    end
end
table.sort(rows, function(a, b) return a[1] < b[1] end)
print(('%s @ 0x%x'):format(tostring(obj._type), base))
for _, row in ipairs(rows) do print(row[2]) end
