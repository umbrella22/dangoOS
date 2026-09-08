# EVE HUD → ESP32-S31 嵌入式移植方案

## 🎯 目标

将 **EVE Online 风格科幻全息 HUD** (Vue + Three.js) 移植到 **ESP32-S31 + Slint** 嵌入式设备

**源项目**: Vue 3.5 + TypeScript + Three.js (26k 行, 6.8MB)  
**目标平台**: ESP32-S31 + 480×480 AMOLED + Slint 1.17 + Rust no_std

---

## ⚠️ 核心挑战分析

### 硬件约束
| 资源 | ESP32-S31 | EVE HUD 需求 | 差距 |
|------|-----------|--------------|------|
| RAM | 74KB 内部 + 16MB PSRAM | ~100MB (Chrome) | ⚠️ 巨大 |
| Flash | 16MB | 6.8MB 构建产物 | ⚠️ 勉强 |
| CPU | 双核 320MHz RISC-V | 现代桌面 CPU | ⚠️ 巨大 |
| GPU | 软件渲染 | WebGL/Three.js | ❌ 不可用 |
| 分辨率 | 480×480 | 任意 | ✅ 可适配 |

### 技术栈鸿沟
| 层次 | Web | 嵌入式 | 兼容性 |
|------|-----|--------|--------|
| 运行时 | V8/SpiderMonkey | 无 JS 引擎 | ❌ 完全不同 |
| UI 框架 | Vue 3 响应式 | Slint 声明式 | ⚠️ 需重写 |
| 3D 渲染 | Three.js (WebGL) | 无 | ❌ 无法移植 |
| 状态管理 | reactive + computed | Rust + Slint 属性 | ⚠️ 需重写 |
| 动画 | CSS transition/animation | Slint 动画 | ⚠️ 语法不同 |

---

## 💡 可行性评估

### ❌ 完全不可行的部分
1. **Three.js 3D 场景** - 无 WebGL，无 GPU，CPU 算力不足
2. **原样移植 Vue 代码** - 无 JS 引擎
3. **复杂粒子系统** - 性能不足
4. **动态模糊/辉光特效** - 软件渲染无法承受

### ⚠️ 需大幅简化的部分
1. **窗口拖拽系统** - 可实现但需简化
2. **复杂动画** - 减少并发动画数量
3. **启动序列** - 大幅削减特效
4. **聊天/日志系统** - 简化滚动和渲染

### ✅ 可以实现的部分
1. **EVE 配色和视觉风格** - 青金石蓝 + 深空黑
2. **基础 HUD 布局** - 导航/目标/状态面板
3. **静态背景** - 预渲染星空图片
4. **简单动画** - 扫描线、数值变化
5. **交互逻辑** - 按钮、滑杆、状态切换

---

## 🎨 移植策略

### 策略 A：视觉风格移植（推荐）⭐
**思路**: 保留 EVE 视觉语言，用 Slint 重新实现简化版

**保留要素**:
- ✅ 配色方案（青金石蓝 #5fd0e0 + 深空黑 #02060a）
- ✅ 字体风格（Rajdhani → Slint 内置粗体）
- ✅ 面板样式（半透明、青色边框、角标）
- ✅ 扫描线特效（简化版）
- ✅ 数值动画（计数器、进度条）

**简化要素**:
- ⚠️ 3D 场景 → 静态星空背景图
- ⚠️ 复杂启动序列 → 简化 3-5 步动画
- ⚠️ 多窗口拖拽 → 固定布局
- ⚠️ 粒子系统 → 移除

**实现难度**: 中等  
**开发周期**: 2-3 周  
**最终效果**: EVE 风格的简化 HUD

---

### 策略 B：混合方案（高级）
**思路**: ESP32 跑核心逻辑，手机/PC 跑完整 Web UI

**架构**:
```
┌─────────────────┐         WebSocket/BLE          ┌──────────────┐
│  ESP32-S31      │  ←──────────────────────────→  │  Web Browser │
│  - 游戏逻辑     │                                │  - 完整 HUD  │
│  - 简化 UI      │                                │  - Three.js  │
│  - 触摸输入     │                                │  - 全特效    │
└─────────────────┘                                └──────────────┘
```

**优势**:
- ✅ 保留完整 Web 体验
- ✅ ESP32 专注核心逻辑
- ✅ 可选单机模式（简化 UI）

**劣势**:
- ⚠️ 需要额外设备
- ⚠️ 网络延迟问题
- ⚠️ 架构复杂度高

**实现难度**: 高  
**开发周期**: 4-6 周

---

### 策略 C：概念演示（快速）
**思路**: 只移植启动序列 + 单个面板

