//! Desktop simulator entry point.

mod app;
mod simulation;

fn main() -> Result<(), slint::PlatformError> {
    app::run()
}
