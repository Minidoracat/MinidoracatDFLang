# /// script
# requires-python = ">=3.11"
# ///
"""把 dist/<folder>/ 這一包（繁、簡兩種語言）上傳到 Steam 工作坊（Steamworks ISteamUGC，ctypes 載入遊戲的 steam_api64.dll）。

預設只檢查並列出會做的事；加 --yes 才真的連 Steam 上傳。上傳是對外發布，執行前要有使用者明確同意。

用法（repo 根目錄，Steam 用戶端要開著並登入擁有 Dwarf Fortress 的帳號）：
  uv run scripts/publish_workshop.py                            # 檢查
  uv run scripts/publish_workshop.py --yes                      # 上傳
  uv run scripts/publish_workshop.py --description-only --yes   # 只換各語言的標題與說明（不傳內容、封面，不留更新說明）
  uv run scripts/publish_workshop.py --check-screenshots        # 只查項目上現有幾張截圖（不改任何東西）
  uv run scripts/publish_workshop.py --retire --yes             # 停用的簡中項目換成「已合併」的標題與說明

每次上傳的內容（workshop.json 的 workshop）：dist/<folder>/ 整個資料夾、標籤（tags）、封面（preview，512 動態 GIF）、
更新說明（每種語言的 steam_changelog 檔：本版這一則，第一行要有版本號，依 languages 順序串起來、中間隔一條橫線；CHANGELOG.md 也要先記這一版）。
標題（steam_title）與說明（steam_description 檔）依 Steam 語言分開放：languages 的第一個放在預設（english）欄位，沒有對應在地化欄位的
用戶端看這份；每種語言再各自提交一次，放進它的 steam_language（tchinese、schinese）在地化欄位。更新說明與截圖不分語言，大家看同一份。
加 --screenshots：先刪掉項目上現有的截圖（查得到幾張就刪幾張），再把 screenshots 資料夾裡的截圖依檔名排序一張張上傳（放在 temp/，不進 git）。
--retire：停用的項目（retired）只換預設欄位的標題與說明，請玩家改訂閱合併後的項目；它的內容不再上傳。
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
        d.SteamAPI_ISteamUGC_StartItemUpdate.argtypes = [ctypes.c_void_p, ctypes.c_uint32, ctypes.c_uint64]
        d.SteamAPI_ISteamUGC_StartItemUpdate.restype = ctypes.c_uint64
        for name in ("SetItemTitle", "SetItemDescription", "SetItemContent", "SetItemPreview"):
            fn = getattr(d, "SteamAPI_ISteamUGC_" + name)
            fn.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_char_p]
            fn.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_SetItemUpdateLanguage.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_char_p]
        d.SteamAPI_ISteamUGC_SetItemUpdateLanguage.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_SetItemTags.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.POINTER(StringArray)]
        d.SteamAPI_ISteamUGC_SetItemTags.restype = ctypes.c_bool
        d.SteamAPI_ISteamUGC_AddItemPreviewFile.argtypes = [ctypes.c_void_p, ctypes.c_uint64, ctypes.c_char_p, ctypes.c_int]
        d.SteamAPI_ISteamUGC_AddItemPreviewFile.restype = ctypes.c_bool
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

    def update(self, app_id: int, file_id: int, workshop: dict, default: dict, content: Path, note: str) -> None:
        """內容、標籤、封面與預設（english）欄位的標題、說明，連同更新說明一次提交"""
        d, ugc = self.dll, self.ugc
        handle = self.start(app_id, file_id)
        tags = [t.encode() for t in workshop["tags"]]
        array = StringArray((ctypes.c_char_p * len(tags))(*tags), len(tags))
        steps = [
            d.SteamAPI_ISteamUGC_SetItemTitle(ugc, handle, default["steam_title"].encode("utf-8")),
            d.SteamAPI_ISteamUGC_SetItemDescription(ugc, handle, read(default["steam_description"]).encode("utf-8")),
            d.SteamAPI_ISteamUGC_SetItemContent(ugc, handle, str(content).encode("utf-8")),
            d.SteamAPI_ISteamUGC_SetItemTags(ugc, handle, ctypes.byref(array)),
            d.SteamAPI_ISteamUGC_SetItemPreview(ugc, handle, str(ROOT / workshop["preview"]).encode("utf-8")),
        ]
        if not all(steps):
            raise RuntimeError(f"Steam 拒絕了部分欄位：{steps}")
        self.submit(handle, note)

    def set_text(self, app_id: int, file_id: int, title: str, description: str, steam_language: str | None = None) -> None:
        """只換標題與說明，更新說明留空；steam_language（tchinese、schinese…）是那個語言的在地化欄位，None 是預設（english）欄位"""
        d, ugc = self.dll, self.ugc
        handle = self.start(app_id, file_id)
        steps = [] if steam_language is None else [d.SteamAPI_ISteamUGC_SetItemUpdateLanguage(ugc, handle, steam_language.encode())]
        steps += [d.SteamAPI_ISteamUGC_SetItemTitle(ugc, handle, title.encode("utf-8")),
                  d.SteamAPI_ISteamUGC_SetItemDescription(ugc, handle, description.encode("utf-8"))]
        if not all(steps):
            raise RuntimeError(f"Steam 拒絕了標題或說明（{steam_language or 'english'}）：{steps}")
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


def read(path: str) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def screenshots(workshop: dict) -> list[Path]:
    folder = ROOT / workshop["screenshots"]
    return sorted(p for p in folder.iterdir() if p.suffix.lower() in IMAGE_SUFFIXES) if folder.is_dir() else []


def localize(steam: Steam, app_id: int, file_id: int, languages: list[dict]) -> None:
    """每種語言的標題與說明各自提交一次，放進它的 steam_language 在地化欄位"""
    for language in languages:
        steam.set_text(app_id, file_id, language["steam_title"], read(language["steam_description"]), language["steam_language"])
        print(f"{language['lang']}（{language['steam_language']}）：標題、說明已更新")


def preflight(config: dict, with_shots: bool) -> list[str]:
    problems = []
    if subprocess.run(["uv", "run", "scripts/verify_mod.py"], cwd=ROOT, capture_output=True).returncode != 0:
        problems.append("verify_mod.py 沒有全過")
    version = config["version"]["displayed"]
    # 發版前先在 CHANGELOG.md 記這一版，工作坊的更新說明再由它整理成玩家看得懂的重點
    changelog = ROOT / "CHANGELOG.md"
    if f"## [{version}]" not in (changelog.read_text(encoding="utf-8") if changelog.is_file() else ""):
        problems.append(f"CHANGELOG.md 沒有 {version} 這一版")
    if not (ROOT / "dist" / config["folder"] / "info.txt").is_file():
        problems.append(f"dist/{config['folder']} 沒建置，先跑 scripts/build.py")
    for language in config["languages"]:
        lang = language["lang"]
        if not (ROOT / language["steam_description"]).is_file():
            problems.append(f"{lang}：缺說明檔 {language['steam_description']}")
        notes = ROOT / language["steam_changelog"]
        if version not in (notes.read_text(encoding="utf-8").split("\n", 1)[0] if notes.is_file() else ""):
            problems.append(f"{lang}：更新說明 {language['steam_changelog']} 的第一行沒有 {version}，先寫這一版的")
    workshop = config["workshop"]
    cover = ROOT / workshop["preview"]
    shots = screenshots(workshop) if with_shots else []
    if not cover.is_file():
        problems.append(f"缺封面 {workshop['preview']}")
    elif cover.stat().st_size > PREVIEW_LIMIT:
        problems.append(f"封面 {cover.stat().st_size:,} bytes，超過 {PREVIEW_LIMIT:,}")
    if with_shots and not shots:
        problems.append(f"{workshop['screenshots']} 沒有截圖")
    problems += [f"截圖 {p.name} {p.stat().st_size:,} bytes，超過 {SHOT_LIMIT:,}" for p in shots if p.stat().st_size > SHOT_LIMIT]
    return problems


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--screenshots", action="store_true", help="也換掉截圖：刪掉項目上現有的，再傳 screenshots 資料夾的")
    ap.add_argument("--check-screenshots", action="store_true", help="只查項目上現有幾張截圖，不改任何東西")
    ap.add_argument("--description-only", action="store_true", help="只換各語言的標題與說明（steam_title、steam_description 檔），不傳內容與封面")
    ap.add_argument("--retire", action="store_true", help="停用的項目（retired）換成「已合併」的標題與說明")
    ap.add_argument("--yes", action="store_true", help="真的上傳；沒加只做檢查")
    args = ap.parse_args()
    config = json.loads(CONFIG.read_text(encoding="utf-8"))
    app_id, workshop, languages = config["app_id"], config["workshop"], config["languages"]
    file_id = int(workshop["published_file_id"])
    default = languages[0]  # 預設（english）欄位放第一個語言的標題與說明

    if args.check_screenshots:
        steam = Steam(app_id)
        try:
            print(f"{file_id} 現有截圖 {steam.count_screenshots(file_id)} 張")
        finally:
            steam.close()
        return 0

    if args.retire:
        retired = config["retired"]
        print(f"停用項目 {retired['published_file_id']}：標題「{retired['steam_title']}」  說明 ← {retired['steam_description']}")
        if not (ROOT / retired["steam_description"]).is_file():
            print(f"FAIL 缺說明檔 {retired['steam_description']}")
            return 1
        if not args.yes:
            print("加 --yes 才會上傳。")
            return 0
        steam = Steam(app_id)
        try:
            steam.set_text(app_id, int(retired["published_file_id"]), retired["steam_title"], read(retired["steam_description"]))
        finally:
            steam.close()
        print(f"停用項目已更新 https://steamcommunity.com/sharedfiles/filedetails/?id={retired['published_file_id']}")
        return 0

    for language in languages:
        slots = ("english、" if language is default else "") + language["steam_language"]
        print(f"{language['lang']}（{slots}）：標題「{language['steam_title']}」  說明 ← {language['steam_description']}"
              + ("" if args.description_only else f"  更新說明 ← {language['steam_changelog']}"))

    if args.description_only:
        if not args.yes:
            print("加 --yes 才會上傳。")
            return 0
        steam = Steam(app_id)
        try:
            steam.set_text(app_id, file_id, default["steam_title"], read(default["steam_description"]))
            print("預設（english）欄位：標題、說明已更新")
            localize(steam, app_id, file_id, languages)
        finally:
            steam.close()
        return 0

    problems = preflight(config, args.screenshots)
    print(f"更新 {file_id} ← dist/{config['folder']}  封面 {workshop['preview']}"
          + (f"  截圖 {len(screenshots(workshop))} 張（先刪掉現有的）" if args.screenshots else ""))
    if problems:
        print("\n".join("FAIL " + p for p in problems))
        return 1
    if not args.yes:
        print("檢查通過；加 --yes 才會上傳。")
        return 0

    steam = Steam(app_id)
    try:
        notes = "\n\n[hr][/hr]\n\n".join(read(language["steam_changelog"]).strip() for language in languages)
        steam.update(app_id, file_id, workshop, default, ROOT / "dist" / config["folder"], notes)
        print("內容、標籤、封面、更新說明與預設欄位的標題、說明已上傳")
        localize(steam, app_id, file_id, languages)
        if args.screenshots:
            old = steam.count_screenshots(file_id)
            if old:
                steam.remove_screenshots(app_id, file_id, old)
            print(f"刪掉舊截圖 {old} 張")
            shots = screenshots(workshop)
            for k, shot in enumerate(shots, 1):
                steam.add_screenshot(app_id, file_id, shot)  # 中途失敗時前面幾張已生效，剩下的到工作坊頁面補
                print(f"截圖 {k}/{len(shots)} {shot.name}")
        print(f"上傳完成 https://steamcommunity.com/sharedfiles/filedetails/?id={file_id}")
    finally:
        steam.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
