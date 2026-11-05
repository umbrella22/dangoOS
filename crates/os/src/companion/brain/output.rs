use crate::companion::behavior::{
    affect::{HapticPattern, SystemCommand},
    Expression,
};
use crate::{companion::render::FoxPose, FoxVisual};
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