**范围**:
- ✅ BootSequence 启动动画（简化版）
- ✅ 单个 HUD 面板（如目标面板）
- ✅ EVE 配色和字体
- ❌ 不包含游戏逻辑

**实现难度**: 低  
**开发周期**: 3-5 天  
**最终效果**: 技术演示 Demo

---

## 🚀 推荐方案：视觉风格移植

### 第一阶段：核心布局（1 周）

#### 1.1 设计令牌迁移
```rust
// src/theme.rs
pub mod eve_theme {
    use slint::Color;
    
    // 配色
    pub const BG: Color = Color::from_rgb_u8(2, 6, 10);
    pub const PANEL: Color = Color::from_argb_u8(200, 7, 17, 24);
    pub const LINE: Color = Color::from_argb_u8(56, 110, 190, 210);
    pub const CYAN: Color = Color::from_rgb_u8(95, 208, 224);
    pub const TEXT: Color = Color::from_rgb_u8(194, 219, 228);
    pub const DIM: Color = Color::from_rgb_u8(104, 131, 143);
    
    // 尺寸
    pub const PANEL_BORDER: f32 = 1.0;
    pub const CORNER_SIZE: f32 = 14.0;
    pub const GAP: f32 = 12.0;
}
```

#### 1.2 基础组件库
```slint
// ui/components/eve_panel.slint
export component EvePanel inherits Rectangle {
    in property <string> title: "PANEL";
    in property <bool> active: false;
    
    background: #07111878; // rgba(7, 17, 24, 0.78)
    border-width: 1px;
    border-color: active ? #5fd0e0 : #6ebdd266;
    
    // 角标装饰
    Rectangle {
        x: -1px; y: -1px;
        width: 14px; height: 1px;
        background: #6ebdd26e;
    }
    // ... 其他三个角
    
    VerticalLayout {
        padding: 12px;
        spacing: 8px;
        
        // 标题栏
        HorizontalLayout {
            height: 20px;
            Text {
                text: root.title;
                color: #5fd0e0;
                font-size: 13px;
                font-weight: 600;
            }
        }
        
        // 内容区
        @children
    }
}
```

#### 1.3 简化的 HUD 布局
```slint
// ui/hud_layout.slint
import { EvePanel } from "components/eve_panel.slint";

export component HudLayout inherits Window {
    width: 480px;
    height: 480px;
    background: #02060a;
    
    // 静态星空背景
    Image {
        source: @image-url("assets/starfield.png");
        width: 100%; height: 100%;
    }
    
    // 扫描线特效（简化）
    Rectangle {
        width: 100%; height: 2px;
        background: #8cdceb0d;
        y: scanline-y;
        
        property <length> scanline-y: 0px;
        animate scanline-y {
            duration: 5s;
            iteration-count: -1;
            easing: linear;
        }
        // 在 Rust 中驱动
    }
    
    // 左侧导航
    EvePanel {
        x: 10px; y: 10px;
        width: 46px; height: 400px;
        title: "NAV";
        // 导航图标
    }
    
    // 顶部状态
    EvePanel {
        x: 70px; y: 10px;
        width: 400px; height: 32px;
        title: "STATUS";
        // 状态信息
    }
    
    // 右侧目标
    EvePanel {
        x: 424px; y: 50px;
        width: 340px; height: 200px;
        title: "TARGET";
        active: true;
        // 目标信息
    }
    
    // 底部控制台
    ShipConsole {
        x: 90px; y: 370px;
        width: 300px; height: 100px;
    }
}
```

---

### 第二阶段：动态元素（3-5 天）

#### 2.1 状态指示器
```slint
// ui/components/status_bar.slint
export component StatusBar inherits Rectangle {
    in property <float> hp: 100.0;
    in property <float> shield: 100.0;
    
    height: 32px;
    background: transparent;
    
    HorizontalLayout {
        spacing: 16px;
        
        // 护盾
        HorizontalLayout {
            Text { text: "SHIELD"; color: #5fd0e0; }
            Rectangle {
                width: 80px; height: 6px;
                background: #1a202966;
                Rectangle {
                    width: parent.width * shield / 100;
                    height: 100%;
                    background: #5fd0e0;
                }
            }
            Text { 
                text: Math.round(shield) + "%"; 
                color: #c2dbe4;
            }
        }
        
        // 装甲
        HorizontalLayout {
            Text { text: "ARMOR"; color: #e8c25a; }
            Rectangle {
                width: 80px; height: 6px;
                background: #1a202966;
                Rectangle {
                    width: parent.width * hp / 100;
                    height: 100%;
                    background: hp > 30 ? #7bc98a : #e05d5d;
                }
            }
            Text { 
                text: Math.round(hp) + "%"; 
                color: #c2dbe4;
            }
        }
    }
}
```

