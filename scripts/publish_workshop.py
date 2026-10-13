# /// script
# requires-python = ">=3.11"
# ///
"""把 dist/ 的繁、簡兩個工作坊包各自上傳到 Steam 工作坊（Steamworks ISteamUGC，ctypes 載入遊戲的 steam_api64.dll）。

預設只檢查並列出會做的事；加 --yes 才真的連 Steam 上傳。上傳是對外發布，執行前要有使用者明確同意。

用法（repo 根目錄，Steam 用戶端要開著並登入擁有 Dwarf Fortress 的帳號）：
  uv run scripts/publish_workshop.py --item zh-Hant --note "更新說明"          # 檢查
  uv run scripts/publish_workshop.py --item all --note "更新說明" --yes        # 兩包各自上傳
  uv run scripts/publish_workshop.py --item zh-Hans --create --yes             # 首次建立項目（設為私人），ID 寫回 workshop.json

每次上傳的內容：dist/<folder>/ 整個資料夾、標題（steam_title）、說明（steam_description 檔）、
說明語言槽（steam_language）、標籤（tags）、預覽圖（preview，有設才傳）、更新說明（--note）。
"""
import argparse
import ctypes
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from dfpaths import DF

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / "scripts/workshop.json"
STEAM_API = DF / "steam_api64.dll"
INVALID_HANDLE = (0, 0xFFFFFFFFFFFFFFFF)
VISIBILITY_PRIVATE = 2  # k_ERemoteStoragePublishedFileVisibilityPrivate
FILE_TYPE_COMMUNITY = 0  # k_EWorkshopFileTypeCommunity


class StringArray(ctypes.Structure):
    _fields_ = [("strings", ctypes.POINTER(ctypes.c_char_p)), ("count", ctypes.c_int32)]


