//! Launcher manifest schema, identities and relative-path validation.
use super::ID_MAX_LEN;
use crate::formats::manifest_json::{self as json, Value};
use alloc::{format, string::String};

/// `app_registry_id_is_valid`: 1..=63 of `[A-Za-z0-9_-]`.
pub fn id_is_valid(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= ID_MAX_LEN
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// `relative_path_is_valid`: non-empty, not absolute, no `..`, no `\`.
pub fn relative_path_is_valid(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.contains("..") && !path.contains('\\')
}

/// A parsed manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub id: String,
    pub entry: String,
    pub icon: Option<String>,
    pub display_name: Option<String>,
    pub args_json: String,
    pub order: i32,
    pub visible: bool,
}

/// Validate `launcher.json` for the app directory named `dir_name`.
pub fn parse_manifest(text: &str, dir_name: &str) -> Result<Manifest, String> {
    let root = json::parse(text)?;
    if !matches!(root.value, Value::Object(_)) {
        return Err("manifest is not an object".into());
    }
    let int = |v: &Value| match *v {
        Value::Number(n) if n == (n as i32) as f64 => Some(n as i32),
        _ => None,
    };
    let string = |key: &str| -> Result<Option<String>, String> {
        match root.get(key).map(|v| &v.value) {
            None => Ok(None),
            Some(Value::Str(s)) => Ok(Some(s.clone())),
            Some(_) => Err(format!("{key} is not a string")),
        }
    };
    match root.get("schema_version").and_then(|v| int(&v.value)) {
        Some(1) => {}
        _ => return Err("schema_version is not 1".into()),
    }
    let id = string("id")?.ok_or("id missing")?;
    if !id_is_valid(&id) || id != dir_name {
        return Err(format!("id {id:?} does not match the directory"));
    }
    let entry = string("entry")?.ok_or("entry missing")?;
    if !relative_path_is_valid(&entry) || !entry.ends_with(".lua") {
        return Err(format!("bad entry {entry:?}"));
    }
    let icon = string("icon")?;
    if let Some(icon) = &icon {
        if !relative_path_is_valid(icon) || !(icon.ends_with(".jpg") || icon.ends_with(".jpeg")) {
            return Err(format!("bad icon {icon:?}"));
        }
    }
    let display_name = string("display_name")?;
    if display_name.as_deref() == Some("") {
        return Err("display_name is empty".into());
    }
    let args_json = match root.get("args") {
        None => "{}".into(),
        Some(v) if matches!(v.value, Value::Object(_)) => text[v.start..v.end].into(),
        Some(_) => return Err("args is not an object".into()),
    };
    let order = match root.get("order") {
        None => 0,
        Some(v) => int(&v.value).ok_or("order is not an integer")?,
    };
    let visible = match root.get("visible").map(|v| &v.value) {
        None => true,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err("visible is not a boolean".into()),
    };
    Ok(Manifest {
        id,
        entry,
        icon,
        display_name,
        args_json,
        order,
        visible,
    })
}