#### 2.2 目标锁定动画
```slint
export component TargetLock inherits Rectangle {
    in property <bool> locked: false;
    in property <string> target-name: "Unknown";
    
    width: 100%; height: 120px;
    
    // 锁定框动画
    if locked: Rectangle {
        border-width: 2px;
        border-color: #5fd0e0;
        background: transparent;
        
        animate border-color {
            duration: 1.2s;
            iteration-count: -1;
            easing: ease-in-out;
        }
    }
    
    VerticalLayout {
        padding: 12px;
        Text {
            text: target-name;
            color: #5fd0e0;
            font-size: 16px;
            font-weight: 700;
        }
        Text {
            text: locked ? "[ LOCKED ]" : "[ LOCKING... ]";
            color: locked ? #7bc98a : #e8c25a;
            font-size: 12px;
        }
    }
}
```

---

### 第三阶段：交互逻辑（3-5 天）

#### 3.1 Rust 状态管理
```rust
// src/game_state.rs
use slint::ComponentHandle;
use core::sync::atomic::{AtomicU8, Ordering};

pub struct GameState {
    pub hp: AtomicU8,
    pub shield: AtomicU8,
    pub target_locked: bool,
    pub target_name: &'static str,
}

impl GameState {
    pub fn new() -> Self {
        Self {
            hp: AtomicU8::new(100),
            shield: AtomicU8::new(100),
            target_locked: false,
            target_name: "Unknown",
        }
    }
    
    pub fn take_damage(&self, amount: u8) {
        let current_shield = self.shield.load(Ordering::Relaxed);
        if current_shield > 0 {
            let new_shield = current_shield.saturating_sub(amount);
            self.shield.store(new_shield, Ordering::Relaxed);
        } else {
            let current_hp = self.hp.load(Ordering::Relaxed);
            let new_hp = current_hp.saturating_sub(amount);
            self.hp.store(new_hp, Ordering::Relaxed);
        }
    }
}

static GAME_STATE: Mutex<RefCell<Option<GameState>>> = 
    Mutex::new(RefCell::new(None));
```

#### 3.2 UI 更新循环
```rust
// src/main.rs (集成到现有代码)
use crate::game_state::GameState;

// 在主循环中添加游戏状态更新
pub async fn run_ui_with_game(window: Rc<MinimalSoftwareWindow>) -> ! {
    let ui = HudLayout::new().expect("创建 HUD 失败");
    let game_state = GameState::new();
    
    ui.show().expect("显示窗口失败");
    
    info!("进入 EVE HUD 主循环");
    
    loop {
        let frame_start = embassy_time::Instant::now();
        
        // 更新游戏逻辑
        game_state.tick();
        
        // 同步状态到 UI
        ui.set_hp(game_state.hp.load(Ordering::Relaxed) as f32);
        ui.set_shield(game_state.shield.load(Ordering::Relaxed) as f32);
        ui.set_target_locked(game_state.target_locked);
        
        // 驱动 Slint 定时器与动画
        slint::platform::update_timers_and_animations();
        
        // 渲染
        window.draw_if_needed(|renderer| {
            renderer.render(display::frame_buffer(), board::LCD_WIDTH as usize);
        });
        
        let result = display::flush();
        if let Err(e) = result {
            warn!("显示刷新失败: {:?}", e);
        }
        
        // 动态帧率
        let elapsed_ms = frame_start.elapsed().as_millis();
        let sleep_ms = board::TARGET_FRAME_TIME_MS.saturating_sub(elapsed_ms);
        embassy_time::Timer::after(embassy_time::Duration::from_millis(sleep_ms)).await;
    }
}
```

---

### 第四阶段：特效优化（2-3 天）

#### 4.1 简化的启动序列
```slint
// ui/boot_sequence.slint
export component BootSequence inherits Rectangle {
    callback reveal();
    
    in-out property <int> stage: 0; // 0-4
    in-out property <float> progress: 0.0;
    
    width: 100%; height: 100%;
    background: #010407;
    
    VerticalLayout {
        alignment: center;
        spacing: 20px;
        
        // 标题
        if stage >= 1: Text {
            text: "NEURAL LINK";
            color: #5fd0e0;
            font-size: 24px;
            font-weight: 700;
        }
        
        // 系统检查
        if stage >= 2: VerticalLayout {
            spacing: 8px;
            Text { text: "[ CORE ] Online"; color: #7bc98a; }
            Text { text: "[ NAV  ] Online"; color: #7bc98a; }
            Text { text: "[ TACT ] Online"; color: #7bc98a; }
        }
        
        // 进度条
        if stage >= 3: Rectangle {
            width: 200px; height: 4px;
            background: #1a2029;
            Rectangle {
                width: parent.width * progress;
                height: 100%;
                background: #5fd0e0;
            }
        }
        
        // 启动按钮
        if stage >= 4: TouchArea {
            width: 120px; height: 40px;
            clicked => { root.reveal(); }
            
            Rectangle {
                background: #5fd0e020;
                border-width: 1px;
                border-color: #5fd0e0;
                
                Text {
                    text: "ENGAGE";
                    color: #5fd0e0;
                    horizontal-alignment: center;
                    vertical-alignment: center;
                }
            }
        }
    }
}
```

