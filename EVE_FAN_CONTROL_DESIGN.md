# EVE 风格风扇控制系统 - 完整设计方案

**项目**: ESP32-S31 风扇控制面板  
**视觉风格**: EVE Online 科幻全息 HUD  
**目标硬件**: ESP-Mosaico (480×480 AMOLED)  
**UI 框架**: Slint 1.17 + Rust no_std

---

## 🎨 设计概览

将 EVE Online 的**青金石蓝科幻风格**应用到嵌入式风扇控制器，打造独一无二的工业美学体验。

### 核心视觉元素
- **配色**: 青金石蓝 (#5fd0e0) + 深空黑 (#02060a) + 琥珀金 (#e8c25a)
- **面板**: 半透明、四角标记、青色边框
- **动画**: 扫描线、数值变化、辉光效果
- **字体**: Rajdhani (标题) + Share Tech Mono (数据)

---

## 📋 启动流程设计

### 完整启动序列（4 秒）

**查看交互式流程图**: [docs/eve-fan-boot-flow.html](docs/eve-fan-boot-flow.html)

```
上电启动 (0s)
    ↓
NEURAL LINK 标题 (0.5s)
  - 青金石蓝大字标题
  - 扫描线从上到下移动
    ↓
系统自检 (1.5s)
  - [ CORE ] · 在线 ✓
  - [ THERMAL ] · 在线 ✓
  - [ CONTROL ] · 在线 ✓
    ↓
传感器校准 (2.3s)
  - 温度传感器初始化...
  - 转速传感器初始化...
  - PWM 模块就绪 ✓
    ↓
面板激活 (3.5s)
  - 温度监控面板点亮 ▶
  - 转速控制面板点亮 ▶
  - 状态指示面板点亮 ▶
    ↓
系统就绪 (4.0s)
  - 显示 [ ENGAGE ] 按钮
  - 辉光效果 + 等待用户点击
```

### 视觉特效详解

1. **NEURAL LINK 标题** (第 1 阶段)
   - 字体: Rajdhani Bold, 24px
   - 颜色: #5fd0e0 (青金石蓝)
   - 动画: 从透明度 0 淡入到 1 (0.3s)
   - 背景: 垂直扫描线从 -100px 移动到 580px (5s 循环)

2. **系统自检** (第 2 阶段)
   ```
   [ CORE    ] · 在线    (0.3s 后显示)
   [ THERMAL ] · 在线    (0.6s 后显示)
   [ CONTROL ] · 在线    (0.9s 后显示)
   ```
   - 每行逐个显示，间隔 0.3s
   - 绿色 ✓ 勾选标记: #7bc98a
   - 字体: Share Tech Mono, 13px

3. **传感器校准** (第 3 阶段)
   - 进度条: 0% → 100% (0.8s)
   - 颜色: #5fd0e0 (青色)
   - 背景: #1a2029 (深灰)
   - 高度: 4px, 圆角

4. **面板激活** (第 4 阶段)
   - 三个面板依次从左到右点亮
   - 间隔: 0.4s
   - 效果: 边框从透明变为 #6ebdd266
   - 内容淡入: opacity 0 → 1

5. **ENGAGE 按钮** (第 5 阶段)
   ```css
   宽度: 120px
   高度: 40px
   背景: #5fd0e020 (半透明青色)
   边框: 1px solid #5fd0e0
   文字: "ENGAGE"
   辉光: 0 0 12px rgba(95, 208, 224, 0.3)
   ```
   - 鼠标悬停: 辉光增强到 0.5
   - 点击后: 淡出整个启动界面，进入主界面

---

## 🖥️ 主界面设计

### 界面预览

**查看完整 SVG 设计图**: [docs/eve-fan-ui-mockup.svg](docs/eve-fan-ui-mockup.svg)

### 布局结构 (480×480)

```
┌────────────────────────────────────────────────┐
│  THERMAL CONTROL SYSTEM           [状态栏]     │ 65px
├────────────────────────────────────────────────┤
│                                                │
│  ┌─────────────┐        ┌─────────────┐      │
│  │ 温度监控     │        │ 转速监控     │      │ 160px
│  │  28.5°C     │        │  2450 RPM   │      │
│  └─────────────┘        └─────────────┘      │
│                                                │
├────────────────────────────────────────────────┤
│  风扇控制                                       │
│  ◀═══════════●══════════════▶  75%           │ 120px
│  0    25    50    75   100                   │
└────────────────────────────────────────────────┘
```

### 组件详细设计

#### 1. 顶部状态栏 (10, 65, 460×50)
```
┌──────────────────────────────────────────────┐
│ SYSTEM STATUS                     [ ONLINE ] │
│ UPTIME: 00:23:41  CPU: 42°C                 │
└──────────────────────────────────────────────┘
```
- **背景**: 半透明渐变 (#0a1821 → #071119, 85-90% opacity)
- **边框**: 1px solid #6ebdd266 (半透明青色)
- **四角标记**: 14px 长的青色线条 (#6ebdd26e)
- **在线指示**: 绿色圆点 (#7bc98a) + 辉光滤镜
- **字体**: 
  - 标题: Rajdhani SemiBold 12px
  - 数据: Share Tech Mono 11px

#### 2. 温度监控面板 (10, 130, 220×160)
```
┌────────────────────┐
│ THERMAL MONITOR    │
├────────────────────┤
│ SENSOR-01          │
│                    │
│    28.5 °C         │
│    ████████░░░     │ ← 进度条
│                    │
│ NOMINAL            │
│ WARNING: 60°C      │
│ ▲ +0.2°C/min       │
└────────────────────┘
```
- **温度显示**: 32px 大字号, #5fd0e0
- **进度条**: 
  - 背景: #1a2029
  - 填充: #5fd0e0 (根据温度动态调整)
  - 高度: 8px, 圆角 1px
- **趋势箭头**: 
  - 上升 ▲ 绿色 #7bc98a
  - 下降 ▼ 蓝色 #6fb7ff
  - 稳定 ● 灰色 #68838f
- **阈值警告**: 
  - < 50°C: NOMINAL (绿色)
  - 50-60°C: ELEVATED (黄色 #e8c25a)
  - > 60°C: WARNING (红色 #e05d5d)

#### 3. 转速监控面板 (250, 130, 220×160)
```
┌────────────────────┐
│ ROTATION SPEED     │
├────────────────────┤
│ FAN-01             │
│                    │
│   2450 RPM    ◉75% │ ← 圆形百分比
│                    │
│ POWER: 5.2W        │
│ EFFICIENCY: OPTIMAL│
└────────────────────┘
```
- **转速显示**: 28px 琥珀金色 #e8c25a
- **百分比环形图**:
  - 直径: 50px (半径 25px)
  - 背景环: #1a2029, 4px 宽
  - 进度环: #e8c25a, 4px 宽, 辉光效果
  - 数值: 居中, 16px
- **效率状态**:
  - OPTIMAL: 绿色 #7bc98a
  - GOOD: 青色 #5fd0e0
  - LOW: 黄色 #e8c25a
  - CRITICAL: 红色 #e05d5d

#### 4. 风扇控制面板 (10, 310, 460×120)
```
┌──────────────────────────────────────────────┐
│ FAN CONTROL                                  │
├──────────────────────────────────────────────┤
│ SPEED LEVEL                                  │
│                                              │
│ ◀═══════════●══════════════▶                │
│ 0    25    50    75   100                   │
└──────────────────────────────────────────────┘
```
- **滑杆设计**:
  - 轨道: 430×12px, #1a2029, 圆角 6px
  - 填充: 左侧填充 #5fd0e020 (半透明青色)
  - 手柄: 
    - 外圈: 直径 28px, #071119 背景 + #5fd0e0 边框 (2px)
    - 内圆: 直径 12px, #5fd0e0 实心
    - 辉光: 0 0 8px rgba(95, 208, 224, 0.4)
- **刻度标记**:
  - 位置: 0, 25, 50, 75, 100
  - 当前档位: 青色 #5fd0e0, 2px 宽
  - 其他档位: 灰色 #6ebdd233, 1px 宽
- **交互**:
  - 拖拽手柄: 实时更新档位
  - 点击轨道: 跳转到对应档位
  - 触摸反馈: 手柄放大 1.1 倍

#### 5. 底部状态栏 (全宽, 448-480)
```
─────────────────────────────────────────────
MOSAICO-S31    [ NEURAL LINK ACTIVE ]  2026-09-08
```
- **分隔线**: 1px solid #6ebdd222
- **左侧**: 设备型号
- **中间**: 连接状态 (青色强调)
- **右侧**: 日期时间

---

## 🎯 Slint 实现要点

### 1. 基础组件库

#### EvePanel 组件
```slint
export component EvePanel inherits Rectangle {
    in property <string> title: "";
    in property <bool> active: false;
    
    background: linear-gradient(180deg, #0a1821e6 0%, #071119d9 100%);
    border-width: 1px;
    border-color: active ? #5fd0e0 : #6ebdd266;
    
    // 四角标记
    CornerMark { x: -1px; y: -1px; }
    CornerMark { x: parent.width - 13px; y: -1px; }
    CornerMark { x: -1px; y: parent.height - 13px; }
    CornerMark { x: parent.width - 13px; y: parent.height - 13px; }
    
    VerticalLayout {
        padding: 16px;
        spacing: 8px;
        
        Text {
            text: root.title;
            color: #5fd0e0;
            font-family: "Rajdhani";
            font-size: 13px;
            font-weight: 600;
        }
        
        @children
    }
}

component CornerMark inherits Rectangle {
    width: 14px;
    height: 14px;
    
    Rectangle {
        x: 0; y: 0;
        width: 14px;
        height: 1px;
        background: #6ebdd26e;
    }
    Rectangle {
        x: 0; y: 0;
        width: 1px;
        height: 14px;
        background: #6ebdd26e;
    }
}
```

#### 温度显示组件
```slint
export component TempDisplay inherits VerticalLayout {
    in property <float> temperature: 28.5;
    in property <float> trend: 0.2;
    
    spacing: 8px;
    
    Text {
        text: "SENSOR-01";
        color: #68838f;
        font-family: "Share Tech Mono";
        font-size: 10px;
    }
    
    HorizontalLayout {
        Text {
            text: Math.round(temperature * 10) / 10;
            color: #5fd0e0;
            font-family: "Rajdhani";
            font-size: 32px;
            font-weight: 700;
        }
        Text {
            text: "°C";
            color: #7895a1;
            font-family: "Rajdhani";
            font-size: 18px;
        }
    }
    
    // 进度条
    Rectangle {
        height: 8px;
        background: #1a2029;
        border-radius: 1px;
        
        Rectangle {
            width: parent.width * temperature / 100;
            height: 100%;
            background: #5fd0e0;
            border-radius: 1px;
        }
    }
    
    // 趋势指示
    Text {
        text: trend > 0 ? "▲ +" + trend + "°C/min" : 
              trend < 0 ? "▼ " + trend + "°C/min" : 
              "● 稳定";
        color: trend > 0 ? #7bc98a : 
               trend < 0 ? #6fb7ff : 
               #68838f;
        font-family: "Share Tech Mono";
        font-size: 9px;
    }
}
```

#### 转速显示组件
```slint
export component RpmDisplay inherits VerticalLayout {
    in property <int> rpm: 2450;
    in property <int> percentage: 75;
    in property <float> power: 5.2;
    
    spacing: 8px;
    
    Text {
        text: "FAN-01";
        color: #68838f;
        font-family: "Share Tech Mono";
        font-size: 10px;
    }
    
    HorizontalLayout {
        alignment: space-between;
        
        HorizontalLayout {
            Text {
                text: rpm;
                color: #e8c25a;
                font-family: "Rajdhani";
                font-size: 28px;
                font-weight: 700;
            }
            Text {
                text: "RPM";
                color: #7895a1;
                font-family: "Rajdhani";
                font-size: 16px;
            }
        }
        
        // 圆形百分比
        CircularProgress {
            width: 50px;
            height: 50px;
            progress: percentage;
        }
    }
    
    HorizontalLayout {
        Text {
            text: "POWER: " + power + "W";
            color: #68838f;
            font-family: "Share Tech Mono";
            font-size: 9px;
        }
        Text {
            text: "EFFICIENCY: OPTIMAL";
            color: #7bc98a;
            font-family: "Share Tech Mono";
            font-size: 9px;
        }
    }
}
```

#### 风扇控制滑杆
```slint
export component FanSlider inherits Rectangle {
    in-out property <int> value: 75;
    callback changed(int);
    
    height: 12px;
    background: #1a2029;
    border-radius: 6px;
    
    // 填充
    Rectangle {
        width: parent.width * root.value / 100;
        height: 100%;
        background: #5fd0e020;
        border-radius: 6px;
    }
    
    // 手柄
    Rectangle {
        x: (parent.width - 28px) * root.value / 100;
        y: -8px;
        width: 28px;
        height: 28px;
        border-radius: 14px;
        background: #071119;
        border-width: 2px;
        border-color: #5fd0e0;
        
        Rectangle {
            x: 8px;
            y: 8px;
            width: 12px;
            height: 12px;
            border-radius: 6px;
            background: #5fd0e0;
        }
    }
    
    TouchArea {
        pointer-event(event) => {
            if (event.kind == PointerEventKind.move) {
                root.value = Math.round(
                    Math.max(0, Math.min(100, 
                        event.position.x / root.width * 100
                    ))
                );
                root.changed(root.value);
            }
        }
    }
}
```

### 2. 主界面组合

```slint
export component FanControlUI inherits Window {
    width: 480px;
    height: 480px;
    background: #02060a;
    
    // 星空背景
    Image {
        source: @image-url("assets/starfield.png");
        width: 100%;
        height: 100%;
    }
    
    // 扫描线
    Rectangle {
        width: 100%;
        height: 2px;
        y: scanline-pos;
        background: linear-gradient(
            90deg,
            #5fd0e000 0%,
            #5fd0e026 50%,
            #5fd0e000 100%
        );
        
        property <length> scanline-pos: 0px;
        animate scanline-pos {
            duration: 5s;
            iteration-count: -1;
            easing: linear;
        }
    }
    
    VerticalLayout {
        padding: 10px;
        spacing: 15px;
        
        // 标题栏
        HorizontalLayout {
            Text {
                text: "THERMAL CONTROL SYSTEM";
                color: #5fd0e0;
                font-family: "Rajdhani";
                font-size: 16px;
                font-weight: 700;
            }
        }
        
        // 状态栏
        EvePanel {
            height: 50px;
            title: "SYSTEM STATUS";
            
            HorizontalLayout {
                Text { text: "UPTIME: 00:23:41"; }
                Text { text: "CPU: 42°C"; }
                Text { text: "[ ONLINE ]"; color: #7bc98a; }
            }
        }
        
        // 监控面板行
        HorizontalLayout {
            spacing: 20px;
            
            EvePanel {
                title: "THERMAL MONITOR";
                TempDisplay {
                    temperature: temp-value;
                    trend: temp-trend;
                }
            }
            
            EvePanel {
                title: "ROTATION SPEED";
                RpmDisplay {
                    rpm: fan-rpm;
                    percentage: fan-percentage;
                    power: fan-power;
                }
            }
        }
        
        // 控制面板
        EvePanel {
            height: 120px;
            title: "FAN CONTROL";
            
            VerticalLayout {
                Text {
                    text: "SPEED LEVEL";
                    color: #68838f;
                }
                
                FanSlider {
                    value <=> fan-speed;
                    changed(speed) => {
                        // 调用 Rust 回调
                        root.fan-speed-changed(speed);
                    }
                }
                
                // 刻度
                HorizontalLayout {
                    alignment: space-between;
                    Text { text: "0"; }
                    Text { text: "25"; }
                    Text { text: "50"; }
                    Text { text: "75"; color: #5fd0e0; }
                    Text { text: "100"; }
                }
            }
        }
        
        // 底部状态
        HorizontalLayout {
            Text { text: "MOSAICO-S31"; }
            Text { text: "[ NEURAL LINK ACTIVE ]"; color: #5fd0e0; }
            Text { text: "2026-09-08"; }
        }
    }
    
    // 属性
    in-out property <float> temp-value: 28.5;
    in-out property <float> temp-trend: 0.2;
    in-out property <int> fan-rpm: 2450;
    in-out property <int> fan-percentage: 75;
    in-out property <float> fan-power: 5.2;
    in-out property <int> fan-speed: 75;
    
    // 回调
    callback fan-speed-changed(int);
}
```

---

## 🔧 Rust 集成代码

### 状态管理
```rust
use core::sync::atomic::{AtomicU8, AtomicU16, Ordering};

pub struct FanControlState {
    pub temperature: AtomicU8,      // 温度 (°C * 10)
    pub rpm: AtomicU16,              // 转速 (RPM)
    pub target_speed: AtomicU8,      // 目标档位 (0-100)
    pub power: AtomicU8,             // 功率 (W * 10)
}

impl FanControlState {
    pub fn new() -> Self {
        Self {
            temperature: AtomicU8::new(285), // 28.5°C
            rpm: AtomicU16::new(2450),
            target_speed: AtomicU8::new(75),
            power: AtomicU8::new(52),  // 5.2W
        }
    }
    
    pub fn set_target_speed(&self, speed: u8) {
        self.target_speed.store(speed.clamp(0, 100), Ordering::Relaxed);
    }
    
    pub fn get_temperature_f32(&self) -> f32 {
        self.temperature.load(Ordering::Relaxed) as f32 / 10.0
    }
    
    pub fn get_rpm(&self) -> u16 {
        self.rpm.load(Ordering::Relaxed)
    }
}
```

### UI 更新循环
```rust
pub async fn run_fan_control_ui(window: Rc<MinimalSoftwareWindow>) -> ! {
    let ui = FanControlUI::new().expect("创建 UI 失败");
    let state = FanControlState::new();
    
    // 回调: 用户拖动滑杆
    ui.on_fan_speed_changed(|speed| {
        state.set_target_speed(speed as u8);
        info!("风扇档位更新: {}%", speed);
    });
    
    ui.show().expect("显示窗口失败");
    
    info!("进入 EVE 风扇控制主循环");
    
    loop {
        let frame_start = embassy_time::Instant::now();
        
        // 更新状态 (模拟)
        update_fan_state(&state);
        
        // 同步到 UI
        ui.set_temp_value(state.get_temperature_f32());
        ui.set_fan_rpm(state.get_rpm() as i32);
        ui.set_fan_percentage((state.target_speed.load(Ordering::Relaxed)) as i32);
        ui.set_fan_power(state.power.load(Ordering::Relaxed) as f32 / 10.0);
        
        // 驱动动画
        slint::platform::update_timers_and_animations();
        
        // 渲染
        window.draw_if_needed(|renderer| {
            renderer.render(display::frame_buffer(), 480);
        });
        
        display::flush().ok();
        
        // 30fps
        let elapsed = frame_start.elapsed().as_millis();
        let sleep = 33u64.saturating_sub(elapsed);
        embassy_time::Timer::after_millis(sleep).await;
    }
}

fn update_fan_state(state: &FanControlState) {
    // 模拟温度变化
    let temp = state.temperature.load(Ordering::Relaxed);
    let new_temp = if temp < 300 { temp + 1 } else { 285 };
    state.temperature.store(new_temp, Ordering::Relaxed);
    
    // 根据档位计算转速
    let speed = state.target_speed.load(Ordering::Relaxed);
    let rpm = (speed as u16) * 32 + 100; // 100-3300 RPM
    state.rpm.store(rpm, Ordering::Relaxed);
    
    // 计算功率
    let power = (speed as u8) * 8 / 10; // 0-8W
    state.power.store(power, Ordering::Relaxed);
}
```

---

## 📦 资源需求

### 静态资源
- **星空背景**: `assets/starfield.png` (480×480, 压缩后 ~20KB)
- **字体**: 
  - Rajdhani (粗体, 子集化) ~30KB
  - Share Tech Mono (子集化) ~25KB

### 内存占用估算
```
Slint UI 代码:     ~60KB
静态资源:         ~75KB (背景 + 字体)
帧缓冲 (PSRAM):   450KB
游戏状态:         ~2KB
────────────────────────
总计:            ~587KB
```

### 性能目标
- **帧率**: 30fps (33ms/帧)
- **渲染**: ~25ms
- **逻辑**: ~3ms
- **余量**: ~5ms

---

## 🎬 动画细节

### 1. 扫描线动画
```slint
Rectangle {
    y: scanline-y;
    animate scanline-y {
        duration: 5s;
        easing: linear;
        iteration-count: -1;
    }
}
```
从 -100px 移动到 580px，无限循环

### 2. 温度数值变化
```slint
Text {
    text: temperature;
    animate text {
        duration: 200ms;
        easing: ease-out;
    }
}
```
平滑过渡，避免跳变

### 3. 滑杆拖拽反馈
```slint
Rectangle {
    // 手柄
    scale: touch-area.pressed ? 1.1 : 1.0;
    animate scale {
        duration: 100ms;
        easing: ease-out;
    }
}
```
按下时放大 10%

### 4. 面板激活动画
```slint
EvePanel {
    opacity: panel-active ? 1.0 : 0.0;
    animate opacity {
        duration: 400ms;
        easing: ease-in-out;
    }
}
```
启动时逐个淡入

---

## 🚀 开发路线图

### Phase 1: 基础框架 (3-5 天)
- [x] 创建 Slint 组件库 (EvePanel, CornerMark)
- [x] 实现主界面布局
- [ ] 集成到现有 ESP32 项目
- [ ] 静态资源准备 (星空背景)

### Phase 2: 启动序列 (2-3 天)
- [ ] NEURAL LINK 标题动画
- [ ] 系统自检逐行显示
- [ ] 传感器校准进度条
- [ ] 面板激活序列
- [ ] ENGAGE 按钮

### Phase 3: 核心功能 (3-5 天)
- [ ] 温度显示组件
- [ ] 转速显示组件
- [ ] 风扇控制滑杆
- [ ] Rust 状态同步
- [ ] 触摸交互

### Phase 4: 视觉特效 (2-3 天)
- [ ] 扫描线动画
- [ ] 辉光滤镜
- [ ] 数值平滑过渡
- [ ] 四角标记动画

### Phase 5: 硬件集成 (3-5 天)
- [ ] 温度传感器读取
- [ ] PWM 输出实现
- [ ] 转速反馈 (如有)
- [ ] 性能优化 (保持 30fps)

---

## 📊 与原项目的对比

| 特性 | Web 版本 (Vue + Three.js) | 嵌入式版本 (Slint + Rust) |
|------|---------------------------|---------------------------|
| 视觉风格 | 100% EVE 风格 | 90% EVE 风格 ✅ |
| 配色方案 | 青金石蓝 | 完全一致 ✅ |
| 3D 场景 | Three.js 动态星空 | 静态星空背景 ⚠️ |
| 面板样式 | 半透明 + 四角标记 | 完全一致 ✅ |
| 动画效果 | 复杂粒子系统 | 简化扫描线 ⚠️ |
| 启动序列 | 10+ 步骤 | 5 步简化 ⚠️ |
| 交互响应 | < 16ms | < 100ms ✅ |
| 运行性能 | 60fps | 30fps ✅ |
| 内存占用 | ~100MB | ~600KB ✅ |

---

## 💡 创新点

1. **首个 EVE 风格嵌入式 UI** - 将 AAA 游戏视觉带到 MCU
2. **Slint 在工业应用中的实践** - 证明嵌入式也能高品质
3. **完整的设计到实现流程** - 可作为其他项目的范例
4. **性能与美学的平衡** - 在极限硬件上的优雅实现

---

## 📝 下一步

1. **确认设计** - 查看 SVG 设计图和启动流程图
2. **准备资源** - 生成星空背景图片
3. **开始编码** - 从基础组件库开始
4. **迭代测试** - 在硬件上验证性能

---

**设计文件位置**:
- 启动流程图: `docs/eve-fan-boot-flow.html` (可在浏览器中打开查看交互式动画)
- 主界面设计: `docs/eve-fan-ui-mockup.svg` (完整 480×480 SVG)
- 本设计文档: `EVE_FAN_CONTROL_DESIGN.md`

**准备好开始实现了吗？** 🚀
