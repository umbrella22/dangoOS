#!/usr/bin/env python3
"""Build and independently inspect dangoOS main/full-install release artifacts."""
import argparse
import hashlib
import json
from pathlib import Path
import re
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
import layout
from main_image import validate_image
import build_bootloader
import resource_bundle
sys.path.insert(0, str(ROOT / "scripts/check"))
import resource_budget

FLASH_BYTES = 0x1000000
SYSTEM_AT = 0xE10000
SYSTEM_BYTES = 0x1F0000


def sha(data):
    return hashlib.sha256(data).hexdigest()


def require(condition, message):
    if not condition:
        raise ValueError(message)


def json_bytes(value):
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode()


def checked_application(data, version):
    info = validate_image(data)
    require(info["version"] == version, "application descriptor and release version differ")
    return info


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


def merge_parts(parts):
    result = bytearray(b"\xff" * FLASH_BYTES)
    end = 0
    for name, (start, payload) in sorted(parts.items(), key=lambda item: item[1][0]):
        require(payload and start >= end and start + len(payload) <= FLASH_BYTES,
                f"overlapping or out-of-range component: {name}")
        result[start:start + len(payload)] = payload
        end = start + len(payload)
    return bytes(result)


def checked_recovery(data):
    return validate_image(data, project_name="dango-recovery", maximum=layout.RECOVERY_BYTES, marker=None)


def checked_bootloader(data):
    build_bootloader.checked_image(data)


def assemble(bootloader, recovery, application, system, table, version):
    application_info = checked_application(application, version)
    checked_bootloader(bootloader)
    recovery_info = checked_recovery(recovery)
    require(recovery_info["version"] == version, "Recovery version differs from main")
    rows = layout.parse_table(table)
    layout.require_layout(rows)
    require(len(system) == SYSTEM_BYTES, "system image has the wrong partition size")
    parts = {"bootloader": (0x2000, bootloader), "recovery": (layout.RECOVERY_AT, recovery),
             "partition_table": (layout.TABLE_AT, table), "boot_state": (layout.JOURNAL_AT, layout.boot_records(application)),
             "application": (layout.APP_AT, application), "system": (SYSTEM_AT, system)}
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


def verify_nand_skins(catalog, files):
    require(isinstance(catalog, list) and catalog, "missing NAND skin catalog")
    expected = set()
    ids = set()
    for skin in catalog:
        identity = skin.get("id")
        require(isinstance(identity, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,63}", identity)
                and identity != "placeholder" and identity not in ids, "invalid/duplicate NAND skin id")
        ids.add(identity)
        require(set(skin["files"]) == {"fox.bin", "skin.json"}, "NAND skin file inventory")
        for name, info in skin["files"].items():
            path = f"dango-skin/{identity}/{name}"
            expected.add(path)
            require(path in files and len(files[path]) == info["bytes"] and sha(files[path]) == info["sha256"],
                    f"NAND skin missing or changed: {path}")
        metadata = json.loads(files[f"dango-skin/{identity}/skin.json"])
        binary = files[f"dango-skin/{identity}/fox.bin"]
        require(metadata["id"] == identity and metadata["name"] == skin["name"]
                and metadata["version"] == skin["version"] and metadata["canvas"] == [352, 336]
                and metadata["schema_version"] == 1
                and metadata["binary_bytes"] == len(binary) <= 1024 * 1024
                and metadata["binary_crc32"] == zlib.crc32(binary), "NAND skin manifest mismatch")
    require({name for name in files if name.startswith("dango-skin/")} == expected,
            "NAND skin archive inventory mismatch")


def verify_resource_bundles(manifest, files):
    if "ui_fonts" not in manifest and "application_bundles" not in manifest:
        return
    require("tools/resource_bundle.py" in files, "resource validation tools missing")
    # Only verified relative paths reach this materialization; never execute
    # validation code from the archive being inspected.
    with tempfile.TemporaryDirectory(prefix="verify-resources-") as temporary:
        root = Path(temporary)
        for name, data in files.items():
            if name.startswith(("applications/", "ui-fonts/")):
                path = Path(name)
                require(not path.is_absolute() and all(p not in (".", "..") for p in path.parts)
                        and "\\" not in name, "unsafe resource archive path")
                target = root / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
        require(resource_bundle.fonts(root / "ui-fonts") == manifest["ui_fonts"], "UI font catalog mismatch")
        ids = set()
        for bundle in manifest["application_bundles"]:
            identity = bundle.get("id")
            require(isinstance(identity, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,63}", identity)
                    and identity not in ids, "invalid/duplicate application bundle id")
            ids.add(identity)
            require(resource_bundle.application(root / "applications" / identity) == bundle, "application bundle catalog mismatch")
        require({p.name for p in (root / "applications").iterdir()} == ids, "unexpected application bundle")


