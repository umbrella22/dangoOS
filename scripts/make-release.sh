#!/usr/bin/env bash
# 构建 ESP-Mosaico (v1.2) 固件交付包
#
# 烧录策略（重要）:
#   Mosaico 出厂自带官方 bootloader(@0x2000) 与分区表(@0x9000, factory@0x20000)。
#   日常交付只烧 app@0x20000 —— 板上的官方 bootloader/分区表保持不动。
#   合并镜像(整片救砖用)需要与出厂完全一致的官方 bootloader 二进制;
#   若 crates/fan-firmware/assets/boot/mosaico-bootloader.bin 存在(从官方
#   ESP-Mosaico-Claw 出厂镜像 0x2000 处提取, 尾部 SHA256 自校验通过)则自动附带
#   合并镜像, 否则跳过并在包内注明。
#
# 官方分区布局: nvs@0xA000 / otadata@0x10000 / phy@0x12000 / factory@0x20000(8MB)
#               ui_apps@0x7F0000 / system@0xB10000
#
# 用法: ./scripts/make-release.sh <版本>
#      ./scripts/make-release.sh <版本> --no-build  复用现有 ELF
#
# 产物: releases/fan-control-<版本>-<日期>/
#   fan-control-<版本>-app.bin             app 单件 @0x20000（日常烧录只用这个）
#   fan-control-<版本>-partition-table.bin @0x9000（仅救砖时用）
#   fan-control-<版本>-bootloader.bin      @0x2000（可选, 有官方提取件才附带）
#   fan-control-<版本>-merged.bin          整片镜像（可选, 同上）
#   fan-control-<版本>.elf                 调试符号（espcoredump/addr2line 用）
#   README.md + BUILD.json + SHA256SUMS + 同名 zip
#
# 为什么整片镜像是「拼」出来的而不是 espflash --merge 一键生成:
#   bootloader 必须用官方出厂那份（已验证可引导 S31 + 官方分区布局）；
#   espflash 不会为 esp32s31 生成 bootloader, IDF 自建是另一个未验证 build。
#   交付包只换 app, 把变量降到最少。
#
# 版本说明放在 docs/releases/<版本>.md，自动附到包内 README 末尾。
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

