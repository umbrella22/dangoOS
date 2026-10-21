# 素材需求清单（ASSET_REQUESTS）

本文件汇总固件当前**用占位表情顶替**、以及**后续要新增**的美术素材。
按「你做素材 → 我转码 → 直接烧录」的分工组织：每一项都写清了文件格式、
画布尺寸、锚点和对齐规则，转码工具见
[`crates/grok-ball/tools/sprite_pack.py`](../crates/grok-ball/tools/sprite_pack.py)。

> 优先级：**P0** = 现在就有明显代入感损失；**P1** = 影响角色可替换路线；
> **P2** = 锦上添花。

---

## 0. 通用规格（所有素材都适用）

| 项 | 要求 |
| --- | --- |
| 文件格式 | **WebP 动图**或 **GIF**（首选 WebP：同样的观感体积小 40%）；也接受 PNG 序列 |
| 画布 | `480x480` 或 `240x240`，**正方形**，四周留白不超过 4% |
| 背景 | 透明（alpha）。转码时以纯黑合成，黑边即屏幕底色 |
| 帧率 | 12–20 fps；超过 20 fps 的帧会被丢弃（单帧光栅预算 ~13 ms） |
| 循环 | 表情类**必须循环**；一次性动作用「播完停在末帧」并在文件名标注 `once` |
| 命名 | `<角色>-<表情>-<变体>.webp`，例如 `blob-struggle-v1.webp`、`cat-ears.webp` |
| 色彩 | 单帧颜色数建议 ≤ 4096；渐变用有序抖动，避免大面积平滑渐变（RGB565 会有色带） |
| 时长 | 单帧态 0.6–1.2 s；循环态 1.0–2.5 s |

**给了 WebP/GIF 之后我做的事**：`ffmpeg` 解帧 → 裁成正方形 → 缩放到目标
画布 → alpha 合成黑底 → RGB565 little-endian → 生成 Rust 常量表（含帧数、
每帧延时、锚点），并加进 `crates/grok-ball/src/sprite.rs` 与一个编译期测试。
你不需要自己切帧、压色或算字节序。

---

## 1. P0：当前用别的表情顶替的动作

拖拽/揪住/松手功能已按产品决定删除，因此不再需要 struggle / grab /
release-dizzy / release-startled 素材。剩下的 P0 是点按和电量表情：

| 素材 | 现在借用 | 需要的样子 | 规格 |
| --- | --- | --- | --- |
| **annoyed**（被点三四下） | `Confused` | 比 `Angry` 轻：斜眼、身体侧过去一点、轻微叹气式下沉；**不能和 angry 混淆** | 循环，8–12 帧，`blob-annoyed-loop.webp` |

---

## 2. P0：电量相关的两个表情

| 素材 | 现在借用 | 需要的样子 | 规格 |
| --- | --- | --- | --- |
| **tired**（≤15%，低电） | 上游 seed 15「疲惫」 | 眼皮下垂、呼吸变慢、偶尔点头打瞌睡 | 循环，10–16 帧，`blob-tired-loop.webp` |
| **dying**（≤5%，濒死） | 上游 seed 6「休眠」 | 眼睛半闭到几乎闭合、身体塌下去、呼吸幅度很小；不要真的闭眼（闭眼是 `Sleep`） | 循环，8–12 帧，`blob-dying-loop.webp` |
| **charging**（可选） | 无 | 插电时：眼睛向上看、身体微微发亮 | 循环，8 帧，`blob-charging-loop.webp` |

> 阈值实现在 `crates/grok-ball/src/affect.rs`：进入 15% / 5%，离开 18% / 8%
> （3 个百分点的迟滞，防止电量在阈值上抖动时表情反复横跳）。

---

## 3. P1：角色可替换（猫耳球 / 狐狸耳球 / 任意角色）

现在已经有**程序化皮肤**：`crates/grok-ball/src/skin.rs` 里 `CAT`、`FOX`
用三角耳多边形画，和 16 个表情完全兼容，不需要任何图片素材。
下面这些是「程序化画不出来、必须给图」的部分。

### 3.1 耳朵/装饰贴图（推荐做法）

| 素材 | 说明 | 规格 |
| --- | --- | --- |
| `cat-ears.webp` | 猫耳，**与身体同色系**，内侧粉色；耳朵根部要覆盖到头部轮廓内 | 只画耳朵，透明背景，画布 240x240，头部中心对齐画布中心；`Behind` 层 |
| `fox-ears.webp` | 狐狸耳，更高更尖，外侧橙、内侧奶油 | 同上 |
| `cat-tail.webp` | 可选：甩动的尾巴，出现在身体右下 | 循环，8–12 帧，`Front` 层 |
| `fox-tail.webp` | 可选：大尾巴 | 循环，8–12 帧 |

**关键约定**：贴图的画布中心和球体头部中心重合，球体直径约画布 88%。
这样我能用同一套 `CanvasPose`（出场下落压扁、缩放）直接叠加，不需要为每个
角色重做动画。

### 3.2 整套角色（如果耳朵贴图不够）

如果以后要做「完全不同的角色」（不是球体加装饰），需要：

