# Lua 应用

正式应用按功能组织，目录名与 `launcher.json` 的 `id` 一致，入口统一为
`scripts/main.lua`。ID 在所有分类中唯一。五个默认 Lua 应用都通过 system 镜像交付。

| 分类 | 应用 | ID |
| --- | --- | --- |
| games | 贪吃蛇 | snake |
| games | 2048 | 2048 |
| games | Dino | dino |
| games | Flappy Bird | flappy |
| tools | Photo | photo |

设备安装路径为 `/system/apps/<id>`，用户数据根为 `/data/appdata/<id>`。USB 下载更新
位于 `/data/apps/<id>`，同 ID 下载版优先；移除下载版后恢复 system 版本。分类只影响
仓库目录，不改变设备路径。固件中的 Rust 原生应用与文件系统中的 Lua 应用分别维护。

每个应用包含启动清单、脚本、资源、README 和许可证。Dino、Flappy 的 `SOURCE.json`
记录官方 v0.6.0 镜像和原始文件哈希。Snake、2048 来自 `espressif/esp-claw-skills-lab`
的 `57969b1e5891b5ad68e3d2433f1e9dde3ab1fcc0`（MIT），脚本和图标保持上游内容。
Photo 的上游来源与维护信息记录在自己的 `SOURCE.json` 中。

```sh
cargo run -p app-pack --offline -- check apps
cargo run -p app-pack --offline -- system apps target/system-release --size 3072000
python3 scripts/device/console.py push apps/games/dino --run --follow
```

system 镜像从当前应用集全新生成，README、许可证和来源记录作为 `application-docs/`
随镜像交付。纯 Lua 应用修改可以单独打包或下载更新；新增引擎能力需要同时更新固件。

开发与运行约定见 [Lua 应用开发](../docs/guides/lua-apps.md)，镜像格式见
[app-pack](../tools/app-pack/README.md)。开发示例位于 `examples/apps`，不进入默认应用集。
