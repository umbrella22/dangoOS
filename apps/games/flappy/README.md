# Flappy Bird

应用 ID 为 `flappy`，入口为 `scripts/main.lua`。

应用提取自官方 ESP-Mosaico-Claw v0.6.0 system 镜像。`SOURCE.json` 记录输入镜像
及原始文件的哈希。Lua 脚本和图标保留提取时的字节，manifest 使用本仓库的应用 ID
与入口路径。

应用使用引擎提供的显示、延时和输入模块，默认使用触摸输入。音频为可选能力，
取决于宿主模块。上游采用 Apache-2.0 许可证，详见 `LICENSE`。

从仓库根目录更新并运行：

```sh
python3 scripts/device/console.py push apps/games/flappy --run
```

默认应用通过 `app-pack system` 生成 system 镜像，见 [应用总览](../../README.md)。
