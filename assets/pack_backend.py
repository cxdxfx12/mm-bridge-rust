"""把 Python 生产转发引擎打成自解压归档，并追加到 Rust Setup.exe 尾部。

为什么是「PE overlay」而不是 include_bytes! / Win32 资源
    runtime 有 7000+ 文件、约 370 MB。若用 include_bytes! 编进 Rust，
    rustc 要生成一个 200 MB 的字节数组常量，link.exe 会吃掉几个 GB 内存且
    每次增量构建都要重来；rc.exe 处理超大 RCDATA 也不可靠。
    Windows 加载器会忽略最后一个节之后的附加数据，所以直接把归档 append
    在 exe 文件末尾是 NSIS / 7z SFX 用了二十年的做法：编译期零负担，
    Setup.exe 仍是**单个文件**，运行时从自身尾部读出归档再解压。

归档格式（全部小端）
    BUNDLE_MAGIC[8] | u32 文件数 | u64 解压后总字节 | u64 名称指纹
    每条：u16 名长 | 名(UTF-8, '/' 分隔) | u8 方式(0=直存,1=raw deflate)
          | u64 原始长度 | u64 存储长度 | 数据

    raw deflate = zlib.compressobj(wbits=-15)，Rust 侧用 flate2::read::DeflateDecoder
    解压。压缩后反而变大的文件（已压缩的 .zip/.onnx）直接存原始字节。

exe 尾部 64 字节尾注（footer）
    MAGIC[16] = b"MIAOMIAO-BUNDLE1"
    u64 归档长度 | u64 CRC32（低 32 位有效） | u32 格式版本 | 28 字节保留

隐私与裁剪
    * 绝不打包 data/（用户的生产数据库、日志、备份）；
    * 绝不打包 app/_env.json（含本机绝对路径的解释器缓存）；
    * config.yaml 只做模板化处理：wxauto_path 改成相对路径、所有凭证清空；
    * 顶层 uninstall.exe / 正式版 GUI exe / 各种 .bat 都不带（Rust 版有自己的 UI）。

用法
    python pack_backend.py                 # 生成 assets/backend.mmb1
    python pack_backend.py --append        # 把归档追加到 dist 的 Setup.exe
    python pack_backend.py --all          # 两步都做
"""
from __future__ import annotations

import argparse
import hashlib
import os
import re
import struct
import sys
import time
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
RUST = HERE.parent
BUNDLE_PATH = HERE / "backend.mmb1"
DIST = RUST / "dist"
SETUP_SRC = RUST / "target" / "release" / "bridge-setup.exe"
SETUP_NAMES = (
    "喵喵钉钉-微信消息转发桥-Rust独立版-Setup.exe",
    "喵喵钉钉-微信消息转发桥-Rust版-Setup.exe",
)

BUNDLE_MAGIC = b"MMBUND01"
FOOTER_MAGIC = b"MIAOMIAO-BUNDLE1"      # 恰好 16 字节
FOOTER_SIZE = 64
FOOTER_VERSION = 1

DEFAULT_SRC = Path(r"C:\Program Files\喵喵钉钉-微信消息转发桥")

# 只带这些顶层条目；其余（data / tests / docs / 卸载器 / 正式版 exe / bat）一律不带。
INCLUDE_TOP = {
    "app", "bridge", "runtime", "wxauto-main", "android", "tools",
    "run.py", "config.yaml", "config.example.yaml", "requirements.txt",
    "README.md", "probe_dingtalk.py",
}

# 明确排除的名字/后缀（在任意层级生效）
EXCLUDE_NAMES = {"__pycache__", ".git", ".pytest_cache", "_env.json", "Thumbs.db"}
EXCLUDE_SUFFIX = {".pyc", ".pyo", ".lnk", ".tmp", ".log", ".bak"}
EXCLUDE_FILES = {
    "uninstall.exe",
    "喵喵钉钉-微信消息转发桥.exe",
    "bridge-gui.exe",
    "bridge-setup.exe",
}


