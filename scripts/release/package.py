#!/usr/bin/env python3
"""Build and independently inspect dangoOS OTA/full-install release artifacts."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import tomllib
import zipfile
import zlib

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(HERE))
sys.path.insert(0, str(ROOT / "scripts/device"))
from bootstrap import download
import layout
from ota_image import validate_image

INPUTS = json.loads((HERE / "inputs.json").read_text())
OFFICIAL = INPUTS["official"]
FLASH_BYTES = 0x1000000
SYSTEM_AT = 0xD00000
SYSTEM_BYTES = 0x2EE000


def sha(data):
    return hashlib.sha256(data).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def json_bytes(value):
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def checked_application(data, version):
    info = validate_image(data)
    require(info["ab_layout"], "application lacks the A/B layout marker")
    require(info["version"] == version, "application descriptor and release version differ")
    return info


def official_components(data, pins=OFFICIAL):
    require(len(data) == pins["bytes"] and sha(data) == pins["sha256"],
            "official base image size/SHA-256 mismatch")
    rows = layout.parse_table(data[layout.TABLE_AT:layout.TABLE_AT + layout.TABLE_BYTES])
    layout.require_legacy(rows)
    parts = {}
    for name, component in pins["components"].items():
        start, size = component["offset"], component["bytes"]
        payload = data[start:start + size]
        require(len(payload) == size and sha(payload) == component["sha256"],
                f"official {name} checksum mismatch")
        parts[name] = (start, payload)
    boot = parts["bootloader"][1]
    require(boot[0] == 0xE9 and b"MOSAICO-BOOT-02!" in boot and b"mosaico_boot_v2" in boot,
            "unexpected Mosaico bootloader")
    return rows, parts


def partition_table():
    require(sha(layout.GENERATOR.read_bytes()) == layout.GENERATOR_SHA,
            "partition generator checksum changed")
    with tempfile.TemporaryDirectory(prefix="dango-partitions-") as temporary:
        destination = Path(temporary) / "partitions.bin"
        subprocess.run([sys.executable, str(layout.GENERATOR), "--flash-size", "16MB",
                        "--offset", "0x8000", "--primary-bootloader-offset", "0x2000",
                        str(HERE / "partitions.csv"), str(destination)], check=True,
                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        data = destination.read_bytes()
    require(len(data) <= layout.TABLE_BYTES, "partition table exceeds one sector")
    return data.ljust(layout.TABLE_BYTES, b"\xff")


def initial_otadata():
    data = bytearray(b"\xff" * layout.OTA_BYTES)
    struct.pack_into("<I", data, 0, 1)  # sequence 1 selects slot zero
    struct.pack_into("<I", data, 24, 2)  # ESP-IDF OTA_IMG_VALID
    struct.pack_into("<I", data, 28, zlib.crc32(struct.pack("<I", 1), 0xFFFFFFFF))
    return bytes(data)


def merge_parts(parts):
    result = bytearray(b"\xff" * FLASH_BYTES)
    end = 0
    for name, (start, payload) in sorted(parts.items(), key=lambda item: item[1][0]):
        require(payload and start >= end and start + len(payload) <= FLASH_BYTES,
                f"overlapping or out-of-range component: {name}")
        result[start:start + len(payload)] = payload
        end = start + len(payload)
    return bytes(result)


def assemble(official, application, system, table, version, pins=OFFICIAL):
    application_info = checked_application(application, version)
    legacy, parts = official_components(official, pins)
    rows = layout.parse_table(table)
    require(rows == layout.planned_rows(legacy), "full-install partition layout differs from A/B contract")
    require(len(system) == SYSTEM_BYTES, "system image has the wrong partition size")
    parts.update(partition_table=(layout.TABLE_AT, table),
                 otadata=(layout.OTA_AT, initial_otadata()),
                 application=(layout.APP_AT, application), system=(SYSTEM_AT, system))
    full = merge_parts(parts)
    components = {name: {"offset": start, "bytes": len(payload), "sha256": sha(payload)}
                  for name, (start, payload) in parts.items()}
    return full, components, rows, application_info


def checksums(files):
    return "".join(f"{sha(payload)}  {name}\n" for name, payload in sorted(files.items())).encode()


def write_zip(path, files):
    with zipfile.ZipFile(path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for name, payload in sorted(files.items()):
            info = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            archive.writestr(info, payload)


def install_notes(tag, stem):
    return f"""# dangoOS {tag} 安装与升级

