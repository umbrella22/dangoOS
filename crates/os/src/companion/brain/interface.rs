//! Typed/raw callback inputs and staged expression, visual, haptic and LED output.
use crate::companion::behavior::{
    affect::{EventKind, HapticPattern, SensorEvent, SystemCommand},
    Expression,
};
use crate::{companion::render::FoxPose, FoxVisual};
use alloc::{
    format,
    string::{String, ToString},
};
use lua_engine::{Call, Error, Host, Request, Ret};

pub(super) const COMPLETE: u32 = 0xB001;
pub(super) const IDLE: u32 = 0xB002;
const NAMES: [&str; 23] = [
    "Idle",
    "Sleep",
    "Curious",
    "Happy",
    "Puzzled",
    "Confused",
    "Surprised",
    "Panic",
    "Angry",
    "Satisfied",
    "Listening",
    "Thinking",
    "Speaking",
    "Error",
    "Tired",
    "Dying",
    "Peering",
    "Dazzled",
    "NetworkDisconnected",
    "NetworkConnected",
    "BluetoothDisconnected",
    "BluetoothConnected",
    "MusicPlaying",
];
const POSES: [&str; 25] = [
    "Idle",
    "Sleep",
    "Curious",
    "Happy",
    "Puzzled",
    "Confused",
    "Surprised",
    "Panic",
    "Angry",
    "Satisfied",
    "Listening",
    "Thinking",
    "Speaking",
    "Error",
    "Tired",
    "Dying",
    "Annoyed",
    "Charging",
    "Peering",
    "Dazzled",
    "NetworkDisconnected",
    "NetworkConnected",
    "BluetoothDisconnected",
    "BluetoothConnected",
    "MusicPlaying",
];
const HAPTICS: [HapticPattern; 13] = [
    HapticPattern::Panic,
    HapticPattern::Angry,
    HapticPattern::Surprised,
    HapticPattern::Puzzled,
    HapticPattern::Happy,
    HapticPattern::Satisfied,
    HapticPattern::Confused,
    HapticPattern::Listening,
    HapticPattern::Thinking,
    HapticPattern::Error,
    HapticPattern::ModuleInsert,
    HapticPattern::ModuleRemove,
    HapticPattern::MagneticContact,
];
pub fn expression(name: &str) -> Option<Expression> {
    NAMES
        .iter()
        .position(|n| *n == name)
        .map(|i| Expression::ALL[i])
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    pub expression: Expression,
    pub visual: FoxVisual,
    pub play: Option<(FoxPose, u64)>,
    pub haptic: Option<HapticPattern>,
    pub command: Option<SystemCommand>,
    pub leds: Option<[[u8; 3]; 6]>,
}
impl Default for Output {
    fn default() -> Self {
        Self {
            expression: Expression::Idle,
            visual: FoxVisual::IDLE,
            play: None,
            haptic: None,
            command: None,
            leds: None,
        }
    }
}
/// Commands generated internally; raw JSON remains the public compatibility path.
#[derive(Clone, Copy, Debug)]
pub(super) enum Input {
    Tick(u64),
    Sensor(SensorEvent),
    Pointer(crate::companion::behavior::personality::PointerSample),
    Interaction(u64),
    IgnoreTouch(u64),
}
pub(super) enum Inbox {
    Raw(String),
    Typed(Input),
}
pub(super) struct BrainHost {
    pub(super) inbox: Option<Inbox>,
    pub(super) response: Option<String>,
    pub(super) output: Output,
    pub(super) now: u64,
}
impl Host for BrainHost {
    fn print(&mut self, _line: &[u8]) {}
}
pub(super) fn next(c: &mut Call) -> Result<Ret, Error> {
    let host = c
        .host::<BrainHost>()
        .ok_or_else(|| Error::msg("brain host missing"))?;
    if let Some(input) = host.inbox.take() {
        match input {
            Inbox::Raw(raw) => {
                c.push_str(&raw);
                Ok(Ret::Values(1))
            }
            Inbox::Typed(input) => Ok(input.push(c)),
        }
    } else {
        Ok(Ret::Yield(Request::Host(IDLE)))
    }
}
impl Input {
    /// Return at most seven scalars through the existing host trampoline.
    /// Allocate tables only in Lua, under lua_resume's protected boundary and
    /// heap cap. JSON's decoder pushes all numbers as floats, including u64s.
    fn push(self, c: &mut Call) -> Ret {
        match self {
            Self::Tick(now) => {
                c.push_number(1.);
                c.push_number(now as f64);
                Ret::Values(2)
            }
            Self::Pointer(p) => {
                c.push_number(2.);
                c.push_number(p.at_ms as f64);
                c.push_number(p.x as f64);
                c.push_number(p.y as f64);
                c.push_bool(p.pressed);
                Ret::Values(5)
            }
            Self::Interaction(now) | Self::IgnoreTouch(now) => {
                c.push_number(if matches!(self, Self::Interaction(_)) {
                    3.
                } else {
                    4.
                });
                c.push_number(now as f64);
                Ret::Values(2)
            }
            Self::Sensor(e) => {
                c.push_number(5.);
                c.push_number(e.at_ms as f64);
                c.push_str(&format!("{:?}", e.source));
                match e.kind {
                    EventKind::Accel {
                        ax_mg,
                        ay_mg,
                        az_mg,
                    } => {
                        c.push_str("Accel");
                        for n in [ax_mg, ay_mg, az_mg] {
                            c.push_number(n as f64);
                        }
                        Ret::Values(7)
                    }
                    EventKind::Gyro {
                        gx_dps,
                        gy_dps,
                        gz_dps,
                    } => {
                        c.push_str("Gyro");
                        for n in [gx_dps, gy_dps, gz_dps] {
                            c.push_number(n as f64);
                        }
                        Ret::Values(7)
                    }
                    EventKind::MagneticEdge { confidence } => {
                        c.push_str("MagneticEdge");
                        c.push_number(confidence as f64);
                        Ret::Values(5)
                    }
                    EventKind::LightLevel(n) | EventKind::BatteryLevel(n) => {
                        c.push_str(if matches!(e.kind, EventKind::LightLevel(_)) {
                            "LightLevel"
                        } else {
                            "BatteryLevel"
                        });
                        c.push_number(n as f64);
                        Ret::Values(5)
                    }
                    EventKind::Presence(value)
                    | EventKind::NetworkConnection(value)
                    | EventKind::BluetoothConnection(value)
                    | EventKind::MusicPlayback(value) => {
                        c.push_str(match e.kind {
                            EventKind::Presence(_) => "Presence",
                            EventKind::NetworkConnection(_) => "NetworkConnection",
                            EventKind::BluetoothConnection(_) => "BluetoothConnection",
                            _ => "MusicPlayback",
                        });
                        c.push_bool(value);
                        Ret::Values(5)
                    }
                    EventKind::ModuleInserted(value) | EventKind::ModuleRemoved(value) => {
                        c.push_str(if matches!(e.kind, EventKind::ModuleInserted(_)) {
                            "ModuleInserted"
                        } else {
                            "ModuleRemoved"
                        });
                        c.push_str(&format!("{value:?}"));
                        Ret::Values(5)
                    }
                    EventKind::Activity(value) => {
                        c.push_str("Activity");
                        c.push_str(&format!("{value:?}"));
                        Ret::Values(5)
                    }
                    unit => {
                        c.push_str(&format!("{unit:?}"));
                        Ret::Values(4)
                    }
                }
            }
        }
    }
}
pub(super) fn complete(c: &mut Call) -> Result<Ret, Error> {
    let text = c.str(1)?;
    if text.len() > 4096 {
        return Err(Error::msg("callback result too large"));
    }
    let text = text.to_string();
    c.host::<BrainHost>().unwrap().response = Some(text);
    Ok(Ret::Yield(Request::Host(COMPLETE)))
}
pub(super) fn set_expression(c: &mut Call) -> Result<Ret, Error> {
    let e = expression(c.str(1)?).ok_or_else(|| Error::arg(1, "unknown expression"))?;
    c.host::<BrainHost>().unwrap().output.expression = e;
    c.push_bool(true);
    Ok(Ret::Values(1))
}
pub(super) fn play(c: &mut Call) -> Result<Ret, Error> {
    let name = c.str(1)?;
    let pose = POSES
        .iter()
        .position(|n| *n == name)
        .map(|i| FoxPose::ALL[i])
        .ok_or_else(|| Error::arg(1, "unknown pose"))?;
    let ms = c.integer(2)?;
    if !(1..=60000).contains(&ms) {
        return Err(Error::arg(2, "duration must be 1..60000 ms"));
    }
    let h = c.host::<BrainHost>().unwrap();
    h.output.play = Some((pose, h.now.saturating_add(ms as u64)));
    c.push_bool(true);
    Ok(Ret::Values(1))
}
pub(super) fn visual(c: &mut Call) -> Result<Ret, Error> {
    let mut v = [0.; 4];
    for (i, x) in v.iter_mut().enumerate() {
        *x = c.number(i as i32 + 1)?;
        if !x.is_finite() {
            return Err(Error::arg(i as i32 + 1, "finite number required"));
        }
    }
    if v[0].abs() > 8.
        || !(0. ..=1.).contains(&v[1])
        || !(0. ..=1.).contains(&v[2])
        || v[3].abs() > 6.
    {
        return Err(Error::msg("visual out of range"));
    }
    c.host::<BrainHost>().unwrap().output.visual = FoxVisual {
        scale_x: 1.,
        scale_y: 1.,
        look_x: v[0] as f32,
        pulse: v[1] as f32,
        pet: v[2] as f32,
        lean_x: v[3] as f32,
    };
    Ok(Ret::Values(0))
}
pub(super) fn haptic(c: &mut Call) -> Result<Ret, Error> {
    let name = c.str(1)?;
    let p = HAPTICS
        .iter()
        .copied()
        .find(|p| format!("{p:?}") == name)
        .ok_or_else(|| Error::arg(1, "unknown haptic"))?;
    c.host::<BrainHost>().unwrap().output.haptic = Some(p);
    c.push_bool(true);
    Ok(Ret::Values(1))
}
pub(super) fn command(c: &mut Call) -> Result<Ret, Error> {
    let v = match c.str(1)? {
        "Hub" => SystemCommand::Hub,
        "StopActivity" => SystemCommand::StopActivity,
        "StartListening" => SystemCommand::StartListening,
        _ => return Err(Error::arg(1, "unknown system command")),
    };
    c.host::<BrainHost>().unwrap().output.command = Some(v);
    Ok(Ret::Values(0))
}
pub(super) fn led_set(c: &mut Call) -> Result<Ret, Error> {
    let index = c.integer(1)?;
    if !(1..=6).contains(&index) {
        return Err(Error::arg(1, "LED index must be 1..6"));
    }
    let mut rgb = [0; 3];
    for (i, v) in rgb.iter_mut().enumerate() {
        let n = c.integer(i as i32 + 2)?;
        if !(0..=255).contains(&n) {
            return Err(Error::arg(i as i32 + 2, "RGB must be 0..255"));
        }
        *v = n as u8;
    }
    c.host::<BrainHost>()
        .unwrap()
        .output
        .leds
        .get_or_insert([[0; 3]; 6])[index as usize - 1] = rgb;
    c.push_bool(true);
    Ok(Ret::Values(1))
}
pub(super) fn led_clear(c: &mut Call) -> Result<Ret, Error> {
    c.host::<BrainHost>().unwrap().output.leds = None;
    Ok(Ret::Values(0))
}
