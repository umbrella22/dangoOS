//! Host measurement only. Each executable links exactly one interpreter.
#[cfg(all(feature = "lua", feature = "ts"))]
compile_error!("select exactly one benchmark engine: lua or ts");

#[cfg(any(feature = "lua", feature = "ts"))]
mod engines;
#[cfg(any(feature = "lua", feature = "ts", test))]
mod recorder;

fn main() -> std::process::ExitCode {
    #[cfg(any(feature = "lua", feature = "ts"))]
    let result = engines::run();
    #[cfg(not(any(feature = "lua", feature = "ts")))]
    let result: Result<(), String> = Err("build with --features lua or --features ts".into());
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("script-bench-runner: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}
