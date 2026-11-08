-- 讓位給同一台機器上 PZ 家族 E2E 的效能量測：lease 裡標了 timing／exclusive 的輪次一放行，就立刻關遊戲（die，不存檔）。
-- dev_game.py up 開好遊戲後自動執行；手動：`dflang-dev-pzyield [lease 檔]`，省略時讀 %LOCALAPPDATA%/pz-family-e2e-lease.json。
-- 約每 5 秒（250 幀）查一次；frames 計時器不隨世界載入、卸載取消，遊戲關掉就跟著停，不留背景程序。
local repeatUtil = require('repeat-util')
local json = require('json')
local path = ({...})[1] or ((os.getenv('LOCALAPPDATA') or '') .. '/pz-family-e2e-lease.json')

local function granted_perf_run()
    local ok, lease = pcall(json.decode_file, path)
    if not ok or type(lease) ~= 'table' then return nil end
    for _, e in ipairs(lease.granted or {}) do
        if e.timing or e.exclusive then return tostring(e.run) end
    end
end

repeatUtil.scheduleEvery('dflang-dev-pzyield', 250, 'frames', function()
    local run = granted_perf_run()
    if run then
        dfhack.printerr('dflang-dev-pzyield: PZ perf run ' .. run .. ' granted, closing DF')
        dfhack.run_command('die')
    end
end)
print('dflang-dev-pzyield: watching ' .. path)