def fnv1a(data: bytes) -> int:
    h = 0xCBF29CE484222325
    for b in data:
        h ^= b
        h = (h * 0x10000000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def crc32(data: bytes) -> int:
    return zlib.crc32(data) & 0xFFFFFFFF


def sanitize_config(text: str) -> str:
    """模板化配置：相对化 wxauto_path，清空一切凭证，避免把本机设置带给别人。"""
    # wechat.wxauto_path → 相对 worker 的 cwd（backend 根）
    text = re.sub(
        r'(?m)^(\s*wxauto_path:\s*)"[^"]*"',
        r'\1"wxauto-main"',
        text,
    )
    # 钉钉 / 机器人 / 网关凭证
    for key in ("client_id", "client_secret", "robot_code", "gateway_token", "secret"):
        text = re.sub(rf'(?m)^(\s*{key}:\s*)"[^"]*"', r'\1""', text)
    text = re.sub(r'access_token=[^"\s]+', 'access_token=改成你自己的', text)
    return text


def should_skip(rel: Path) -> bool:
    parts = rel.parts
    if any(p in EXCLUDE_NAMES for p in parts):
        return True
    if rel.suffix.lower() in EXCLUDE_SUFFIX:
        return True
    if rel.name in EXCLUDE_FILES:
        return True
    # 顶层 install 脚本 / 各种 bat 不需要（Rust 版有自己的界面和卸载入口）
    if len(parts) == 1 and rel.suffix.lower() in {".bat", ".cmd"}:
        return True
    return False


def collect(src: Path):
    """产出 [(归档名, 源文件路径 或 None+内存字节)]，config.yaml 单独走内存模板。"""
    items: list[tuple[str, Path | None, bytes | None]] = []
    seen: set[str] = set()

    for top in sorted(p for p in src.iterdir() if p.name in INCLUDE_TOP):
        if top.name == "config.yaml":
            continue  # 单独处理
        files = [top] if top.is_file() else [p for p in top.rglob("*") if p.is_file()]
        for f in files:
            rel = f.relative_to(src)
            if should_skip(rel):
                continue
            name = "backend/" + rel.as_posix()
            if name in seen:
                continue
            seen.add(name)
            items.append((name, f, None))

    cfg = src / "config.yaml"
    if not cfg.is_file():
        raise SystemExit(f"源目录缺少 config.yaml：{cfg}")
    raw = cfg.read_text(encoding="utf-8")
    items.append(("config.yaml", None, sanitize_config(raw).encode("utf-8")))
    items.sort(key=lambda x: x[0])
    return items


def deflate_raw(data: bytes) -> bytes:
    c = zlib.compressobj(level=6, wbits=-15)
    return c.compress(data) + c.flush()


def build_bundle(src: Path) -> None:
    t0 = time.time()
    items = collect(src)
    name_fingerprint = fnv1a("\n".join(n for n, _, _ in items).encode("utf-8"))

    out = bytearray()
    out += BUNDLE_MAGIC
    out += struct.pack("<IQQ", len(items), 0, name_fingerprint)  # 总字节稍后回填
    total_raw = 0
    total_stored = 0
    stored_files = 0

    for i, (name, path, inline) in enumerate(items):
        data = inline if inline is not None else path.read_bytes()
        raw_len = len(data)
        packed = deflate_raw(data)
        method = 1
        if len(packed) >= raw_len:
            method = 0
            packed = data
            stored_files += 1
        nb = name.encode("utf-8")
        if len(nb) > 65535:
            raise SystemExit(f"归档名过长：{name}")
        out += struct.pack("<H", len(nb)) + nb
        out += struct.pack("<BQQ", method, raw_len, len(packed))
        out += packed
        total_raw += raw_len
        total_stored += len(packed)
        if (i + 1) % 1000 == 0:
            print(f"  已压缩 {i + 1}/{len(items)} 个文件…", flush=True)

    struct.pack_into("<Q", out, 12, total_raw)
    BUNDLE_PATH.write_bytes(bytes(out))
    digest = hashlib.sha256(bytes(out)).hexdigest()
    print(f"归档：{BUNDLE_PATH}")
    print(f"  文件数 {len(items)}（{stored_files} 个已压缩格式直接存）")
    print(f"  原始 {total_raw / 1048576:.1f} MB → 归档 {total_stored / 1048576:.1f} MB "
          f"（压缩率 {100 * total_stored / max(1, total_raw):.1f}%）")
    print(f"  sha256 {digest}")
    print(f"  用时 {time.time() - t0:.1f}s")


def append_bundle() -> None:
    if not BUNDLE_PATH.is_file():
        raise SystemExit(f"归档不存在，请先运行不带参数的打包：{BUNDLE_PATH}")
    if not SETUP_SRC.is_file():
        raise SystemExit(f"找不到 release 安装器：{SETUP_SRC}")
    DIST.mkdir(parents=True, exist_ok=True)

    stub = SETUP_SRC.read_bytes()
    # 可重复执行：若上一次已经追加过，先把旧 overlay 切掉。
    if len(stub) >= FOOTER_SIZE and stub[-FOOTER_SIZE:-FOOTER_SIZE + 16] == FOOTER_MAGIC:
        old_size = struct.unpack_from("<Q", stub, len(stub) - FOOTER_SIZE + 16)[0]
        stub = stub[:len(stub) - FOOTER_SIZE - old_size]

    bundle = BUNDLE_PATH.read_bytes()
    h = crc32(bundle)
    footer = bytearray(FOOTER_SIZE)
    footer[0:16] = FOOTER_MAGIC
    struct.pack_into("<QQI", footer, 16, len(bundle), h, FOOTER_VERSION)
    payload = stub + bundle + bytes(footer)

    for name in SETUP_NAMES:
        p = DIST / name
        p.write_bytes(payload)
        print(f"已生成 {p}  ({len(payload) / 1048576:.1f} MB)")
    print(f"  stub {len(stub) / 1048576:.2f} MB + 引擎归档 {len(bundle) / 1048576:.1f} MB")
    print(f"  CRC32 {h:08x}")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", type=Path, default=None, help="Python 正式版安装目录")
    ap.add_argument("--append", action="store_true", help="只做 exe 追加")
    ap.add_argument("--all", action="store_true", help="打包 + 追加")
    a = ap.parse_args()

    src = a.src or Path(os.getenv("MIAOMIAO_BACKEND_SRC", str(DEFAULT_SRC)))
    if not src.is_dir():
        raise SystemExit(f"源目录不存在：{src}")
    print(f"后端源目录：{src}")

    if not a.append:
        build_bundle(src)
    if a.append or a.all:
        append_bundle()


if __name__ == "__main__":
    main()
