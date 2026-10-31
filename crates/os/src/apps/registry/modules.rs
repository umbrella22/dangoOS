//! Required-module discovery against the firmware-provided Lua modules.
use super::PROVIDED_MODULES;
use alloc::{string::String, vec::Vec};

/// Modules `source` requires unconditionally that are neither provided nor
/// found by `local_module` (searching the app and shared directories).
/// `pcall(require, "x")` is optional and does not count; neither do
/// commented-out lines.
pub fn missing_modules(source: &str, local_module: &mut dyn FnMut(&str) -> bool) -> Vec<String> {
    let mut missing: Vec<String> = Vec::new();
    for line in source.lines() {
        let code = line.split("--").next().unwrap_or("");
        let mut rest = code;
        while let Some(at) = rest.find("require") {
            let before_ok = rest[..at]
                .chars()
                .next_back()
                .map_or(true, |c| !(c.is_alphanumeric() || c == '_' || c == '.'));
            let after = rest[at + "require".len()..].trim_start();
            rest = &rest[at + "require".len()..];
            if !before_ok {
                continue;
            }
            let after = after
                .strip_prefix('(')
                .map(str::trim_start)
                .unwrap_or(after);
            let Some(quote) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                continue;
            };
            let body = &after[1..];
            let Some(end) = body.find(quote) else {
                continue;
            };
            let name = &body[..end];
            if name.is_empty()
                || PROVIDED_MODULES.contains(&name)
                || missing.iter().any(|m| m == name)
            {
                continue;
            }
            if !local_module(name) {
                missing.push(name.into());
            }
        }
    }
    missing
}