| 素材 | 说明 | 规格 |
| --- | --- | --- |
| `<角色>-body.webp` | 身体轮廓（替代 blob），单色或两色 | 静态一帧，240x240，中心对齐 |
| `<角色>-eyes-atlas.webp` | **一张图集**：横向排列 16 个表情的眼睛，每个格子 120x120 | 静态，透明背景，**格子顺序必须是** `Idle, Sleep, Curious, Happy, Puzzled, Confused, Surprised, Panic, Angry, Satisfied, Listening, Thinking, Speaking, Error, Tired, Dying` |
| 说明 | 图集方式能让 16 个表情复用一套眨眼/视线代码 | 图集总尺寸 1920x120，或 2 行 x 8 列 960x240 |

---

## 4. P1：官方点阵字体（启动页 `ikaros` 用）

现在的 `ikaros` 是手写的 5x7 点阵字体
（`crates/mosaico-s31/src/splash_font.rs`），刻意贴近官方的点阵观感，
但**不是官方的同一套字形**。官方 splash 的字体在 bootloader 的
`splash` blob 里是**位图**，不是字体文件。

| 素材 | 说明 | 规格 |
| --- | --- | --- |
| `mosaico-splash-font.png` | 官方 splash 的原始位图（从 `mosaico_boot_splash.bin` 提取，或对屏幕拍照后二值化） | PNG，1:1 像素，黑色背景白色字 |
| 可选：`ikaros-atlas.png` | 如果希望我按官方字形重画 `ikaros` | 同上，6 个字母横向排列 |

给了之后我只需要替换 `splash_font.rs` 里的 `glyph()` 表，动画和上屏逻辑不用动。

---

## 5. P2：其他建议（非必须）

| 素材 | 用途 | 规格 |
| --- | --- | --- |
| `boot-blank.png` | 如果希望启动页不是纯黑而是极暗纹理 | 480x480，亮度 ≤ 8/255 |
| `battery-icons.png` | 极简电量角标（当前没有画电量 UI） | 4 档，每档 32x16 |
| `icon-*.png` | 后续 Hub/Apps 页 | 视页面而定 |

---

## 6. 转码命令（素材到位后）

```bash
# 单张/动图 -> Rust 常量表
python3 crates/grok-ball/tools/sprite_pack.py \
    --input ~/Downloads/blob-struggle-loop.webp \
    --name struggle \
    --size 240 \
    --out crates/grok-ball/src/sprite/struggle.rs

# 看一眼转码结果（输出每帧的 PPM，方便检查透明边缘与压色）
python3 crates/grok-ball/tools/sprite_pack.py \
    --input ~/Downloads/blob-struggle-loop.webp \
    --name struggle --size 240 --preview target/sprite-preview
```

工具会打印：帧数、帧延时、每帧不透明像素数、占用 flash 字节数。
一份 12 帧 240x240 的素材约占 **1.35 MiB** flash（240×240×2×12），
所以如果角色素材很多，建议先用 180x180 或 120x120 画布，我再放大上屏。

---

## 7. 已经不需要素材的部分

以下都已实现，**不用做图**：

- 16 个表情（Idle/Sleep/Curious/Happy/Puzzled/Confused/Surprised/Panic/
  Angry/Satisfied/Listening/Thinking/Speaking/Error/Tired/Dying）——全部来自
  上游 MIT 数据，程序化矢量渲染，任意画布尺寸都清晰；
- 眨眼、呼吸、视线漂移、表情池轮换；
- 点按阶梯（1/2/3/5 下）、长按揪住、拖动跟随、松手反应；
- 启动页 `ikaros`（手写点阵）、下落 + 落地压扁 + 回弹的出场动画；
- 猫耳/狐狸耳的程序化皮肤；
- 电量阈值与 `Tired`/`Dying` 表情切换逻辑。

---

## 8. 附：表情/种子对照表

方便对素材命名时对齐上游编号。

| 我们的表情 | 上游 seed | 上游名字 | 备注 |
| --- | --- | --- | --- |
| `Sleep` | `00` | 睡眠 | |
| `Idle` | `02` | 待机放空 | |
| `Curious` | `03` | 好奇 | |
| `Dying` | `06` | 休眠 | 电量 ≤5% |
| `Happy` | `10` | 开心 | |
| `Puzzled` | `11` | 疑惑 | |
| `Surprised` | `13` | 惊讶 | |
| `Tired` | `15` | 疲惫 | 电量 ≤15% |
| `Panic` | `17` | 慌张 | 被揪住时顶替 `struggle` |
| `Satisfied` | `19` | 满意 | |
| `Confused` | `20` | 困惑 | 顶替 `annoyed`/`dizzy` |
| `Angry` | `21` | 生气 | |
| `Thinking` | `30` | 思考中 | |
| `Error` | `34` | 出错 | |
| `Listening` | `35` | 等待输入 | |
| `Speaking` | `39` | 输出回复 | |

上游还有 16 个没接入的表情（`01` 唤醒、`04` 发呆、`05` 加载苏醒、`07` 抖动唤醒、
`12` 失落、`14` 害羞、`16` 专注、`18` 无奈、`31` 接收任务、`32` 处理中忙碌、
`33` 任务完成、`36` 联网加载、`37` 复述回忆、`38` 拒绝/受限、`40` 检索资料、
`41` 停止终止）。它们的数据已经在 `data.rs` 里，随时可以接入，不需要新素材。
