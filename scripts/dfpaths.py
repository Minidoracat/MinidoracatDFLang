"""遊戲、DFHack 與工作坊的安裝位置。

環境變數 DF_PATH／DFHACK_PATH 優先；沒設就讀 Steam 登錄檔的 SteamPath，
在 libraryfolders.vdf 列出的每個遊戲庫裡找 appmanifest_<appid>.acf。
"""
import os
import re
from pathlib import Path

DF_APPID = 975370
DFHACK_APPID = 2346660


def steam_root() -> Path | None:
    try:
        import winreg
        with winreg.OpenKey(winreg.HKEY_CURRENT_USER, r"Software\Valve\Steam") as key:
            return Path(winreg.QueryValueEx(key, "SteamPath")[0])
    except (ImportError, OSError):
        return None


def steamapps_with(appid: int) -> Path | None:
    root = steam_root()
    if root is None:
        return None
    vdf = root / "steamapps/libraryfolders.vdf"
    libraries = [root]
    if vdf.is_file():
        libraries += [Path(p.replace("\\\\", "\\")) for p in re.findall(r'"path"\s+"([^"]+)"', vdf.read_text(encoding="utf-8"))]
    for lib in libraries:
        if (lib / "steamapps" / f"appmanifest_{appid}.acf").is_file():
            return lib / "steamapps"
    return None


def _app_dir(appid: int, env: str, folder: str) -> Path | None:
    if os.environ.get(env):
        return Path(os.environ[env])
    steamapps = steamapps_with(appid)
    return steamapps / "common" / folder if steamapps else None


DF = _app_dir(DF_APPID, "DF_PATH", "Dwarf Fortress")
if DF is None:
    raise SystemExit(f"找不到 Dwarf Fortress（Steam app {DF_APPID}）；請設定環境變數 DF_PATH")
DFHACK = _app_dir(DFHACK_APPID, "DFHACK_PATH", "DFHack") or DF.parent / "DFHack"
STEAMAPPS = DF.parent.parent
WORKSHOP = STEAMAPPS / "workshop/content" / str(DF_APPID)
# 反編譯快照（scripts/df_snapshot.py 產生）：repo 旁的 df-decompiled-reference/snapshots/<版本>-<YYYYMMDD>/
SNAPSHOTS = Path(__file__).resolve().parents[2] / "df-decompiled-reference/snapshots"


def latest_snapshot() -> Path | None:
    """日期最新、且有 exe 字串表的快照；沒有快照時回傳 None。"""
    snaps = [p for p in SNAPSHOTS.glob("*-*") if (p / "exe/strings.tsv").is_file()] if SNAPSHOTS.is_dir() else []
    return max(snaps, key=lambda p: p.name.rsplit("-", 1)[1], default=None)