适用硬件：ESP-Mosaico V1.2 / ESP32-S31，16 MiB Flash。

## OTA 升级

`{stem}-ota.bin` 是应用镜像，只适用于已经采用 MOSAICO_S31_AB_LAYOUT_V1 的 dangoOS。
它保留 BL、分区表、上一运行槽、system 应用和用户数据。它不能从 Flash 地址 0 刷入。
OTA ZIP 内含镜像和 USB 更新工具；解压后关闭其他串口监视器，执行：

```sh
python3 -m pip install -r requirements.txt
python3 tools/console.py ota {stem}-ota.bin --port PORT --verbose
python3 tools/console.py ota-status --port PORT
```

将 PORT 替换为运行时 USB CDC 端口，例如 `/dev/cu.usbmodemmosaico_s31_r11` 或 `COM3`。
升级完成以重启后的运行槽、版本 `{tag[1:]}`、`Valid` 和自检结果为准。

## 全量安装与恢复

`{stem}-full.bin` 是从 Flash 地址 **0x0** 开始的完整 **16 MiB** 镜像。
**它会重建分区，并清除设置、照片、存档及下载应用；刷入前备份需要保留的数据。**
镜像包括官方 Mosaico BL、Vibe Mode 和工厂 UI 资源、dangoOS A/B 分区、slot 0 应用及
从仓库生成的 system 应用。slot 1、用户 data、NVS 和崩溃记录从擦除状态开始。
首次启动由系统初始化 data；初始 OTA 元数据选择 slot 0。

全量 ZIP 解压后，按住 BOOT 点一下 RESET 进入 ROM 下载模式，再执行：

```sh
python3 -m pip install -r requirements.txt
python3 -m esptool --chip esp32s31 --port PORT --baud 460800 --before no-reset --after hard-reset write-flash 0x0 {stem}-full.bin
```

这里的 PORT 是 ROM 下载端口，可能与运行时端口不同。完整镜像已覆盖全部 Flash，
保留镜像中的 Flash 参数即可。进入系统后检查版本、显示/触摸、应用列表和 OTA 状态。

## 产物与验证范围

先核对发布页的 `SHA256SUMS`。ZIP 内部也包含各文件的 SHA-256 清单。
`manifest.json` 记录源码提交、镜像类型、分区偏移、官方组件来源和哈希。
官方 BL 来源：Mosaico Claw v0.6.0；BL 构建时间为 Oct 21 2026 19:45:22。
该 BL 与此前保留版本的差异限于构建元数据及校验字段。

