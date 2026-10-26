# Lua 存储模块

通过 `local storage = require("storage")` 访问文件系统。此处为上游 API 的本地中文译文；ESP-Mosaico 为每个前台应用提供 `/data/appdata/<id>` 根目录，生命周期规则见[应用生命周期](../../../../../docs/architecture/app-lifecycle.md)。

## API

| 函数 | 行为 / 返回值 |
| --- | --- |
| `get_root_dir()` | 返回可写 DATA 根目录 |
| `join_path(...)` | 拼接路径字符串，忽略空分量和分量间重复分隔符；不解析 `..`，不创建目录 |
| `exists(path)` | 返回路径是否存在 |
| `stat(path)` | 返回 `{ type, size, mtime, mode }`，失败返回 `nil, err, detail` |
| `mkdir(path)` | 创建目录，成功返回 `true` |
| `write_file(path, content)` | 写文件内容，成功返回 `true` |
| `read_file(path)` | 以字符串返回文件内容 |
| `listdir(path)` | 返回含 `name`、`type` 的条目，可用时还含 `size`、`mtime`、`mode` |
| `remove(path)` | 删除文件或空目录，成功返回 `true` |
| `remove_tree(path)` | 递归删除 DATA 子路径，删除成功或原本不存在返回 `true`，失败返回 `nil, detail` |
| `rename(old_path, new_path)` | 重命名或移动路径，成功返回 `true` |
| `get_free_space()` | 返回 DATA 存储的 `{ total, free, used }`，单位字节 |

## 删除与错误

- `remove()` 失败时抛出 Lua 错误，包括路径不存在和非空目录；可用 `pcall(storage.remove, path)` 处理。
- `remove_tree()` 只接受 DATA 后代的绝对路径，拒绝 DATA 根本身、`.`/`..`、重复分隔符和尾部分隔符。路径必须短于 1024 字节，目录深度最多 16 层，含目标目录。
- 递归删除失败时可能只完成部分删除，不能并发修改目标树。
- `stat()`、`remove_tree()` 的 `detail` 包含 `code`、`errno`、`operation`、`path`、`message`；非法参数也可能直接抛出 Lua 错误。

## 示例

```lua
local storage = require("storage")

local dir = storage.join_path(storage.get_root_dir(), "demo")
local file = storage.join_path(dir, "test.txt")

storage.mkdir(dir)
storage.write_file(file, "hello")
print(storage.read_file(file))
storage.remove(file)
storage.remove(dir)
```
