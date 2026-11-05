use crate::companion::behavior::affect::SensorEvent;
use alloc::string::String;
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
