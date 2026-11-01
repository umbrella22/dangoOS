#!/usr/bin/env python3
"""Install release resources, optionally flashing the bundled full NOR image first."""
import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import time
import zlib


def require(condition, message):
    if not condition:
        raise ValueError(message)


def validate_bundle(root):
    """Validate all shipped files before opening a port or running esptool."""
    root = root.resolve()
    listed = {}
    for line in (root / "SHA256SUMS").read_text().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([^\r\n]+)", line)
        require(match is not None, "invalid SHA256SUMS line")
        digest, name = match.groups()
        path = PurePosixPath(name)
        require(not path.is_absolute() and all(p not in (".", "..") for p in path.parts)
                and path.as_posix() == name and "\\" not in name and name not in listed
                and (root / name).resolve().is_relative_to(root), "unsafe/duplicate checksum path")
        require(hashlib.sha256((root / name).read_bytes()).hexdigest() == digest,
                f"checksum mismatch: {name}")
        listed[name] = digest
    require({"manifest.json", "install.py", "tools/console.py", "tools/ota_image.py"} <= set(listed),
            "installation bundle is incomplete")
    manifest = json.loads((root / "manifest.json").read_bytes())
    require(manifest.get("project") == "dangoOS" and manifest.get("schema") == 1,
            "unsupported release manifest")
    require(manifest.get("chip") == "esp32s31" and manifest.get("flash_bytes") == 16 * 1024 * 1024
            and manifest.get("layout") == "MOSAICO_S31_AB_LAYOUT_V2_6M_NAND",
            "unsupported device or storage layout")
    tag = manifest.get("tag", "")
    require(re.fullmatch(r"v\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", tag)
            and tag == "v" + manifest.get("version", ""), "invalid release version")
    available = 0
    for mode in ("ota", "full"):
        item = manifest["images"][mode]
        require(item["file"] == f"dangoOS-{tag}-esp32s31-{mode}.bin", "unexpected image filename")
        if item["file"] in listed:
            image = root / item["file"]
            require(listed[item["file"]] == item["sha256"] and image.stat().st_size == item["bytes"],
                    f"{mode} image mismatch")
            available += 1
    require(available > 0, "bundle contains no firmware image")
    catalog = manifest.get("nand_skins")
    require(isinstance(catalog, list) and catalog, "release contains no NAND skin catalog")
    ids, expected = set(), set()
    for skin in catalog:
        identity = skin.get("id")
        require(isinstance(identity, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,63}", identity)
                and identity != "placeholder" and identity not in ids, "invalid/duplicate skin id")
        ids.add(identity)
        require(set(skin["files"]) == {"fox.bin", "skin.json"}, "skin needs BIN and JSON")
        for name, info in skin["files"].items():
            relative = f"dango-skin/{identity}/{name}"
            expected.add(relative)
            require(listed.get(relative) == info["sha256"], f"skin not covered by checksums: {relative}")
            require((root / relative).stat().st_size == info["bytes"], f"skin size mismatch: {relative}")
        directory = root / "dango-skin" / identity
        metadata = json.loads((directory / "skin.json").read_bytes())
        binary = (directory / "fox.bin").read_bytes()
        require(metadata["id"] == identity and metadata["name"] == skin["name"]
                and metadata["version"] == skin["version"] and metadata["schema_version"] == 1
                and metadata["canvas"] == [352, 336]
                and metadata["binary_bytes"] == len(binary) <= 1024 * 1024
                and metadata["binary_crc32"] == zlib.crc32(binary), "skin metadata mismatch")
    require({p.relative_to(root).as_posix() for p in (root / "dango-skin").rglob("*") if p.is_file()} == expected,
            "unexpected/missing NAND skin files")
    if "ui_fonts" in manifest or "application_bundles" in manifest:
        require("tools/resource_bundle.py" in listed, "resource validation tools missing")
        sys.path.insert(0, str(root / "tools"))
        import resource_bundle
        require(resource_bundle.fonts(root / "ui-fonts") == manifest["ui_fonts"], "UI fonts catalog mismatch")
        ids = set()
        for bundle in manifest["application_bundles"]:
            identity = bundle.get("id")
            require(isinstance(identity, str) and re.fullmatch(r"[A-Za-z0-9_-]{1,63}", identity)
                    and identity not in ids, "invalid/duplicate application bundle id")
            ids.add(identity)
            require(resource_bundle.application(root / "applications" / identity) == bundle,
                    "application catalog mismatch")
        resource_files = {p.relative_to(root).as_posix() for directory in (root / "ui-fonts", root / "applications")
                          for p in directory.rglob("*") if p.is_file()}
        require(resource_files <= set(listed), "resource file missing from checksums")
    return manifest


