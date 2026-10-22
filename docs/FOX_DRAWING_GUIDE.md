# 狐狸团子是怎么画出来的（以及怎么画下一个角色）

这份文档讲的是“角色美术从哪来、拆成什么、固件怎么用”，目的是让另一个会话照着做出
新角色（猫、兔子、别的团子），并且不改或少改固件动画代码。像素级的渲染细节（平滑、
量化、RLE、DMA）见 [FOX_RENDER_PIPELINE.md](FOX_RENDER_PIPELINE.md)，素材目录和清单格式见
[assets/fox-dango/README.md](../assets/fox-dango/README.md)。

## 1. 一句话流程

```text
设定图 ──(图像模型重绘)──▶ idle 主形象 1254x1254 黑底
   └─(以 idle 为底逐张编辑)──▶ 其余 19 个表情
          └─(每个表情再做“只改一处”的编辑)──▶ 变体：眨眼 / 嘴型 / 耳朵 / 视线 / 去符号
fox_build.py  统一几何 + 128 色调色板
fox_parts.py  表情和变体做差 → 补丁、符号精灵、manifest.json
fox_pack.py   → src/assets/fox.bin + src/fox/data.rs（入库，编译不需要 Python）
FoxAnimator + FoxRenderer 在设备上每帧合成
```

关键思路：**不是逐帧手绘动画**。每个表情只有一张静态姿势，动起来靠四层叠加：
换姿势（L0）、局部补丁（L1）、整体形变（L2）、符号精灵（L3）。

## 2. 美术规格（新角色必须遵守）

| 项目 | 要求 | 为什么 |
| --- | --- | --- |
| 原稿尺寸 | 1254×1254 PNG | 管线按这个尺寸读，统一裁切后缩到画布 |
| 背景 | 纯黑 `#000000` | AMOLED 黑色不发光，画布外也是黑，边界看不出来 |
| 构图 | 正面、居中、完整不裁切，约占宽度 75% | 所有表情共用一个裁切框，必须同位置同大小 |
| 风格 | 平涂赛璐璐、清晰深色描边、约 12 种颜色、每色最多一级阴影 | 全部表情共享 128 色调色板；渐变和纹理会出色带和斑点 |
| 禁止 | 纹理、渐变、发光、背景、细小挂件 | 缩到 352 宽后会变噪点 |
| 设备画布 | 352×336，底部对齐，头顶留 24 px | 跳跃、拉伸不出界；面板上原点 (64, 88) |

狐狸团子的设定提示词在 `assets/fox-dango/prompts/idle.txt`：圆润的白色麻薯身体、银白波波头
齐刘海、黑色耳尖的大狐耳、红金双色眼与菱形瞳孔、额头小火焰、金色菱形发卡、两只粉色
圆爪、小 v 嘴。新角色写同样结构的提示词：身份特征 + 风格 + 背景构图 + 去掉的装饰 + 表情。

## 3. 表情：20 张姿势

固件的 `FoxPose` 有 20 项，顺序固定（`fox_build.ORDER`、`data::POSES`、`FoxPose` 三处一致）：

`idle sleep curious happy puzzled confused surprised panic angry satisfied listening
thinking speaking error tired dying annoyed charging peering dazzled`

- `idle` 是主形象，其他 19 张都以它为输入做编辑，提示词以
  “Keep exactly the same character design ... Change only the expression: ...”开头，
  见 `prompts/<表情>.txt`。这样身体、发型、位置基本不动，只换脸和爪子。
- 每个表情要在**形状**上可分，而不是只靠颜色：Angry 有怒筋和虎牙，Annoyed 只是斜眼鼓腮；
  Sleep 全闭眼，Dying 留一条眼缝；Error 是 X 眼加摊平。小屏上远看要一眼能认出。
- 低电量会显示 Tired / Dying，被吵醒会显示 Tired（睡眼惺忪）或 Angry（起床气），所以这两张
  要和 Sleep 有明显区别。

