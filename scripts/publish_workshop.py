# /// script
# requires-python = ">=3.11"
# ///
"""把 dist/ 的繁、簡兩個工作坊包各自上傳到 Steam 工作坊（Steamworks ISteamUGC，ctypes 載入遊戲的 steam_api64.dll）。

預設只檢查並列出會做的事；加 --yes 才真的連 Steam 上傳。上傳是對外發布，執行前要有使用者明確同意。

用法（repo 根目錄，Steam 用戶端要開著並登入擁有 Dwarf Fortress 的帳號）：
  uv run scripts/publish_workshop.py --item all                   # 檢查
  uv run scripts/publish_workshop.py --item all --yes             # 兩包各自上傳
  uv run scripts/publish_workshop.py --item zh-Hans --create --yes             # 首次建立項目（設為私人），ID 寫回 workshop.json
  uv run scripts/publish_workshop.py --item all --description-only --yes     # 只換說明（不傳內容、封面，不留更新說明）

每次上傳的內容：dist/<folder>/ 整個資料夾、標題（steam_title）、說明（steam_description 檔）、標籤（tags）、
封面（preview，512 動態 GIF）、更新說明（steam_changelog 檔：本版這一則，第一行要有版本號；CHANGELOG.md 也要先記這一版）。
標題與說明放在預設（english）欄位，任何語言的 Steam 用戶端都看得到同一份中文。
首次建立時另外上傳 screenshots 資料夾裡的截圖（依檔名排序，一張一次提交；放在 temp/，不進 git）。
更新時加 --screenshots：先刪掉項目上現有的截圖（查得到幾張就刪幾張），再照樣一張張上傳資料夾裡的截圖；--check-screenshots 只查目前有幾張（不改任何東西）。
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
PREVIEW_TYPE_IMAGE = 0  # k_EItemPreviewType_Image
# 大小上限照家族 PZ MOD 的實測（pz-mod-template 的 publish_workshop.py）：封面 1,024,000 bytes；
# 截圖（AddItemPreviewFile）274 KB 成功、314 KB 回 EResult 25，網頁上傳的 2 MB 上限不適用
PREVIEW_LIMIT = 1_024_000
SHOT_LIMIT = 280_000
IMAGE_SUFFIXES = (".jpg", ".jpeg", ".png")


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
        for name in ("SetItemTitle", "SetItemDescription", "SetItemContent", "SetItemPreview"):
            fn = getattr(d, "SteamAPI_ISteamUGC_" + name)
            fn.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_char_p]
            fn.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_SetItemTags.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.POINTER(StringArray)]
        d.SteamAPI_ISteamUGC_SetItemTags.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_AddItemPreviewFile.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_char_p, ctypes.c_int]
        d.SteamAPI_ISteamUGC_AddItemPreviewFile.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_SetItemVisibility.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_int]
        d.SteamAPI_ISteamUGC_SetItemVisibility.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_SubmitItemUpdate.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_char_p]
        d.SteamAPI_ISteamUGC_SubmitItemUpdate.restype = ctypes.c_uint64
        d.SteamAPI_ISteamUtils_IsAPICallCompleted.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.POINTER(ctypes.c_bool)]
        d.SteamAPI_ISteamUGC_CreateQueryUGCDetailsRequest.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_uint64), ctypes.c_uint32]
        d.SteamAPI_ISteamUGC_CreateQueryUGCDetailsRequest.restype = ctypes.c_uint64
        d.SteamAPI_ISteamUGC_SetReturnAdditionalPreviews.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_bool]
        d.SteamAPI_ISteamUGC_SetReturnAdditionalPreviews.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_SendQueryUGCRequest.argtypes = [ctypes.c_void_p, ctypes.c_uint64]
        d.SteamAPI_ISteamUGC_SendQueryUGCRequest.restype = ctypes.c_uint64
        d.SteamAPI_ISteamUGC_GetQueryUGCNumAdditionalPreviews.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_uint32]
        d.SteamAPI_ISteamUGC_GetQueryUGCNumAdditionalPreviews.restype = ctypes.c_uint32
        d.SteamAPI_ISteamUGC_ReleaseQueryUGCRequest.argtypes = [ctypes.c_void_p, ctypes.c_uint64]
        d.SteamAPI_ISteamUGC_ReleaseQueryUGCRequest.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_RemoveItemPreview.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_uint32]
        d.SteamAPI_ISteamUGC_RemoveItemPreview.restype = ctypes.c_bool
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

    def start(self, app_id: int, file_id: int) -> int:
        handle = self.dll.SteamAPI_ISteamUGC_StartItemUpdate(self.ugc, app_id, file_id)
        if handle in INVALID_HANDLE:
            raise RuntimeError("StartItemUpdate 失敗")
        return handle

    def submit(self, handle: int, note: str) -> None:
        # SubmitItemUpdateResult_t（k_iSteamUGCCallbacks + 4）：EResult@0、需要接受協議@4、PublishedFileId@8
        raw = self.wait(self.dll.SteamAPI_ISteamUGC_SubmitItemUpdate(self.ugc, handle, note.encode("utf-8")), 3404, 16)
        if int.from_bytes(raw[:4], "little") != 1:
            raise RuntimeError(f"SubmitItemUpdate EResult={int.from_bytes(raw[:4], 'little')}")
        if raw[4]:
            raise RuntimeError("需要先到 Steam 網頁接受工作坊法律協議")

    def update(self, app_id: int, file_id: int, item: dict, content: Path, note: str, private: bool) -> None:
        d, ugc = self.dll, self.ugc
        handle = self.start(app_id, file_id)
        tags = [t.encode() for t in item["tags"]]
        array = StringArray((ctypes.c_char_p * len(tags))(*tags), len(tags))
        steps = [
            d.SteamAPI_ISteamUGC_SetItemTitle(ugc, handle, item["steam_title"].encode("utf-8")),
            d.SteamAPI_ISteamUGC_SetItemDescription(ugc, handle, (ROOT / item["steam_description"]).read_text(encoding="utf-8").encode("utf-8")),
            d.SteamAPI_ISteamUGC_SetItemContent(ugc, handle, str(content).encode("utf-8")),
            d.SteamAPI_ISteamUGC_SetItemTags(ugc, handle, ctypes.byref(array)),
            d.SteamAPI_ISteamUGC_SetItemPreview(ugc, handle, str(ROOT / item["preview"]).encode("utf-8")),
        ]
        if private:
            steps.append(d.SteamAPI_ISteamUGC_SetItemVisibility(ugc, handle, VISIBILITY_PRIVATE))
        if not all(steps):
            raise RuntimeError(f"Steam 拒絕了部分欄位：{steps}")
        self.submit(handle, note)

    def set_description(self, app_id: int, file_id: int, item: dict) -> None:
        handle = self.start(app_id, file_id)
        text = (ROOT / item["steam_description"]).read_text(encoding="utf-8")
        if not self.dll.SteamAPI_ISteamUGC_SetItemDescription(self.ugc, handle, text.encode("utf-8")):
            raise RuntimeError("Steam 拒絕了說明")
        self.submit(handle, "")

    def add_screenshot(self, app_id: int, file_id: int, path: Path) -> None:
        # 一次提交只能帶一張（家族 PZ MOD 實測多張同批回 EResult 25），每張各自提交，更新說明留空
        handle = self.start(app_id, file_id)
        if not self.dll.SteamAPI_ISteamUGC_AddItemPreviewFile(self.ugc, handle, str(path).encode("utf-8"), PREVIEW_TYPE_IMAGE):
            raise RuntimeError(f"AddItemPreviewFile 拒絕：{path.name}")
        self.submit(handle, "")

    def count_screenshots(self, file_id: int) -> int:
        """項目上現有幾張額外預覽圖（截圖）。SteamUGCQueryCompleted_t（k_iSteamUGCCallbacks + 1）：handle@0、EResult@8、
        回傳筆數@12、總筆數@16、是否快取@20、下一頁游標 char[256]@21，對齊 8 共 280 bytes"""
        d, ugc = self.dll, self.ugc
        ids = (ctypes.c_uint64 * 1)(file_id)
        query = d.SteamAPI_ISteamUGC_CreateQueryUGCDetailsRequest(ugc, ids, 1)
        if query in INVALID_HANDLE:
            raise RuntimeError("CreateQueryUGCDetailsRequest 失敗")
        try:
            d.SteamAPI_ISteamUGC_SetReturnAdditionalPreviews(ugc, query, True)
            raw = self.wait(d.SteamAPI_ISteamUGC_SendQueryUGCRequest(ugc, query), 3401, 280)
            if int.from_bytes(raw[8:12], "little") != 1 or int.from_bytes(raw[12:16], "little") != 1:
                raise RuntimeError(f"查詢項目失敗：EResult={int.from_bytes(raw[8:12], 'little')}")
            return d.SteamAPI_ISteamUGC_GetQueryUGCNumAdditionalPreviews(ugc, query, 0)
        finally:
            d.SteamAPI_ISteamUGC_ReleaseQueryUGCRequest(ugc, query)

    def remove_screenshots(self, app_id: int, file_id: int, count: int) -> None:
        # 從最後一張往前刪，前面的編號不會因為刪掉後面的而移動
        handle = self.start(app_id, file_id)
        if not all(self.dll.SteamAPI_ISteamUGC_RemoveItemPreview(self.ugc, handle, i) for i in reversed(range(count))):
            raise RuntimeError("RemoveItemPreview 拒絕")
        self.submit(handle, "")


def screenshots(item: dict) -> list[Path]:
    folder = ROOT / item["screenshots"]
    return sorted(p for p in folder.iterdir() if p.suffix.lower() in IMAGE_SUFFIXES) if folder.is_dir() else []


def preflight(items: list[dict], create: bool, version: str, with_shots: bool) -> list[str]:
    problems = []
    if subprocess.run(["uv", "run", "scripts/verify_mod.py"], cwd=ROOT, capture_output=True).returncode != 0:
        problems.append("verify_mod.py 沒有全過")
    # 發版前先在 CHANGELOG.md 記這一版，工作坊的更新說明再由它整理成玩家看得懂的重點
    changelog = ROOT / "CHANGELOG.md"
    if not create and f"## [{version}]" not in (changelog.read_text(encoding="utf-8") if changelog.is_file() else ""):
        problems.append(f"CHANGELOG.md 沒有 {version} 這一版")
    for item in items:
        if not (ROOT / "dist" / item["folder"] / "info.txt").is_file():
            problems.append(f"{item['lang']}：dist/{item['folder']} 沒建置，先跑 scripts/build.py")
        if not (ROOT / item["steam_description"]).is_file():
            problems.append(f"{item['lang']}：缺說明檔 {item['steam_description']}")
        if create and item["published_file_id"]:
            problems.append(f"{item['lang']}：已有 published_file_id，不能再 --create")
        if not create and not item["published_file_id"]:
            problems.append(f"{item['lang']}：還沒有 published_file_id，首次請用 --create")
        notes = ROOT / item["steam_changelog"]
        if not create and version not in (notes.read_text(encoding="utf-8").split("\n", 1)[0] if notes.is_file() else ""):
            problems.append(f"{item['lang']}：更新說明 {item['steam_changelog']} 的第一行沒有 {version}，先寫這一版的")
        cover = ROOT / item["preview"]
        shots = screenshots(item) if with_shots else []
        if not cover.is_file():
            problems.append(f"{item['lang']}：缺封面 {item['preview']}")
        elif cover.stat().st_size > PREVIEW_LIMIT:
            problems.append(f"{item['lang']}：封面 {cover.stat().st_size:,} bytes，超過 {PREVIEW_LIMIT:,}")
        if with_shots and not shots:
            problems.append(f"{item['lang']}：{item['screenshots']} 沒有截圖")
        problems += [f"{item['lang']}：截圖 {p.name} {p.stat().st_size:,} bytes，超過 {SHOT_LIMIT:,}" for p in shots if p.stat().st_size > SHOT_LIMIT]
    return problems


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--item", required=True, choices=["zh-Hant", "zh-Hans", "all"])
    ap.add_argument("--create", action="store_true", help="首次建立工作坊項目（私人可見）")
    ap.add_argument("--screenshots", action="store_true", help="更新時也換掉截圖：刪掉項目上現有的，再傳 screenshots 資料夾的")
    ap.add_argument("--check-screenshots", action="store_true", help="只查項目上現有幾張截圖，不改任何東西")
    ap.add_argument("--description-only", action="store_true", help="只換說明（steam_description 檔），不傳內容與封面")
    ap.add_argument("--yes", action="store_true", help="真的上傳；沒加只做檢查")
    args = ap.parse_args()
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    items = [i for i in config["items"] if args.item in ("all", i["lang"])]

    if args.check_screenshots:
        steam = Steam(config["app_id"])
        try:
            for item in items:
                print(f"{item['lang']}：{item['published_file_id']} 現有截圖 {steam.count_screenshots(int(item['published_file_id']))} 張")
        finally:
            steam.close()
        return 0

    if args.description_only:
        for item in items:
            print(f"{item['lang']}：只換說明 {item['published_file_id']} ← {item['steam_description']}")
        if not args.yes:
            print("加 --yes 才會上傳。")
            return 0
        steam = Steam(config["app_id"])
        try:
            for item in items:
                steam.set_description(config["app_id"], int(item["published_file_id"]), item)
                print(f"{item['lang']}：說明已更新")
        finally:
            steam.close()
        return 0

    with_shots = args.create or args.screenshots
    problems = preflight(items, args.create, config["version"]["displayed"], with_shots)
    for item in items:
        target = "新建（私人）" if args.create else f"更新 {item['published_file_id']}"
        print(f"{item['lang']}：{target} ← dist/{item['folder']}  標題「{item['steam_title']}」  封面 {item['preview']}"
              + (f"  截圖 {len(screenshots(item))} 張" + ("（先刪掉現有的）" if args.screenshots and not args.create else "") if with_shots else ""))
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
            file_id = int(item["published_file_id"])
            notes = "初次上傳" if args.create else (ROOT / item["steam_changelog"]).read_text(encoding="utf-8")
            steam.update(config["app_id"], file_id, item, ROOT / "dist" / item["folder"], notes, private=args.create)
            print(f"{item['lang']}：內容、標題、說明、封面已上傳")
            if args.screenshots and not args.create:
                old = steam.count_screenshots(file_id)
                if old:
                    steam.remove_screenshots(config["app_id"], file_id, old)
                print(f"{item['lang']}：刪掉舊截圖 {old} 張")
            shots = screenshots(item) if with_shots else []
            for k, shot in enumerate(shots, 1):
                steam.add_screenshot(config["app_id"], file_id, shot)  # 中途失敗時前面幾張已生效，剩下的到工作坊頁面補
                print(f"{item['lang']}：截圖 {k}/{len(shots)} {shot.name}")
            print(f"{item['lang']}：上傳完成 https://steamcommunity.com/sharedfiles/filedetails/?id={file_id}")
    finally:
        steam.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