class Steam:
    """遊戲附的 steam_api64.dll（ISteamUGC v016）；SteamAPI_Init 讀工作目錄的 steam_appid.txt，放在暫存目錄。"""

    def __init__(self, app_id: int):
        self.prev_cwd = os.getcwd()
        self.tmp = tempfile.mkdtemp(prefix="dflang-workshop-")
        Path(self.tmp, "steam_appid.txt").write_text(str(app_id))
        os.chdir(self.tmp)
        d = self.dll = ctypes.CDLL(str(STEAM_API))
        for name in ("SteamAPI_SteamUser_v021", "SteamAPI_SteamUGC_v016", "SteamAPI_SteamUtils_v010"):
            getattr(d, name).restype = ctypes.c_void_p
        d.SteamAPI_Init.restype = ctypes.c_bool
        d.SteamAPI_GetHSteamPipe.restype = ctypes.c_int32
        d.SteamAPI_ManualDispatch_RunFrame.argtypes = [ctypes.c_int32]
        d.SteamAPI_ISteamUser_BLoggedOn.argtypes = [ctypes.c_void_p]
        d.SteamAPI_ISteamUser_BLoggedOn.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_CreateItem.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_int]
        d.SteamAPI_ISteamUGC_CreateItem.restype = ctypes.c_uint64
        d.SteamAPI_ISteamUGC_StartItemUpdate.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint64]
        d.SteamAPI_ISteamUGC_StartItemUpdate.restype = ctypes.c_uint64
        for name in ("SetItemUpdateLanguage", "SetItemTitle", "SetItemDescription", "SetItemContent", "SetItemPreview"):
            fn = getattr(d, "SteamAPI_ISteamUGC_" + name)
            fn.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_char_p]
            fn.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_SetItemTags.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.POINTER(StringArray)]
        d.SteamAPI_ISteamUGC_SetItemTags.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_SetItemVisibility.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_int]
        d.SteamAPI_ISteamUGC_SetItemVisibility.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_SubmitItemUpdate.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_char_p]
        d.SteamAPI_ISteamUGC_SubmitItemUpdate.restype = ctypes.c_uint64
        d.SteamAPI_ISteamUtils_IsAPICallCompleted.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.POINTER(ctypes.c_bool)]
        d.SteamAPI_ISteamUtils_IsAPICallCompleted.restype = ctypes.c_bool
        d.SteamAPI_ISteamUtils_GetAPICallResult.argtypes = [
            ctypes.c_void_p, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.POINTER(ctypes.c_bool)]
        d.SteamAPI_ISteamUtils_GetAPICallResult.restype = ctypes.c_bool
        d.SteamAPI_ManualDispatch_Init()
        if not d.SteamAPI_Init():
            self.close(shutdown=False)
            raise RuntimeError("SteamAPI_Init 失敗：Steam 用戶端沒開、沒登入，或帳號沒有 Dwarf Fortress")
        self.user, self.ugc, self.utils = d.SteamAPI_SteamUser_v021(), d.SteamAPI_SteamUGC_v016(), d.SteamAPI_SteamUtils_v010()
        self.pipe = d.SteamAPI_GetHSteamPipe()
        if not d.SteamAPI_ISteamUser_BLoggedOn(self.user):
            raise RuntimeError("Steam 沒有登入")

    def close(self, shutdown: bool = True) -> None:
        if shutdown:
            self.dll.SteamAPI_Shutdown()
        os.chdir(self.prev_cwd)

    def wait(self, call: int, callback_id: int, size: int) -> bytes:
        failed = ctypes.c_bool(False)
        deadline = time.time() + 3600
        while time.time() < deadline:
            self.dll.SteamAPI_ManualDispatch_RunFrame(self.pipe)
            if self.dll.SteamAPI_ISteamUtils_IsAPICallCompleted(self.utils, call, ctypes.byref(failed)):
                buf = ctypes.create_string_buffer(size)
                ok = self.dll.SteamAPI_ISteamUtils_GetAPICallResult(self.utils, call, buf, size, callback_id, ctypes.byref(failed))
                if not ok or failed.value:
                    raise RuntimeError(f"Steam 呼叫 {callback_id} 失敗")
                return buf.raw
            time.sleep(0.5)
        raise RuntimeError("等待 Steam 回應超時")

    def create(self, app_id: int) -> int:
        # CreateItemResult_t（k_iSteamUGCCallbacks + 3）：EResult@0、PublishedFileId@8、需要接受協議@16
        raw = self.wait(self.dll.SteamAPI_ISteamUGC_CreateItem(self.ugc, app_id, FILE_TYPE_COMMUNITY), 3403, 24)
        if int.from_bytes(raw[:4], "little") != 1:
            raise RuntimeError(f"CreateItem EResult={int.from_bytes(raw[:4], 'little')}")
        if raw[16]:
            raise RuntimeError("需要先到 Steam 網頁接受工作坊法律協議")
        return int.from_bytes(raw[8:16], "little")

    def update(self, app_id: int, file_id: int, item: dict, content: Path, note: str, private: bool) -> None:
        d, ugc = self.dll, self.ugc
        handle = d.SteamAPI_ISteamUGC_StartItemUpdate(ugc, app_id, file_id)
        if handle in INVALID_HANDLE:
            raise RuntimeError("StartItemUpdate 失敗")
        tags = [t.encode() for t in item["tags"]]
        array = StringArray((ctypes.c_char_p * len(tags))(*tags), len(tags))
        steps = [
            d.SteamAPI_ISteamUGC_SetItemUpdateLanguage(ugc, handle, item["steam_language"].encode()),
            d.SteamAPI_ISteamUGC_SetItemTitle(ugc, handle, item["steam_title"].encode("utf-8")),
            d.SteamAPI_ISteamUGC_SetItemDescription(ugc, handle, (ROOT / item["steam_description"]).read_text(encoding="utf-8").encode("utf-8")),
            d.SteamAPI_ISteamUGC_SetItemContent(ugc, handle, str(content).encode("utf-8")),
            d.SteamAPI_ISteamUGC_SetItemTags(ugc, handle, ctypes.byref(array)),
        ]
        if item.get("preview"):
            steps.append(d.SteamAPI_ISteamUGC_SetItemPreview(ugc, handle, str(ROOT / item["preview"]).encode("utf-8")))
        if private:
            steps.append(d.SteamAPI_ISteamUGC_SetItemVisibility(ugc, handle, VISIBILITY_PRIVATE))
        if not all(steps):
            raise RuntimeError(f"Steam 拒絕了部分欄位：{steps}")
        # SubmitItemUpdateResult_t（k_iSteamUGCCallbacks + 4）：EResult@0、需要接受協議@4、PublishedFileId@8
        raw = self.wait(d.SteamAPI_ISteamUGC_SubmitItemUpdate(ugc, handle, note.encode("utf-8")), 3404, 16)
        if int.from_bytes(raw[:4], "little") != 1:
            raise RuntimeError(f"SubmitItemUpdate EResult={int.from_bytes(raw[:4], 'little')}")
        if raw[4]:
            raise RuntimeError("需要先到 Steam 網頁接受工作坊法律協議")