表情含义速查见 [assets/fox-dango/README.md 的表情映射](../assets/fox-dango/README.md#表情映射)。

## 4. 部件：变体图怎么做

变体是“以某张表情为底，只改一处”的编辑，文件名 `source/variants/<表情>__<部位>.png`，
提示词在 `prompts/variants/`，统一以
“Keep this image exactly identical ... pixel for pixel. Change only the ...”开头。

| 部位名 | 类型 | 用途 | 哪些表情有 |
| --- | --- | --- | --- |
| `blink` | 补丁 ×2（两只眼） | 眨眼 | 睁眼的表情 |
| `gaze_left` / `gaze_right` | 补丁 ×2 | 视线漂移、跟随手指 | Idle、Peering |
| `ear_flick` / `ear_turn` | 补丁 ×1 | 抖耳朵 | Idle、Listening |
| `mouth_closed` / `mouth_wide` | 补丁 ×1 | 说话嘴型循环 | Speaking |
| `clean` | 去符号底图 | 把符号从底图里抠出来变成精灵 | Puzzled、Confused、Panic、Angry、Thinking、Annoyed、Charging |
| `zzz`（Sleep 反向） | 加符号 | Sleep 的底图是干净的，变体加上 Zzz | Sleep |

`fox_parts.py` 的做法：变体和底图做差 → 开运算去噪 → 每个部位保留最大的 N 个连通块
（`PATCHES` 字典）→ 外扩 2 px 得到矩形。补丁是不透明矩形，盖在底图同一位置。
符号是 `<表情>` 和 `<表情>__clean` 的差，每个连通块一个带透明的精灵，名字来自
`SYMBOL_NAMES`（question、sweat、anger_mark、dot、sigh、spark、z）。
Confused 的螺旋眼是程序旋转出的 3 帧，不需要画。

编辑时常见问题：模型会顺手改别的像素（发丝、爪子），差分就会多出碎块。先看
`preview/parts_sheet.png`（绿框是补丁，品红是精灵），框不对就重做那张变体。

## 5. 动起来：四层合成

每帧顺序（`fox/animate.rs` 产出 `Scene`，`fox/render.rs` 执行）：

1. 解码姿势底图（RLE → 索引缓冲）。
2. 叠补丁：眨眼、视线、耳朵、嘴型、螺旋帧（`Scene.under`）。
3. 叠 `over: "body"` 的精灵（怒筋、头发上的汗），会跟着身体形变。
4. 整体最近邻形变 `Warp`，锚点在底边中点：呼吸（纵向 ±1.5%，横向反向一半）、
   换表情时的 pop 压扁回弹、Happy 小跳、Panic 抖动、Angry 跺脚、Tired 点头。
5. 叠 `over: "bg"` 的精灵（Zzz、问号、汗滴飞出），各自位移、闪烁，用 4×4 Bayer 抖动淡出。
6. 调色板查表成 RGB565。

另外两个外部输入：开机入场 `fox/entrance.rs`（下落、落地压扁、小跳），以及性格层
`FoxInput`（点击缩放脉冲、抚摸时的倾斜 `offset_x`、跟随手指的 `look_x`）。这些全是
对同一张图做形变，所以**换角色不用改**。

每个表情的动作参数（呼吸周期、眨眼间隔、符号运动）在 `animate.rs`，离线参考实现是
`tools/fox_anim.py`，`fox_parts.py` 的 `HINTS` 也写进了 manifest。先在 Python 版调好出视频，
再同步到 Rust。

## 6. 上屏路径（了解即可）

主页上狐狸是 LVGL 自定义控件 `ui/fox_widget.rs` 的 `FoxWidget`，但设备上不走 LVGL 绘制：
控件把帧渲染进两槽 `CanvasPool`，UI 任务取帧交给 `tasks/plane.rs` 直接 DMA 到面板
(64, 88) 的 352×336 窗口；离开主页时才切换到 LVGL 的整屏渲染。表情由
`behavior/`（affect、personality、runtime）决定，通过 `FoxControl` 下发。

## 7. 画一个新角色：步骤

推荐路线：**保持 20 个表情键和部位名不变**，这样 `animate.rs`、`behavior/` 一行都不用改。

1. 新建 `assets/<角色>/`，结构照抄 `assets/fox-dango/`（`prompts/`、`tools/`、`source/`、
   `device/`、`preview/`）。`source/` 不入库（`.gitignore`），自己保留好原稿。
2. 写 `prompts/idle.txt` 设定提示词，生成主形象，满足第 2 节规格。反复改到满意再往下，
   后面所有图都以它为底。
3. 用 `prompts/<表情>.txt` 逐张生成其余 19 个表情（可以先复制狐狸的提示词，把角色特征词
   换掉）。确认身体位置一致：把 20 张叠起来看轮廓。
4. 按第 4 节的表格生成变体。没有的部位可以不做：对应列表为空，固件就不播那个动作
   （例如没有 `gaze_left` 就不会转眼珠）。
5. 改工具里的路径：三个脚本都用 `ROOT = Path(__file__).parent.parent`，复制到新目录后
   自动指向新角色；`fox_pack.py` 的 `CRATE_SRC`/`OUT`/`BLOB_OUT` 决定输出到哪里。
6. 依次运行：

   ```sh
   python3 assets/<角色>/tools/fox_parts.py   # device/ + preview/parts_sheet.png
   python3 assets/<角色>/tools/fox_build.py   # preview/expressions_480.png
   python3 assets/<角色>/tools/fox_anim.py    # preview/animation_parts.mp4
   python3 assets/<角色>/tools/fox_pack.py    # src/assets/*.bin + data.rs
   ```

   需要 numpy、pillow、scipy，视频需要 ffmpeg。
7. 看预览：`expressions_480.png` 有没有白点、色带、裁切；`parts_sheet.png` 框对不对；
   动画视频里眨眼和符号位置对不对。
8. 接入固件，两种方式：
   - 直接替换：`fox_pack.py` 输出覆盖 `src/fox/data.rs` 和 `src/assets/fox.bin`，最简单，
     适合先上机看效果。
   - 并存可切换：把 `data` 模块按角色分开（例如 `fox/data_cat.rs` + `cat.bin`），给
     `FoxPose::data()` 和 `render.rs` 用的 `PALETTE` / `BLOB` 加一层“当前角色”选择。每个角色
     自带 128 色调色板，渲染器本身不用改。注意 flash：狐狸 20 张底图 + 79 个部件约 722 KiB。
9. `cargo test -p mosaico-s31 --lib`（含渲染器的 host 测试），再按
   [DEVELOPMENT.md](DEVELOPMENT.md) 只烧 main_app。
10. 上机看串口遥测 `anim= ras=`：渲染时间应在 10–15 ms，明显变高说明部件太大或太多。

## 8. 经验

- 先定主形象再做表情，先定表情再做变体。返工主形象等于全部重来。
- 编辑提示词一定要写“pixel for pixel”“Nothing else changes”，并点名要保持的部位。
- 颜色越少越干净：狐狸约 12 种基础色，128 色调色板里大部分给了皮肤和头发的过渡。
- 不要依赖颜色区分表情，AMOLED 小屏远看主要靠形状。
- 符号尽量画在黑底上（`over: "bg"`），可以自由运动；压在身体上的只能跟着身体动。
