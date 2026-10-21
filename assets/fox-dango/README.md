# 狐狸团子（fox-dango）角色素材

固件的角色形象（取代了早期的 grok-ball 矢量小球）。18 个表情的静态姿势、14 个表情的
局部补丁（眨眼 / 嘴型 / 耳朵 / 视线 / 螺旋眼）、8 个表情的独立符号精灵，以及描述
这一切的 `device/manifest.json`。固件已接入：`crates/mosaico-s31/src/fox/`。
整条管线的讲解见 [docs/FOX_RENDER_PIPELINE.md](../../docs/FOX_RENDER_PIPELINE.md)。

| 目录 | 内容 | 是否入库 |
| --- | --- | --- |
| `source/` | 1254×1254 高清原稿（黑底）：18 个表情 + `variants/` 下 26 张对齐编辑（`<表情>__<部位>.png`） | 否（`.gitignore`） |
| `device/poses/` | 18 张 352×336 底图。带符号的表情这里放的是去掉符号的干净版 | 是 |
| `device/parts/<表情>/` | 补丁和符号精灵（RGBA，透明处 alpha=0） | 是 |
| `device/manifest.json` | 画布、128 色 RGB565 调色板、每个部件的类型和位置、动画参数 | 是 |
| `preview/` | `expressions_480.png` 全表情（含符号）；`parts_sheet.png` 部件框（绿=补丁，品红=精灵）；`animation_parts.mp4/.gif` 九宫格动画 | 是 |
| `prompts/` | 生成提示词：姿势在根目录，对齐编辑在 `variants/` | 是 |
| `tools/` | `fox_build.py`（几何 / 调色板 / 全表情预览）、`fox_parts.py`（差分出部件，写 `device/`）、`fox_anim.py`（只用 `device/` 合成动画） | 是 |

重新生成：

```sh
python3 assets/fox-dango/tools/fox_parts.py   # device/ + preview/parts_sheet.png
python3 assets/fox-dango/tools/fox_build.py   # preview/expressions_480.png
python3 assets/fox-dango/tools/fox_anim.py    # preview/animation_parts.*
```

需要 numpy、pillow、scipy；MP4 需要 ffmpeg。所有 PNG 都只用调色板里的 128 种颜色，
转换脚本按 RGB565 值查回索引即可，不会有未命中。

## 表情映射

顺序与 `behavior::Expression` 一致，最后两项（annoyed、charging）是额外姿势。

| 文件 | 表情 | 说明 |
| --- | --- | --- |
| `idle` | Idle | 主形象，其他表情都以它为底图编辑 |
| `sleep` | Sleep | 闭眼、耳朵下垂、身体压低 |
| `curious` | Curious | 歪头、单耳竖起 |
| `happy` | Happy | ^ ^ 眼、腮红、耳朵竖起 |
| `puzzled` | Puzzled | 挑眉侧视、带问号 |
| `confused` | Confused | 螺旋眼、波浪嘴、汗滴 |
| `surprised` | Surprised | 瞳孔缩小、O 嘴、爪子抬到脸边 |
| `panic` | Panic | 含泪、汗滴飞出、爪子乱挥 |
| `angry` | Angry | 下压眉、小虎牙、怒筋 |
| `satisfied` | Satisfied | 眯眼微笑 |
| `listening` | Listening | 双耳前竖、抬眼 |
| `thinking` | Thinking | 托腮、头顶三点 |
| `speaking` | Speaking | 张嘴说话、抬爪 |
| `error` | Error | X 眼 + 摊平。和 Angry 在形状上彻底分开，不再只靠红色区分 |
| `tired` | Tired | 眼皮半垂、黑眼圈（电量 ≤15%） |
| `dying` | Dying | 只剩一条眼缝、身体塌平；不完全闭眼，和 Sleep 区分（电量 ≤5%） |
| `annoyed` | Annoyed（P0） | 斜眼、鼓腮、叹气团；没有怒筋和虎牙，和 Angry 区分 |
| `charging` | Charging（可选） | 抬眼、头顶闪电、火花 |

## 设备规格

- 画布 352×336，底部对齐，头顶留 24 px 给跳跃和拉伸；放在 480 屏中心偏下 16 px。
- 全部表情共享一张 128 色调色板，不抖动。64 色会丢掉皮肤的粉调；
  Floyd–Steinberg 抖动在头发上会产生颗粒，而且缩放时颗粒会闪。
  平涂风格不抖动反而最干净。
- 存储：8 bit 索引 + 字节 RLE（每行独立、(长度, 索引) 两字节一段），
  18 张底图约 569 KiB（单张最大约 40 KiB），73 个部件约 88 KiB，合计约 660 KiB flash，
  外加 128×2 B 调色板。精灵用索引 255 表示透明。原始 RGB565 是每张 231 KiB。

## manifest 格式

```jsonc
{
  "canvas": [352, 336],
  "palette_rgb565": [/* 128 项 */],
  "poses": {
    "charging": {
      "file": "poses/charging.png", "label": "充电 Charging",
      "hints": { "breath_ms": 3000, "blink": [2500, 6000], "symbols": "spark[0] (bolt): bob 4 px; ..." },
      "parts": {
        "blink": [{ "kind": "patch",  "file": "parts/charging/blink_0.png", "x": 93, "y": 214, "w": 68, "h": 64 }, ...],
        "spark": [{ "kind": "sprite", "over": "bg", "file": "parts/charging/spark_0.png", "x": 152, "y": 28, "w": 48, "h": 70 }, ...]
      }
    }
  }
}
```

