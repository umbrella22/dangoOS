//! Rounded physical viewport contact ownership, shared by the firmware input
//! dispatcher and host pointer regression. Radius is a configurable assumption.
const SIDE: f32 = 480.0;

pub fn contains(x: f32, y: f32, radius: f32) -> bool {
    if !x.is_finite() || !y.is_finite() || !(0.0..SIDE).contains(&x) || !(0.0..SIDE).contains(&y) {
        return false;
    }
    let radius = radius.clamp(0.0, SIDE / 2.0);
    let dx = x - x.clamp(radius, SIDE - radius);
    let dy = y - y.clamp(radius, SIDE - radius);
    dx * dx + dy * dy <= radius * radius
}

#[derive(Default)]
pub struct ScreenContact {
    active: bool,
    accepted: bool,
}
impl ScreenContact {
    pub fn sample(&mut self, x: f32, y: f32, radius: f32) -> bool {
        if !self.active {
            self.active = true;
            self.accepted = contains(x, y, radius);
        }
        self.accepted
    }
    pub fn accepted(&self) -> bool {
        self.active && self.accepted
    }
    pub fn release(&mut self) -> bool {
        let accepted = self.accepted();
        self.active = false;
        self.accepted = false;
        accepted
    }
    /// An I2C error is not a confirmed release. Both accepted and ignored
    /// contacts keep their original lease until an actual Up report.
    pub fn transport_error(&self) {}
}
