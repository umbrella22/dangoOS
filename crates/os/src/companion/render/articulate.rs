//! Stylized mitten gestures and a small body sway for the v4 character layers.
use super::{
    Scene,
    math::{PI, cos, round, sin},
    scene::{ArticulatedLayer, Layer, Layers, RigFrame},
    skin::{BalanceRig, RigArticulation},
};
use crate::companion::behavior::balance::{BalanceFace, BalanceStage, BalanceVisual};

pub(super) fn pose(
    scene: &mut Scene,
    rig: BalanceRig,
    art: RigArticulation,
    motion: BalanceVisual,
    t: u32,
    blink: bool,
) -> f32 {
    let moving = motion.active;
    let grip = motion.grip;
    // A held mitten may float a little from the cheek. The character remains
    // a soft dango silhouette, with a short squash/stretch of the mitten itself.
    let body_dx = round(-motion.hold_side * motion.drag * grip * 7.0) as i16;
    let body_dy = round(-6.0 * (1.0 - motion.drag) * grip) as i16;
    let breathing = sin(t as f32 / 620.0);
    let sway = motion.hold_side * (5.0 + sin(t as f32 / 115.0) * 3.0) * grip;
    let angle = sway * PI / 180.0;
    let hands = core::array::from_fn(|i| {
        let side = if i == 0 { -1.0 } else { 1.0 };
        let leading = side * motion.hold_side > 0.0;
        let walk = side * motion.step;
        let scoot_stretch = if motion.stage == BalanceStage::Scooting {
            walk.max(0.0)
        } else {
            0.0
        };
        let loose_x = side * motion.reach * 12.0;
        let loose_y = -motion.reach * 7.0 - walk.max(0.0) * 6.0;
        let held_x = side * if leading { 23.0 } else { 10.0 };
        let held_y = if leading {
            -30.0 - motion.drag * 6.0
        } else {
            -5.0 + walk * 6.0
        };
        let dx = loose_x + (held_x - loose_x) * grip;
        let dy = loose_y + (held_y - loose_y) * grip + breathing * 0.6;
        let x = (rig.hands[i].img.x + rig.hands[i].pivot[0]) as f32 + dx - 176.0;
        let y = (rig.hands[i].img.y + rig.hands[i].pivot[1]) as f32 + dy - 168.0;
        // Counter the body sway so the leading mitten reads as holding one
        // side while the head tilts underneath it.
        let hold = if leading { grip } else { grip * 0.25 };
        ArticulatedLayer {
            img: rig.hands[i].img,
            pivot: rig.hands[i].pivot,
            angle_deg: side * (12.0 * grip + 10.0 * motion.reach) + walk * 6.0 - sway * hold,
            scale_x: 1.0 - grip * if leading { 0.05 } else { 0.02 } - scoot_stretch * 0.025,
            scale_y: 1.0
                + scoot_stretch * 0.12
                + grip
                    * if leading {
                        0.12 + motion.drag * 0.12
                    } else {
                        0.04
                    },
            dx: round(dx + (x * cos(angle) + y * sin(angle) - x) * hold) as i16,
            dy: round(dy + (-x * sin(angle) + y * cos(angle) - y) * hold) as i16,
        }
    });
    let ears = core::array::from_fn(|i| {
        let original = art.ears[i];
        let side = if i == 0 { -1.0 } else { 1.0 };
        // Low amplitude idle drift, with delayed follow-through under effort.
        let fold = match motion.stage {
            BalanceStage::Struggling => 2.0 * motion.reach,
            BalanceStage::Falling => 2.5,
            BalanceStage::Landing => 2.0 * motion.reach,
            _ => 0.0,
        };
        ArticulatedLayer {
            img: original.img,
            pivot: original.pivot,
            angle_deg: side * fold
                + sin(t as f32 / 430.0 + i as f32 * 1.2) * if moving { 1.0 } else { 0.6 },
            scale_x: 1.0,
            scale_y: 1.0,
            dx: body_dx,
            dy: body_dy,
        }
    });
    scene.under = Layers::new();
    scene.over = Layers::new();
    let face = match motion.face {
        BalanceFace::Effort => Some(rig.faces[0]),
        BalanceFace::Dizzy => Some(rig.faces[1]),
        BalanceFace::Weary => Some(rig.faces[2]),
        _ if blink => Some(art.blink),
        _ => None,
    };
    if let Some(face) = face {
        scene.under.push(Layer::at(&face));
    }
    scene.rig = Some(RigFrame {
        body: rig.body,
        body_dx,
        body_dy,
        hands,
        ears: Some(ears),
    });
    sway
}