def preflight(items: list[dict], create: bool, note: str | None) -> list[str]:
    problems = []
    if subprocess.run(["uv", "run", "scripts/verify_mod.py"], cwd=ROOT, capture_output=True).returncode != 0:
        problems.append("verify_mod.py 沒有全過")
    for item in items:
        if not (ROOT / "dist" / item["folder"] / "info.txt").is_file():
            problems.append(f"{item['lang']}：dist/{item['folder']} 沒建置，先跑 scripts/build.py")
        if not (ROOT / item["steam_description"]).is_file():
            problems.append(f"{item['lang']}：缺說明檔 {item['steam_description']}")
        if create and item["published_file_id"]:
            problems.append(f"{item['lang']}：已有 published_file_id，不能再 --create")
        if not create and not item["published_file_id"]:
            problems.append(f"{item['lang']}：還沒有 published_file_id，首次請用 --create")
        if not create and not note:
            problems.append("更新要附 --note 更新說明")
        if not item.get("preview"):
            print(f"注意 {item['lang']}：workshop.json 沒有設 preview，工作坊頁面不會有封面圖")
    return problems


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--item", required=True, choices=["zh-Hant", "zh-Hans", "all"])
    ap.add_argument("--create", action="store_true", help="首次建立工作坊項目（私人可見）")
    ap.add_argument("--note", help="更新說明（Change Notes）")
    ap.add_argument("--yes", action="store_true", help="真的上傳；沒加只做檢查")
    args = ap.parse_args()
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    items = [i for i in config["items"] if args.item in ("all", i["lang"])]

    problems = preflight(items, args.create, args.note)
    for item in items:
        target = "新建（私人）" if args.create else f"更新 {item['published_file_id']}"
        print(f"{item['lang']}：{target} ← dist/{item['folder']}  標題「{item['steam_title']}」  語言槽 {item['steam_language']}")
    if problems:
        print("\n".join("FAIL " + p for p in problems))
        return 1
    if not args.yes:
        print("檢查通過；加 --yes 才會上傳。")
        return 0

    steam = Steam(config["app_id"])
    try:
        for item in items:
            if args.create:
                item["published_file_id"] = steam.create(config["app_id"])
                CONFIG.write_text(json.dumps(config, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
                print(f"{item['lang']}：已建立 {item['published_file_id']}，寫回 workshop.json")
            steam.update(config["app_id"], int(item["published_file_id"]), item, ROOT / "dist" / item["folder"],
                         args.note or "初次上傳", private=args.create)
            print(f"{item['lang']}：上傳完成 https://steamcommunity.com/sharedfiles/filedetails/?id={item['published_file_id']}")
    finally:
        steam.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