- `patch`：不透明矩形，直接覆盖底图同一位置。
- `sprite`：带透明的符号。`over: "bg"` 在黑底上，可以随意移动、闪烁、淡出；
  `over: "body"` 压在角色身上（怒筋、头发上的汗滴），底图下面是干净的，所以缩放 / 移开也不会露馅。
- `frame`：只有 Confused 的螺旋眼有，1–3 是旋转帧，0 就是底图本身。

## 部件清单

| 表情 | 补丁 | 符号精灵 |
| --- | --- | --- |
| Idle | blink ×2、gaze_left ×2、gaze_right ×2、ear_flick | — |
| Sleep | — | z ×3（上浮淡出） |
| Curious / Surprised / Tired | blink ×2 | — |
| Puzzled | blink ×2 | question（上下浮动） |
| Confused | spiral 3 帧 × 2 眼 | sweat ×3（闪烁） |
| Panic | blink ×2 | sweat ×6（向外飞出）、tremble ×6（爪边抖动线） |
| Angry | blink ×2 | anger_mark（放大缩小脉动） |
| Listening | blink ×2、ear_turn | — |
| Thinking | blink ×2 | dot ×3（依次亮起） |
| Speaking | blink ×2、mouth_closed、mouth_wide | — |
| Annoyed | blink ×2 | sigh（飘出淡出） |
| Charging | blink ×2 | spark ×11，`spark_0` 是闪电（浮动），其余是火花（闪烁） |
| Happy / Satisfied / Error / Dying | — | —（眼睛本来就是闭的或不需要眨） |

## 动画方案

分四层，越往下越便宜，绝大多数动作不需要逐帧手绘。

### L0 姿势（静态，每个表情一张）

就是 `device/poses/` 里的 18 张。切换表情 = 换一张 RLE 姿势。

### L1 局部补丁（对齐编辑 + 自动差分）

用底图做“只改某个部位”的编辑，模型基本能保持其余像素不动。
`fox_parts.py` 对原稿做差分、开运算去噪、按部位保留最大的 1–2 个连通块，自动得到补丁框，
清单见上面的“部件清单”。Confused 的螺旋眼是程序旋转出来的 3 帧（最近邻，颜色不变）。

### L2 程序化形变（整张姿势，零素材）

都是以底边中点为锚的最近邻行/列映射，每帧只建两张索引表。原型里用了这些：

| 动作 | 参数 | 用在 |
| --- | --- | --- |
| 呼吸 | 纵向 ±1.5%、横向反向一半，周期 3 s | 大多数状态 |
| 切换 pop | 进入时先压扁 12%，在最扁处换姿势，阻尼弹簧回弹（约 0.4 s） | 所有切换，掩盖换图的跳变 |
| 小跳 | 18 px 抛物线 0.4 s，落地压扁 8% | Happy |
| 抖动 | 横向 ±3 px、11 Hz，呼吸加快到 0.6 s | Panic |

每个表情的设定：

| 表情 | 动作 |
| --- | --- |
| Idle | 呼吸 3 s；2.5–6 s 随机眨眼；偶尔视线漂移 |
| Sleep | 呼吸 4.5 s、幅度 2.5%；Zzz 上浮 |
| Curious / Listening | 呼吸；耳朵偶尔抖一下 |
| Happy | pop + 2–3 次小跳 |
| Puzzled / Thinking | 慢呼吸；问号或三个点浮动 / 依次亮起 |
| Confused | 螺旋眼轮换；左右轻摆 2 px |
| Surprised | 强 pop（先拉长 8% 再回弹） |
| Panic | 抖动 + 快呼吸；汗滴飞出 |
| Angry | 每 1.5 s 一次“跺脚”压扁 + 2 px 抖动 |
| Satisfied | 慢呼吸 + 轻微左右摇 |
| Speaking | 嘴型补丁循环 |
| Error | 落下压扁后静止，每 2 s 轻抖一次 |
| Tired | 慢呼吸；每 4 s 点头（下沉 6 px 再回） |
| Dying | 6 s 极慢呼吸，幅度 0.8% |
| Annoyed | 一次叹气压扁 + 气团上浮 |
| Charging | 闪电上下浮动、火花闪烁 |

### L3 符号精灵

带符号的表情各出了一版“去掉符号”的干净底图（`<表情>__clean`），Sleep 反过来出了一版
“加上 Zzz”的。两者差分就是符号本身，每个连通块一个精灵，几百字节到 2 KiB。
淡出用 4×4 Bayer 点阵丢像素，不做 alpha 混合，索引渲染每像素一次比较就够。

### 每帧合成顺序（固件照这个做）

1. 解码底图 RLE 到工作缓冲（索引）
2. 叠 `patch`（眨眼 / 嘴型 / 耳朵 / 视线 / 螺旋帧）
3. 叠 `over: "body"` 的精灵（会跟着身体一起形变）
4. 整体最近邻形变（呼吸、pop、跺脚、抖动）
5. 叠 `over: "bg"` 的精灵（各自位移、闪烁、淡出，不跟着形变）
6. 调色板查表成 RGB565 送屏

`tools/fox_anim.py` 就是这个流程的参考实现，`preview/animation_parts.mp4` 是它的输出。

## 固件实现

- `fox/render.rs`：RLE 解码 → 索引缓冲 → 最近邻形变 → 128 色调色板查表，写入
  canvas pool 槽位。真机约 10 ms/帧（带符号约 15 ms），比原矢量光栅（约 19 ms）快。
- `tasks/plane.rs`：352×336 固定窗口，面板原点 (64, 88)，每帧 11 块 32 行 DMA。
- `behavior/` 负责状态机与时序，`fox/animate.rs` 负责“怎么动”，只把画什么换成了精灵。