def verify_fonts_pin(application, fonts):
    pins = re.findall(rb"DANGO_UI_FONTS_V1:([0-9a-f]{64}):([0-9]+);", application)
    require(pins == [(fonts["id"].encode("ascii"), str(fonts["bytes"]).encode("ascii"))],
            "firmware UI font pin differs from resource bundle; rebuild firmware from current resource inputs")


def install_notes(tag, stem):
    return f"""# dangoOS {tag} 安装与救援

硬件：ESP-Mosaico V1.2 / ESP32-S31，16 MiB NOR。

## 首次安装

解压 `{stem}-full.zip`，按住 BOOT、短按 RESET、松开 BOOT，执行：

```sh
python3 -m pip install -r requirements.txt
python3 install.py --flash-port ROM_PORT --port RUNTIME_PORT
```

全量刷写替换引导、Recovery、分区、主应用与 system，重置 NOR 配置。
独立 NAND 的应用、照片和资源保留。无法挂载 NAND 时安装器退出；
只有需要清空该 NAND 时才添加 `--format-nand`。已挂载卷不会格式化。

## 主应用更新

`{stem}-main.zip` 只适用于 S31ONE03。USB 更新前在主系统执行：

```sh
python3 tools/console.py recovery --port RUNTIME_PORT
python3 tools/recovery.py --port RECOVERY_PORT install {stem}-main.bin --boot
```

Recovery 只写主应用和启动记录，完整读回后试启动。更新中断或健康确认失败返回
Recovery，连接 USB 重新安装。单主应用不会保留上一版系统镜像。
也可以连接 Wi-Fi，在“设置 → 系统更新”中下载并交给 Recovery 安装。
更新包 SHA256 用于完整性检查；当前包没有发布者数字签名。

主系统损坏时，按住 AI 键启动以进入 Recovery。ROM 下载使用 BOOT 键。
Recovery 默认提供只读导出：

```sh
python3 tools/recovery.py --port RECOVERY_PORT info
python3 tools/recovery.py --port RECOVERY_PORT nor backup-nor.bin
python3 tools/recovery.py --port RECOVERY_PORT nand backup-nand.bin --raw
```

## 应用与团子资源

两个 ZIP 都带有 `applications/<id>/`、`ui-fonts/` 和四套 `dango-skin/`。
运行系统后执行 `python3 install.py --port RUNTIME_PORT` 安装匹配字体与团子。
添加 `--update-apps` 替换同 ID 的 Data 应用，存档保留。默认团子包未安装时显示
狐狸虚影和问号。脚本、图片、图标和团子更新无需重新刷写主应用。

```sh
python3 tools/console.py bundle-push applications/snake --port RUNTIME_PORT
python3 tools/console.py fonts-push ui-fonts --port RUNTIME_PORT
python3 tools/console.py reboot --port RUNTIME_PORT
python3 tools/console.py boot-status --port RUNTIME_PORT
```

更新后检查版本 `{tag[1:]}` 和启动状态 `Ready`。主机测试与镜像校验不代替设备验收；
本发布的设备验证范围见 `manifest.json` 的 `hardware_validation`。
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
    bootloader = args.bootloader.read_bytes()
    receipt_path = args.bootloader_receipt or args.bootloader.parent.parent / "receipt.json"
    loader_receipt = json.loads(receipt_path.read_text())
    require(loader_receipt["sha256"] == sha(bootloader) and loader_receipt["bytes"] == len(bootloader)
            and loader_receipt["layout"] == layout.LAYOUT
            and loader_receipt["idf_revision"] == (ROOT / "bootloader/idf-revision.txt").read_text().strip()
            and loader_receipt["source_sha256"] == build_bootloader.source_sha256(),
            "bootloader build receipt differs from current source or binary")
    table = partition_table()
    catalog = json.loads(subprocess.check_output(
        ["cargo", "run", "-p", "app-pack", "--locked", "--offline", "--", "skins", "dango-skin"],
        cwd=ROOT, text=True))["skins"]
    skin_files = {f"dango-skin/{skin['id']}/{name}": (ROOT / "dango-skin" / skin["id"] / name).read_bytes()
                  for skin in catalog for name in skin["files"]}
    verify_nand_skins(catalog, skin_files)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="release-build-", dir=ROOT / "target") as temporary:
        temporary = Path(temporary)
        main_path = temporary / "application.bin"
        subprocess.run(["espflash", "save-image", "--chip", "esp32s31", "--flash-size", "16mb",
                        "--flash-mode", "dio", "--flash-freq", "40mhz", str(args.elf), str(main_path)], check=True)
        application = main_path.read_bytes()
        recovery_path = temporary / "recovery.bin"
        subprocess.run(["espflash", "save-image", "--chip", "esp32s31", "--flash-size", "16mb",
                        "--flash-mode", "dio", "--flash-freq", "40mhz", str(args.recovery_elf), str(recovery_path)], check=True)
        recovery = recovery_path.read_bytes()
        checked_application(application, version)
        resources = resource_budget.assess(args.elf.read_bytes(), application)
        require(resources["status"] == "pass", "; ".join(resources["failures"]))
        system_dir = temporary / "system"
        subprocess.run(["cargo", "run", "-p", "app-pack", "--locked", "--offline", "--",
                        "system", "apps", str(system_dir), "--size", str(SYSTEM_BYTES)], cwd=ROOT, check=True)
        system = (system_dir / "system.bin").read_bytes()
        report = json.loads((system_dir / "file-tree-report.json").read_text())
        require(report["status"] == "verified-system-image" and report["output_sha256"] == sha(system),
                "system image verification receipt mismatch")
        application_dir = temporary / "applications"
        applications = json.loads(subprocess.check_output(
            ["cargo", "run", "-p", "app-pack", "--locked", "--offline", "--",
             "bundles", "apps", str(application_dir)], cwd=ROOT, text=True))
        font_dir = temporary / "ui-fonts"
        fonts = json.loads(subprocess.check_output(
            ["cargo", "run", "-p", "app-pack", "--locked", "--offline", "--",
             "fonts", str(font_dir)], cwd=ROOT, text=True))
        verify_fonts_pin(application, fonts)
        bundle_files = {path.relative_to(temporary).as_posix(): path.read_bytes()
                        for directory in (application_dir, font_dir)
                        for path in sorted(directory.rglob("*")) if path.is_file()}
        full, components, rows, info = assemble(bootloader, recovery, application, system, table, version)
        stem = f"dangoOS-{tag}-esp32s31"
        manifest = {
            "schema": 2, "project": "dangoOS", "version": version, "tag": tag,
            "source_commit": commit, "source_dirty": dirty, "board": "ESP-Mosaico V1.2",
            "chip": "esp32s31", "flash_bytes": FLASH_BYTES, "layout": layout.LAYOUT,
            "bootloader_build": loader_receipt, "partitions": rows, "components": components,
            "application": info, "hardware_validation": "not-run for this release",
            "resource_budget": resources,
            "nand_skins": catalog,
            "application_bundles": applications,
            "ui_fonts": fonts,
            "images": {
                "main": {"file": f"{stem}-main.bin", "bytes": len(application), "sha256": sha(application),
                        "delivery": "Recovery USB or closed NAND package", "preserves_user_data": True},
                "full": {"file": f"{stem}-full.bin", "bytes": len(full), "sha256": sha(full),
                         "flash_offset": 0, "preserves_user_data": False},
            },
            "tools": {"rust": subprocess.check_output(["rustc", "--version"], text=True).strip(),
                      "espflash": subprocess.check_output(["espflash", "--version"], text=True).strip()},
        }
        args.out.mkdir()
        files = {f"{stem}-main.bin": application, f"{stem}-full.bin": full,
                 "manifest.json": json_bytes(manifest), "INSTALL.md": install_notes(tag, stem)}
        requirements = b"pyserial==3.5\nesptool==5.6.0\n"
        common = {"manifest.json": files["manifest.json"], "INSTALL.md": files["INSTALL.md"],
                  "requirements.txt": requirements, **skin_files, **bundle_files,
                  "install.py": (HERE / "install.py").read_bytes(),
                  "tools/console.py": (ROOT / "scripts/device/console.py").read_bytes(),
                  "tools/resource_bundle.py": (ROOT / "scripts/device/resource_bundle.py").read_bytes(),
                  "tools/main_image.py": (ROOT / "scripts/device/main_image.py").read_bytes(),
                  "tools/recovery.py": (ROOT / "scripts/device/recovery.py").read_bytes()}
        main_files = {**common, f"{stem}-main.bin": application}
        full_files = {**common, f"{stem}-full.bin": full, "partitions.csv": (HERE / "partitions.csv").read_bytes(),
                      "system-file-tree.json": json_bytes(report),
                      "UPSTREAM.md": (HERE / "UPSTREAM.md").read_bytes()}
        for path in sorted((system_dir / "application-docs").rglob("*")):
            if path.is_file():
                full_files[path.relative_to(system_dir).as_posix()] = path.read_bytes()
        for mode, payload in [("main", main_files), ("full", full_files)]:
            payload["SHA256SUMS"] = checksums(payload)
            archive = args.out / f"{stem}-{mode}.zip"
            write_zip(archive, payload)
            files[archive.name] = archive.read_bytes()
        for name, payload in files.items():
            (args.out / name).write_bytes(payload)
        (args.out / "SHA256SUMS").write_bytes(checksums(files))
    verify(args.out, require_clean=not args.allow_dirty)
    print(json.dumps({"status": "verified-release", "tag": tag, "source_commit": commit,
                      "main_bytes": len(application), "full_bytes": len(full), "output": str(args.out)}))


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
    require(manifest["schema"] == 2 and manifest["project"] == "dangoOS", "unknown release manifest")
    require(not require_clean or not manifest["source_dirty"], "dirty source is not publishable")
    require(re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", manifest["tag"]) and
            manifest["tag"] == "v" + manifest["version"], "manifest tag/version mismatch")
    stem = f"dangoOS-{manifest['tag']}-esp32s31"
    expected_files = {"manifest.json", "INSTALL.md", "SHA256SUMS"} | {
        f"{stem}-{mode}.{extension}" for mode in ("main", "full") for extension in ("bin", "zip")}
    require(set(files) == expected_files, "release asset inventory mismatch")
    require(re.fullmatch(r"[0-9a-f]{40}", manifest["source_commit"]), "invalid source commit")
    require((manifest["chip"], manifest["flash_bytes"], manifest["layout"]) ==
            ("esp32s31", FLASH_BYTES, layout.LAYOUT), "manifest device/layout mismatch")
    images = {}
    for mode in ("main", "full"):
        item = manifest["images"][mode]
        require(item["file"] == f"dangoOS-{manifest['tag']}-esp32s31-{mode}.bin", "unexpected artifact filename")
        payload = files[item["file"]]
        require(len(payload) == item["bytes"] and sha(payload) == item["sha256"], f"{mode} image mismatch")
        images[mode] = payload
    require(len(images["full"]) == FLASH_BYTES and manifest["images"]["full"]["flash_offset"] == 0,
            "full image size/offset mismatch")
    require(checked_application(images["main"], manifest["version"]) == manifest["application"],
            "manifest application descriptor mismatch")
    if "ui_fonts" in manifest:
        verify_fonts_pin(images["main"], manifest["ui_fonts"])
    table = partition_table()
    require(layout.parse_table(table) == manifest["partitions"], "release partition table mismatch")
    parts = {"partition_table": (layout.TABLE_AT, table), "boot_state": (layout.JOURNAL_AT, layout.boot_records(images["main"])),
             "application": (layout.APP_AT, images["main"])}
    expected = {"bootloader": (0x2000, 0x6000), "recovery": (layout.RECOVERY_AT, layout.RECOVERY_BYTES)}
    for name, (offset, maximum) in expected.items():
        pin = manifest["components"][name]
        require(pin["offset"] == offset and 24 <= pin["bytes"] <= maximum, f"{name} bounds")
        payload = images["full"][offset:offset + pin["bytes"]]
        require(sha(payload) == pin["sha256"], f"full image {name} checksum mismatch")
        parts[name] = (offset, payload)
    checked_bootloader(parts["bootloader"][1])
    checked_recovery(parts["recovery"][1])
    loader = manifest["bootloader_build"]
    require(loader["sha256"] == sha(parts["bootloader"][1]) and loader["layout"] == layout.LAYOUT
            and loader["idf_revision"] == (ROOT / "bootloader/idf-revision.txt").read_text().strip(),
            "bootloader provenance mismatch")
    system = images["full"][SYSTEM_AT:SYSTEM_AT + SYSTEM_BYTES]
    require(sha(system) == manifest["components"]["system"]["sha256"], "system image hash mismatch")
    parts["system"] = (SYSTEM_AT, system)
    components = {name: {"offset": start, "bytes": len(payload), "sha256": sha(payload)}
                  for name, (start, payload) in parts.items()}
    require(components == manifest["components"], "manifest component map mismatch")
    require(images["full"] == merge_parts(parts), "full image has wrong placement, metadata or non-erased gaps")
    for mode in ("main", "full"):
        image_name = manifest["images"][mode]["file"]
        archive_name = image_name.removesuffix(".bin") + ".zip"
        with zipfile.ZipFile(directory / archive_name) as archive:
            names = archive.namelist()
            require(len(names) == len(set(names)) and all(not name.startswith("/") and ".." not in Path(name).parts for name in names),
                    "unsafe or duplicate ZIP entry")
            require(archive.testzip() is None, "ZIP CRC mismatch")
            zipped = {name: archive.read(name) for name in names}
        verify_checksums(zipped)
        verify_resource_bundles(manifest, zipped)
        if "nand_skins" in manifest:
            verify_nand_skins(manifest["nand_skins"], zipped)
            require({"install.py", "tools/console.py", "tools/main_image.py", "tools/recovery.py", "requirements.txt"} <= set(zipped),
                    "NAND installation tools missing")
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
    command.add_argument("--bootloader", type=Path, required=True)
    command.add_argument("--bootloader-receipt", type=Path)
    command.add_argument("--recovery-elf", type=Path, default=ROOT / "target/riscv32imafc-unknown-none-elf/release/recovery-fw")
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