if [[ $# -lt 1 || $# -gt 2 || "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then
    echo "用法: $0 <版本，例如 v23> [--no-build]"
    if [[ "${1:-}" == "--help" || "${1:-}" == "-h" ]]; then exit 0; fi
    exit 1
fi
VER="$1"
NO_BUILD="${2:-}"
if [[ ! "$VER" =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]*$ ]]; then
    echo "版本名须以字母或数字开头，只能包含字母、数字、点、下划线和连字符" >&2
    exit 1
fi
if [[ -n "$NO_BUILD" && "$NO_BUILD" != "--no-build" ]]; then
    echo "未知参数: $NO_BUILD" >&2
    exit 1
fi
DATE="$(date +%Y%m%d)"
PKG="releases/fan-control-${VER}-${DATE}"
TARGET="riscv32imafc-unknown-none-elf"
ELF="${CARGO_TARGET_DIR:-target}/${TARGET}/release/fan-control-fw"
PT_ASSET="crates/fan-firmware/assets/boot/mosaico-partition-table.bin"
BOOT_ASSET="crates/fan-firmware/assets/boot/mosaico-bootloader.bin"
# 官方出厂布局
PT_OFF=$((0x9000))
APP_OFF=$((0x20000))
APP_MAX=$((0x20000 + 0x7D0000))   # factory 分区末尾
FLASH=$((0x400000))               # 合并镜像长度(app 结束远小于此)

[ -f "$PT_ASSET" ]   || { echo "❌ 缺 $PT_ASSET"; exit 1; }
if [[ -e "$PKG" || -e "${PKG}.zip" ]]; then
    echo "交付包已存在，请使用新的版本名: $PKG" >&2
    exit 1
fi
HAS_BOOT=0
[ -f "$BOOT_ASSET" ] && HAS_BOOT=1

if [ "$NO_BUILD" != "--no-build" ]; then
    echo "== 1/4 编译 fan-firmware release =="
    cargo build --locked --release -p fan-firmware --bin fan-control-fw --target "$TARGET"
else
    echo "== 1/4 跳过编译，复用现有 ELF =="
fi
[ -f "$ELF" ] || { echo "❌ 找不到 ELF: $ELF"; exit 1; }

mkdir -p releases
mkdir "$PKG"
echo "== 2/4 生成 app 镜像 =="
espflash save-image --chip esp32s31 "$ELF" "$PKG/fan-control-${VER}-app.bin"

echo "== 3/4 校验 + 拼合并镜像 =="
python3 - "$PKG" "$VER" "$PT_ASSET" "$BOOT_ASSET" "$PT_OFF" "$APP_OFF" "$APP_MAX" "$FLASH" "$HAS_BOOT" <<'PY'
import sys, os, hashlib
pkg, ver, pt_p, boot_p, pt_off, app_off, app_max, flash, has_boot = sys.argv[1:]
pt_off, app_off, app_max, flash = int(pt_off), int(app_off), int(app_max), int(flash)
has_boot = has_boot == "1"
pt = open(pt_p, 'rb').read()
app = open(os.path.join(pkg, f"fan-control-{ver}-app.bin"), 'rb').read()

# --- 自校验: 别交付坏镜像 ---
assert pt[:2] == b'\xaa\x50', "分区表 magic 错"
n = 0
while pt[n:n+2] == b'\xaa\x50':
    n += 32
assert pt[n:n+2] == b'\xeb\xeb', "分区表缺 MD5 记录"
assert hashlib.md5(pt[:n]).digest() == pt[n+16:n+32], "分区表 MD5 不匹配"
assert app[0] == 0xE9, "app 头 magic 错"
assert hashlib.sha256(app[:-32]).digest() == app[-32:], "app 尾部 SHA256 不匹配"
assert app_off + len(app) <= app_max, "app 超出 factory 分区"

if has_boot:
    boot = open(boot_p, 'rb').read()
    assert boot[0] == 0xE9, "bootloader 头 magic 错"
    assert hashlib.sha256(boot[:-32]).digest() == boot[-32:], "bootloader 尾部 SHA256 不匹配"
    assert 0x2000 + len(boot) <= pt_off, "bootloader 越过分区表起点"
    img = bytearray(b'\xff' * flash)
    img[0:0x2000] = b'\xff' * 0x2000   # 0x0..0x2000 保留（S31 家族布局）
    img[0x2000:0x2000+len(boot)] = boot
    img[pt_off:pt_off+len(pt)] = pt
    img[app_off:app_off+len(app)] = app
    out = os.path.join(pkg, f"fan-control-{ver}-merged.bin")
    open(out, 'wb').write(img)
    print(f"   bootloader {len(boot)}B @0x2000 + 分区表 {len(pt)}B @0x{pt_off:x} + app {len(app)}B @0x{app_off:x}")
    print(f"   整片 {len(img)}B → {out}")
else:
    print("   ⚠️ 无官方 bootloader 提取件(mosaico-bootloader.bin), 跳过合并镜像")
    print("      → 包内只含 app(烧 @0x20000, 板上官方 bootloader/分区表不动)")
print(f"   app 占 factory 分区 {len(app)/1048576:.2f}MB / 8.0MB")
PY

echo "== 4/4 附属文件 + ELF + 校验和 =="
cp "$PT_ASSET"   "$PKG/fan-control-${VER}-partition-table.bin"
cp "$ELF"        "$PKG/fan-control-${VER}.elf"
if [[ "$HAS_BOOT" == 1 ]]; then
    cp "$BOOT_ASSET" "$PKG/fan-control-${VER}-bootloader.bin"
fi

cat > "$PKG/README.md" <<MD
# ikaros 风扇控制 ${VER} — ESP-Mosaico (v1.2) 真机固件

构建时间: ${DATE}   目标板: ESP-Mosaico v1.2 + CO5300 480×480 QSPI AMOLED

## 包里是什么

| 文件 | 烧录地址 | 说明 |
|------|---------|------|
| \`fan-control-${VER}-app.bin\` | **0x20000** | 只有应用，**日常烧录用这个**；板上官方 bootloader/分区表保持不动 |
| \`fan-control-${VER}-partition-table.bin\` | **0x9000** | 官方出厂布局复刻件，仅救砖时用 |
| \`fan-control-${VER}-bootloader.bin\` | **0x2000** | 官方出厂提取件（存在时才有此文件），S31 家族布局**不是 0x0** |
| \`fan-control-${VER}-merged.bin\` | **0x0** | 整片镜像（存在时才有此文件），换板/救砖一把烧 |
| \`fan-control-${VER}.elf\` | — | 调试符号，崩溃回溯用 |
| \`BUILD.json\` | — | 工具版本、锁定依赖、源码与 ELF 哈希 |

## 烧录

**方式一（唯一日常方式）：只烧 app**，地址 **0x20000**，flash_download_tool / esptool 均可：
\`\`\`
esptool.py --chip esp32s31 write_flash 0x20000 fan-control-${VER}-app.bin
\`\`\`

**方式二：整片一把烧**（仅当包内有 merged.bin；换板/官方 bootloader 被破坏时用）：
\`\`\`
esptool.py --chip esp32s31 write_flash 0x0 fan-control-${VER}-merged.bin
\`\`\`

串口参数：**115200 8N1**（板载 Type-C 桥接 UART0）。烧完看串口日志，正常顺序：
1. \`ikaros 风扇控制固件启动 (ESP-Mosaico / 480x480 QSPI AMOLED)\`
2. \`板硬件版本 v1.2 (variant=v1.2)\` —— 若这里 panic 说明 eFuse 版本不是 v1.1/v1.2
3. \`CO5300 已启动: 480x480 QSPI@40MHz ...\` → 屏幕亮起显示启动页
4. \`CST9220 已就绪\` 或 \`未检测到触摸控制器\`
5. 之后每 2 秒输出简短 \`心跳\`（渲染/帧率/GRAM 刷新/触摸间隔）

## 验收时请重点看串口心跳

- \`渲染 avg/max\`：Slint 软渲染单帧耗时（方屏全帧约几十 ms 量级）。
- \`帧=(x.xHz)\`：模型帧率；\`GRAM刷新avg/max\`：整帧 QSPI 传输耗时（约 25ms 量级，40MHz 四线）。
- \`DMA错误=\` 应恒为 0；持续增长说明 QSPI 时序/信号完整性有问题。
- 触摸按下/抬起应即时打印坐标；无触摸板则确认 \`未检测到触摸控制器\` 一行。
- 温度、转速和图案应保持位置稳定，触摸换档后完整更新。

## 校验

\`\`\`
shasum -a 256 -c SHA256SUMS
\`\`\`
MD
if [ -f "docs/releases/${VER}.md" ]; then
    { echo; echo "---"; echo; cat "docs/releases/${VER}.md"; } >> "$PKG/README.md"
fi

python3 - "$PKG" "$VER" "$NO_BUILD" <<'PY'
import datetime, hashlib, json, pathlib, subprocess, sys, tomllib, zipfile

pkg, ver, no_build = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
root = pathlib.Path.cwd()
sources = [root / name for name in (
    "Cargo.toml", "Cargo.lock", ".cargo/config.toml", "rust-toolchain.toml",
    "scripts/make-release.sh",
)]
for crate in sorted((root / "crates").iterdir()):
    sources.append(crate / "Cargo.toml")
    if (crate / "build.rs").exists():
        sources.append(crate / "build.rs")
    for directory in ("src", "ui", "assets", "tests"):
        sources += sorted(p for p in (crate / directory).rglob("*") if p.is_file())
sources += sorted(p for p in (root / "vendor").rglob("*") if p.is_file())
notes = root / f"docs/releases/{ver}.md"
if notes.exists():
    sources.append(notes)
lock = tomllib.loads((root / "Cargo.lock").read_text())
manifest = {
    "release": ver,
    "created_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "target": "riscv32imafc-unknown-none-elf",
    "package": "fan-firmware",
    "bin": "fan-control-fw",
    "build_command": "cargo build --locked --release -p fan-firmware --bin fan-control-fw --target riscv32imafc-unknown-none-elf",
    "build_working_directory": ".",
    "reused_elf": no_build == "--no-build",
    "features": [],
    "hardware_verified": False,
    "app_flash_offset": "0x20000",
    "merged_flash_offset": "0x0",
    "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
    "espflash": subprocess.check_output(["espflash", "--version"], text=True).strip(),
    "dependencies": [
        {key: dep[key] for key in ("name", "version", "source")}
        for dep in lock["package"]
        if dep["name"] in ("esp-hal", "esp32s31", "slint", "esp-bootloader-esp-idf")
    ],
    "elf_sha256": hashlib.sha256((pkg / f"fan-control-{ver}.elf").read_bytes()).hexdigest(),
    "source_sha256": {
        str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sources
    },
}
(pkg / "BUILD.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n")
files = sorted(p for p in pkg.iterdir() if p.name != "SHA256SUMS")
(pkg / "SHA256SUMS").write_text("".join(
    f"{hashlib.sha256(p.read_bytes()).hexdigest()}  {p.name}\n" for p in files
))
with zipfile.ZipFile(pathlib.Path(str(pkg) + ".zip"), "x", zipfile.ZIP_DEFLATED) as archive:
    for p in sorted(pkg.iterdir()):
        archive.write(p, f"{pkg.name}/{p.name}")
PY
echo ""
echo "✅ 交付包就绪: $PKG"
ls -la "$PKG"
