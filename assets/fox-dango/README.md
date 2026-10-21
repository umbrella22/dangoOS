# 狐狸团子（fox-dango）角色素材

替换 grok-ball 程序化小球的新形象。目前完成：18 个表情的静态姿势、480 屏量化
预览、眨眼补丁验证和一段设备等效的动画原型。固件还没有接入。

| 目录 | 内容 | 是否入库 |
| --- | --- | --- |
| `source/` | 1254×1254 高清原稿（黑底），19 张：18 个表情 + `idle_blink` | 否（约 21 MB，`.gitignore`） |
| `device/` | 352×336 设备画布，共享 128 色调色板、无抖动、RGB565 精确色 | 是 |
| `preview/` | `expressions_480.png` 全表情屏幕预览；`animation_prototype.gif/.mp4` 动画原型 | 是 |
| `prompts/` | 每张图的生成提示词（`gpt-image-2.5-sunburst`，以 `idle` 为底图做 edit） | 是 |
| `tools/` | `fox_build.py`（量化 + 预览）、`fox_anim.py`（动画原型） | 是 |

重新生成：`python3 assets/fox-dango/tools/fox_build.py && python3 assets/fox-dango/tools/fox_anim.py`
（需要 numpy、pillow、scipy；MP4 需要 ffmpeg）。

## 表情映射

顺序与 `BallExpression` 一致，最后两项是 `docs/ASSET_REQUESTS.md` 里的 P0 / 可选项。

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
- 存储：8 bit 索引 + 字节 RLE，18 个姿势合计约 535 KiB flash（单张最大约 36 KiB），
  外加 128×2 B 调色板。原始 RGB565 是每张 231 KiB。

## 动画方案

分四层，越往下越便宜，绝大多数动作不需要逐帧手绘。

### L0 姿势（静态，每个表情一张）

就是 `device/` 里的 18 张。切换表情 = 换一张 RLE 姿势。

### L1 局部补丁（对齐编辑 + 自动差分）

用底图做“只改某个部位”的编辑，模型基本能保持其余像素不动。
`fox_anim.py` 对原稿做差分、开运算去噪、取最大的两个连通块，自动得到补丁框。
已经验证眨眼：补丁 163×69 px，RGB565 原始约 22 KiB，RLE 后更小。
同样的方法可以做：

- 眨眼（每个睁眼表情一张闭眼补丁，必要时再加半闭帧）
- Speaking 的 3 个嘴型（跟随音量切换）
- Confused 螺旋眼的 2–3 个旋转帧
- Listening / Idle 的耳朵抖动帧
- 视线漂移（瞳孔左右各一帧）

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

### L3 叠加粒子

问号、三个点、汗滴、Zzz、闪电、怒筋现在是画进姿势里的，所以只能跟着整体动。
要让它们独立动，需要再出一版“去掉符号”的姿势，把符号做成小精灵单独叠加
（每个几百字节到几 KiB）。

## 固件接入要点（下一步）

- 新增 `sprite` 渲染路径：RLE 解码 → 128 色调色板查表 → 行/列映射 → 写入
  canvas pool 槽位。没有多边形覆盖率计算，预计比现在约 18 ms 的矢量光栅快
  很多，但要实测。
- `ball_plane` 改成 352×336 的固定窗口。整帧上传约 236 KB，是现在 240×240 的
  2 倍左右，上传时间也会相应变长。呼吸会改动所有行，必要时可以把呼吸降到 10 fps，
  眨眼这类只改局部的帧只传补丁所在的行。
- grok-ball 引擎继续负责状态机、时序和物理参数（pop、跳跃、抖动），
  只把“画什么”从矢量换成精灵。