#### 4.2 Rust 启动逻辑
```rust
// src/boot.rs
pub async fn run_boot_sequence(ui: &BootSequence) {
    info!("启动序列开始");
    
    // Stage 1: 标题出现
    embassy_time::Timer::after_millis(500).await;
    ui.set_stage(1);
    
    // Stage 2: 系统检查
    embassy_time::Timer::after_millis(800).await;
    ui.set_stage(2);
    
    // Stage 3: 加载进度
    embassy_time::Timer::after_millis(600).await;
    ui.set_stage(3);
    
    for i in 0..=100 {
        ui.set_progress(i as f32 / 100.0);
        embassy_time::Timer::after_millis(15).await;
    }
    
    // Stage 4: 启动按钮
    ui.set_stage(4);
    
    info!("启动序列完成,等待用户交互");
}
```

---

## 📊 性能预估

### 内存占用
```
Slint UI 代码: ~50KB
静态资源(图片/字体): ~200KB
游戏状态: ~10KB
帧缓冲(PSRAM): 450KB
----------------------------
总计: ~710KB (16MB flash 的 4.3%)
```

### 运行时性能
```
目标帧率: 30fps (33ms/帧)
渲染耗时: ~25ms (软件渲染)
逻辑耗时: ~3ms
余量: ~5ms
```

---

## 🛠️ 实施步骤

### Week 1: 基础框架
- [ ] 创建 EVE 主题 token (colors/fonts/sizes)
- [ ] 实现 EvePanel 基础组件
- [ ] 搭建 HudLayout 布局
- [ ] 集成静态星空背景

### Week 2: 核心组件
- [ ] StatusBar (HP/Shield)
- [ ] TargetPanel (目标锁定)
- [ ] NavBar (导航图标)
- [ ] ShipConsole (底部控制台)

### Week 3: 交互与特效
- [ ] 游戏状态管理
- [ ] 触摸交互
- [ ] 简化启动序列
- [ ] 扫描线/数值动画

### Week 4: 优化与调试
- [ ] 性能优化(保持 30fps)
- [ ] 内存优化
- [ ] 硬件调试
- [ ] 用户测试

---

## 🎯 最终效果预期

**保留**:
- ✅ EVE 科幻风格 90%
- ✅ 核心 HUD 功能 70%
- ✅ 配色与字体 100%
- ✅ 基础动画 60%

**妥协**:
- ⚠️ 无 3D 场景
- ⚠️ 简化启动序列
- ⚠️ 固定布局(不可拖拽)
- ⚠️ 减少动画数量

**体验**:
- ✅ 识别度高(一眼认出 EVE 风格)
- ✅ 流畅(30fps)
- ✅ 响应快速(<100ms)
- ✅ 嵌入式设备上独一无二的视觉体验

---

## 🚨 风险与对策

### 风险 1: 性能不足
**对策**: 
- 减少并发动画
- 降低目标帧率至 20fps
- 移除复杂特效

### 风险 2: 内存溢出
**对策**:
- 压缩静态资源
- 动态加载/卸载
- 减少缓存

### 风险 3: 开发周期过长
**对策**:
- 分阶段交付
- 先做概念演示
- 逐步迭代功能

---

## 💡 创新点

1. **首个嵌入式 EVE 风格 HUD** - 在 MCU 上实现 AAA 游戏视觉风格
2. **Slint + Rust 技术展示** - 证明嵌入式也能有高品质 UI
3. **性能优化案例** - 在极限硬件上实现复杂 UI 的最佳实践
4. **开源参考项目** - 可作为其他嵌入式 GUI 项目的范例

---

## 📝 总结

将 EVE HUD 移植到 ESP32-S31 是**可行但有挑战**的。推荐采用**视觉风格移植**策略：

**核心思路**: 
- 保留 EVE 的视觉语言(配色/字体/布局)
- 用 Slint 重新实现简化版
- 移除 3D 和复杂特效
- 优化性能达到 30fps

**预期成果**:
- 在 480×480 AMOLED 上呈现 EVE 风格 HUD
- 流畅的交互体验
- 独特的嵌入式视觉演示
- 3-4 周开发周期

**下一步**:
1. 确认移植范围(完整 HUD vs 概念演示)
2. 设计静态资源(星空背景/图标)
3. 开始基础组件库开发