def flash_full(root, manifest, port):
    item = manifest["images"]["full"]
    require(item["file"] == f"dangoOS-{manifest['tag']}-esp32s31-full.bin", "unexpected full image filename")
    image = root / item["file"]
    data = image.read_bytes()
    require(item.get("flash_offset") == 0 and len(data) == item["bytes"] == 16 * 1024 * 1024
            and hashlib.sha256(data).hexdigest() == item["sha256"], "full NOR image mismatch")
    subprocess.run([sys.executable, "-m", "esptool", "--chip", "esp32s31", "--port", port,
                    "--baud", "460800", "--before", "no-reset", "--after", "hard-reset",
                    "write-flash", "0x0", str(image)], check=True)


def connect(console, port, timeout=120):
    deadline = time.monotonic() + timeout
    last = "runtime port unavailable"
    while time.monotonic() < deadline:
        device = None
        try:
            device = console.Device(port, False)
            device.command("hello", timeout=2)
            return device
        except (OSError, SystemExit) as error:
            last = str(error)
            if device is not None:
                device.ser.close()
            time.sleep(1)
    raise ValueError(f"runtime connection failed on {port}: {last}")


def install_skins(console, device, root, catalog, format_nand=False):
    # The existing per-skin staging/commit protocol preserves other packages,
    # app data and the durable selected character. Never select a bundled skin.
    deadline = time.monotonic() + 120
    while True:
        fields = console.response_fields(device.command("nand-status", timeout=30))
        if fields.get("ready") != "0":
            break
        require(time.monotonic() < deadline, "NAND boot probe did not finish; no resources installed")
        time.sleep(1)
    mounted = fields.get("mounted")
    require(mounted in ("0", "1"), "NAND status did not report mounted state")
    if mounted == "0":
        require(format_nand, "NAND is not mounted. For a new or disposable volume, rerun with --format-nand; this erases NAND.")
        device.command("nand-format", timeout=120)
    for skin in catalog:
        console.push_skin(device, str(root / "dango-skin" / skin["id"]), False)
    status = device.command("skin-ls", timeout=30)
    installed = set(status.split()[2:])
    require(all(skin["id"] in installed for skin in catalog), "installed NAND skin catalog is incomplete")
    print(f"Installed {len(catalog)} NAND skins; current character selection preserved.")


def install_resources(console, device, root, manifest, format_nand=False, update_apps=False):
    # Check firmware compatibility before writing any package. A first upgrade
    # from embedded fonts installs firmware first, then resources, then restarts.
    fonts = manifest.get("ui_fonts")
    if fonts is not None:
        fields = console.response_fields(device.command("fonts-status"))
        require(fields.get("expected") == fonts["id"], "font resources do not match running firmware; install firmware first")
    install_skins(console, device, root, manifest["nand_skins"], format_nand)
    if fonts is not None:
        console.push_fonts(device, str(root / "ui-fonts"))
    if update_apps:
        for bundle in manifest.get("application_bundles", []):
            console.push_bundle(device, str(root / "applications" / bundle["id"]), False)
    if fonts is not None:
        print("Resources installed. Restart the device to activate UI fonts.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", required=True, help="runtime USB console port")
    parser.add_argument("--flash-port", help="ROM download port; flash full NOR before installing skins")
    parser.add_argument("--format-nand", action="store_true",
                        help="erase and initialize NAND only if it is unmounted; existing mounted volumes are preserved")
    parser.add_argument("--update-apps", action="store_true",
                        help="install bundled Lua applications as Data updates, replacing same-ID Data apps; saves are preserved")
    parser.add_argument("--bundle", type=Path, default=Path(__file__).resolve().parent,
                        help="extracted release ZIP directory")
    args = parser.parse_args()
    try:
        root = args.bundle.resolve()
        manifest = validate_bundle(root)
        if args.flash_port:
            flash_full(root, manifest, args.flash_port)
        sys.path.insert(0, str(root / "tools"))
        import console
        device = connect(console, args.port)
        try:
            install_resources(console, device, root, manifest, args.format_nand, args.update_apps)
        finally:
            device.ser.close()
    except (ValueError, KeyError, TypeError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(2, f"installation failed: {error}\n")


if __name__ == "__main__":
    main()