发布流水线验证主机测试、固件构建、镜像结构、分区、system 文件树和校验和。
本次发布没有刷写设备；新 BL 的整机首次安装和恢复入口未做本轮硬件验收。
保留官方 Vibe Mode 资源不代表已验证它与本项目应用和数据布局的全部交互。
""".encode()


def build(args):
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    tag = args.tag or f"v{version}"
    require(re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", tag) and tag[1:] == version,
            "tag must match workspace.package.version")
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT))
    require(not dirty or args.allow_dirty, "release build requires a clean checkout; use --allow-dirty only for local inspection")
    require(not args.out.exists(), "release output directory already exists")
    if args.upstream_image:
        official = args.upstream_image.read_bytes()
    else:
        cache = ROOT / "target/release-inputs"
        cache.mkdir(parents=True, exist_ok=True)
        path = cache / f"{OFFICIAL['sha256']}.bin"
        download(OFFICIAL["url"], path, OFFICIAL["sha256"])
        official = path.read_bytes()
    official_components(official)
    table = partition_table()
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="release-build-", dir=ROOT / "target") as temporary:
        temporary = Path(temporary)
        ota_path = temporary / "application.bin"
        subprocess.run(["espflash", "save-image", "--chip", "esp32s31", "--flash-size", "16mb",
                        "--flash-mode", "dio", "--flash-freq", "40mhz", str(args.elf), str(ota_path)], check=True)
        application = ota_path.read_bytes()
        checked_application(application, version)
        system_dir = temporary / "system"
        subprocess.run(["cargo", "run", "-p", "app-pack", "--locked", "--offline", "--",
                        "system", "apps", str(system_dir), "--size", str(SYSTEM_BYTES)], cwd=ROOT, check=True)
        system = (system_dir / "system.bin").read_bytes()
        report = json.loads((system_dir / "file-tree-report.json").read_text())
        require(report["status"] == "verified-system-image" and report["output_sha256"] == sha(system),
                "system image verification receipt mismatch")
        full, components, rows, info = assemble(official, application, system, table, version)
        stem = f"dangoOS-{tag}-esp32s31"
        manifest = {
            "schema": 1, "project": "dangoOS", "version": version, "tag": tag,
            "source_commit": commit, "source_dirty": dirty, "board": "ESP-Mosaico V1.2",
            "chip": "esp32s31", "flash_bytes": FLASH_BYTES, "layout": "MOSAICO_S31_AB_LAYOUT_V1",
            "upstream": OFFICIAL, "partitions": rows, "components": components,
            "application": info, "hardware_validation": "not-run for this release",
            "images": {
                "ota": {"file": f"{stem}-ota.bin", "bytes": len(application), "sha256": sha(application),
                        "delivery": "runtime USB A/B OTA", "preserves_user_data": True},
                "full": {"file": f"{stem}-full.bin", "bytes": len(full), "sha256": sha(full),
                         "flash_offset": 0, "preserves_user_data": False},
            },
            "tools": {"rust": subprocess.check_output(["rustc", "--version"], text=True).strip(),
                      "espflash": subprocess.check_output(["espflash", "--version"], text=True).strip()},
        }
        args.out.mkdir()
        files = {f"{stem}-ota.bin": application, f"{stem}-full.bin": full,
                 "manifest.json": json_bytes(manifest), "INSTALL.md": install_notes(tag, stem)}
        requirements = b"pyserial==3.5\nesptool==5.6.0\n"
        common = {"manifest.json": files["manifest.json"], "INSTALL.md": files["INSTALL.md"],
                  "requirements.txt": requirements}
        ota_files = {**common, f"{stem}-ota.bin": application,
                     "tools/console.py": (ROOT / "scripts/device/console.py").read_bytes(),
                     "tools/ota_image.py": (ROOT / "scripts/device/ota_image.py").read_bytes()}
        full_files = {**common, f"{stem}-full.bin": full, "partitions.csv": (HERE / "partitions.csv").read_bytes(),
                      "system-file-tree.json": json_bytes(report),
                      "UPSTREAM.md": (HERE / "UPSTREAM.md").read_bytes()}
        for path in sorted((system_dir / "application-docs").rglob("*")):
            if path.is_file():
                full_files[path.relative_to(system_dir).as_posix()] = path.read_bytes()
        for mode, payload in [("ota", ota_files), ("full", full_files)]:
            payload["SHA256SUMS"] = checksums(payload)
            archive = args.out / f"{stem}-{mode}.zip"
            write_zip(archive, payload)
            files[archive.name] = archive.read_bytes()
        for name, payload in files.items():
            (args.out / name).write_bytes(payload)
        (args.out / "SHA256SUMS").write_bytes(checksums(files))
    verify(args.out, require_clean=not args.allow_dirty)
    print(json.dumps({"status": "verified-release", "tag": tag, "source_commit": commit,
                      "ota_bytes": len(application), "full_bytes": len(full), "output": str(args.out)}))


def verify_checksums(files):
    require("SHA256SUMS" in files, "missing SHA256SUMS")
    expected = {}
    for line in files["SHA256SUMS"].decode().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([^\r\n]+)", line)
        require(match is not None, "invalid checksum line")
        digest, name = match.groups()
        require(name not in expected and name != "SHA256SUMS", "duplicate checksum entry")
        expected[name] = digest
    require(set(expected) == set(files) - {"SHA256SUMS"}, "checksum file inventory mismatch")
    for name, digest in expected.items():
        require(sha(files[name]) == digest, f"checksum mismatch: {name}")


def verify(directory, require_clean=True):
    files = {path.name: path.read_bytes() for path in directory.iterdir() if path.is_file()}
    verify_checksums(files)
    manifest = json.loads(files["manifest.json"])
    require(manifest["schema"] == 1 and manifest["project"] == "dangoOS", "unknown release manifest")
    require(not require_clean or not manifest["source_dirty"], "dirty source is not publishable")
    require(manifest["upstream"] == OFFICIAL, "upstream pin differs from release inputs")
    require(re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", manifest["tag"]) and
            manifest["tag"] == "v" + manifest["version"], "manifest tag/version mismatch")
    stem = f"dangoOS-{manifest['tag']}-esp32s31"
    expected_files = {"manifest.json", "INSTALL.md", "SHA256SUMS"} | {
        f"{stem}-{mode}.{extension}" for mode in ("ota", "full") for extension in ("bin", "zip")}
    require(set(files) == expected_files, "release asset inventory mismatch")
    require(re.fullmatch(r"[0-9a-f]{40}", manifest["source_commit"]), "invalid source commit")
    require((manifest["chip"], manifest["flash_bytes"], manifest["layout"]) ==
            ("esp32s31", FLASH_BYTES, "MOSAICO_S31_AB_LAYOUT_V1"), "manifest device/layout mismatch")
    images = {}
    for mode in ("ota", "full"):
        item = manifest["images"][mode]
        require(item["file"] == f"dangoOS-{manifest['tag']}-esp32s31-{mode}.bin", "unexpected artifact filename")
        payload = files[item["file"]]
        require(len(payload) == item["bytes"] and sha(payload) == item["sha256"], f"{mode} image mismatch")
        images[mode] = payload
    require(len(images["full"]) == FLASH_BYTES and manifest["images"]["full"]["flash_offset"] == 0,
            "full image size/offset mismatch")
    require(checked_application(images["ota"], manifest["version"]) == manifest["application"],
            "manifest application descriptor mismatch")
    table = partition_table()
    require(layout.parse_table(table) == manifest["partitions"], "release partition table mismatch")
    parts = {"partition_table": (layout.TABLE_AT, table), "otadata": (layout.OTA_AT, initial_otadata()),
             "application": (layout.APP_AT, images["ota"])}
    for name, pin in OFFICIAL["components"].items():
        payload = images["full"][pin["offset"]:pin["offset"] + pin["bytes"]]
        require(sha(payload) == pin["sha256"], f"full image {name} differs from official pin")
        parts[name] = (pin["offset"], payload)
    system = images["full"][SYSTEM_AT:SYSTEM_AT + SYSTEM_BYTES]
    require(sha(system) == manifest["components"]["system"]["sha256"], "system image hash mismatch")
    parts["system"] = (SYSTEM_AT, system)
    components = {name: {"offset": start, "bytes": len(payload), "sha256": sha(payload)}
                  for name, (start, payload) in parts.items()}
    require(components == manifest["components"], "manifest component map mismatch")
    require(images["full"] == merge_parts(parts), "full image has wrong placement, metadata or non-erased gaps")
    for mode in ("ota", "full"):
        image_name = manifest["images"][mode]["file"]
        archive_name = image_name.removesuffix(".bin") + ".zip"
        with zipfile.ZipFile(directory / archive_name) as archive:
            names = archive.namelist()
            require(len(names) == len(set(names)) and all(not name.startswith("/") and ".." not in Path(name).parts for name in names),
                    "unsafe or duplicate ZIP entry")
            require(archive.testzip() is None, "ZIP CRC mismatch")
            zipped = {name: archive.read(name) for name in names}
        verify_checksums(zipped)
        require(zipped[image_name] == images[mode] and zipped["manifest.json"] == files["manifest.json"] and
                zipped["INSTALL.md"] == files["INSTALL.md"],
                "ZIP and standalone artifacts differ")
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("build")
    command.add_argument("--out", type=Path, required=True)
    command.add_argument("--tag")
    command.add_argument("--elf", type=Path, default=ROOT / "target/riscv32imafc-unknown-none-elf/release/os-fw")
    command.add_argument("--upstream-image", type=Path)
    command.add_argument("--allow-dirty", action="store_true")
    command = commands.add_parser("verify")
    command.add_argument("directory", type=Path)
    command.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    try:
        if args.command == "build":
            build(args)
        else:
            manifest = verify(args.directory, require_clean=not args.allow_dirty)
            print(json.dumps({"status": "verified-release", "tag": manifest["tag"], "source_commit": manifest["source_commit"]}))
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(2, f"release rejected: {error}\n")


if __name__ == "__main__":
    main()
