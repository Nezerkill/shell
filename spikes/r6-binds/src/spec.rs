//! The declarative spec read by the generator.
//!
//! Shape mirrors the shell config model (`docs/architecture/0002-config-model.md`
//! §7: hotkeys live in the compositor config, the shell only generates a
//! fragment; `docs/architecture/0003-compositor.md` §6: `binds: { action = "SUPER+b" }`).
//! A bind is a map entry `id -> { keys, do }`, so `binds.terminal.keys = "SUPER+Return"`.

use crate::combo::Combo;
use crate::json::Value;

#[derive(Debug, Clone)]
pub struct Spec {
    /// Emit `hotkey-overlay { skip-at-startup }`.
    pub skip_hotkey_overlay_at_startup: bool,
    /// Program used for shell IPC binds; `shellctl` by default.
    pub ipc_command: Vec<String>,
    /// Binds in spec order (output order is deterministic).
    pub binds: Vec<Bind>,
}

#[derive(Debug, Clone)]
pub struct Bind {
    /// Spec key, used in comments and conflict messages only.
    pub id: String,
    pub combo: Combo,
    pub action: Action,
    /// `repeat=false` when false, omitted when None.
    pub repeat: Option<bool>,
    /// `cooldown-ms=N`.
    pub cooldown_ms: Option<u32>,
    /// `allow-when-locked=true` (niri only accepts it on spawn binds).
    pub allow_when_locked: bool,
    /// `hotkey-overlay-title="..."` / `=null`; omitted when None.
    pub overlay_title: Option<OverlayTitle>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum OverlayTitle {
    Text(String),
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `spawn "prog" "arg"...`
    Spawn(Vec<String>),
    /// `spawn-sh "one shell line"`
    SpawnSh(String),
    /// A niri action with an optional argument list: `focus-workspace 2`.
    Niri { action: String, args: Vec<String> },
    /// Shell IPC: `spawn "shellctl" "call" "<call>"`.
    Ipc { call: String },
}

impl Action {
    pub fn kind(&self) -> &'static str {
        match self {
            Action::Spawn(_) => "spawn",
            Action::SpawnSh(_) => "spawn-sh",
            Action::Niri { .. } => "niri",
            Action::Ipc { .. } => "ipc",
        }
    }
}

pub fn load(text: &str) -> Result<Spec, String> {
    let root = crate::json::parse(text)?;
    if !matches!(root, Value::Object(_)) {
        return Err(format!("top level must be an object, got {}", root.type_name()));
    }

    let skip_hotkey_overlay_at_startup = match root.get("hotkey_overlay") {
        None => false,
        Some(v) => match v.get("skip_at_startup") {
            None => false,
            Some(b) => b.as_bool().ok_or_else(|| {
                format!(
                    "hotkey_overlay.skip_at_startup must be bool, got {}",
                    b.type_name()
                )
            })?,
        },
    };

    let ipc_command = match root.get("ipc_command") {
        None => vec!["shellctl".to_string()],
        Some(v) => string_array(v, "ipc_command")?,
    };
    if ipc_command.is_empty() {
        return Err("ipc_command must not be empty".into());
    }

    let binds_value = root.get("binds").ok_or("missing `binds`")?;
    let pairs = binds_value.as_object().ok_or_else(|| {
        format!(
            "binds must be an object keyed by bind id, got {}",
            binds_value.type_name()
        )
    })?;

    let mut binds = Vec::with_capacity(pairs.len());
    for (id, value) in pairs {
        binds.push(parse_bind(id, value)?);
    }
    if binds.is_empty() {
        return Err("binds is empty".into());
    }
    Ok(Spec {
        skip_hotkey_overlay_at_startup,
        ipc_command,
        binds,
    })
}

fn parse_bind(id: &str, value: &Value) -> Result<Bind, String> {
    if !matches!(value, Value::Object(_)) {
        return Err(format!("binds.{id} must be an object, got {}", value.type_name()));
    }
    let keys = value
        .get("keys")
        .ok_or_else(|| format!("binds.{id}: missing `keys`"))?
        .as_str()
        .ok_or_else(|| format!("binds.{id}.keys must be a string"))?
        .to_string();
    let combo = Combo::parse(&keys).map_err(|e| format!("binds.{id}: {e}"))?;

    let do_value = value
        .get("do")
        .ok_or_else(|| format!("binds.{id}: missing `do`"))?;
    let action = parse_action(id, do_value)?;

    let repeat = match value.get("repeat") {
        None => None,
        Some(v) => Some(
            v.as_bool()
                .ok_or_else(|| format!("binds.{id}.repeat must be bool"))?,
        ),
    };
    let cooldown_ms = match value.get("cooldown_ms") {
        None => None,
        Some(v) => Some(
            v.as_u32()
                .ok_or_else(|| format!("binds.{id}.cooldown_ms must be a u32"))?,
        ),
    };
    let allow_when_locked = match value.get("allow_when_locked") {
        None => false,
        Some(v) => v
            .as_bool()
            .ok_or_else(|| format!("binds.{id}.allow_when_locked must be bool"))?,
    };
    let overlay_title = match value.get("overlay_title") {
        None => None,
        Some(Value::Null) => Some(OverlayTitle::Null),
        Some(Value::Str(s)) => Some(OverlayTitle::Text(s.clone())),
        Some(other) => {
            return Err(format!(
                "binds.{id}.overlay_title must be a string or null, got {}",
                other.type_name()
            ));
        }
    };

    Ok(Bind {
        id: id.to_string(),
        combo,
        action,
        repeat,
        cooldown_ms,
        allow_when_locked,
        overlay_title,
    })
}

fn parse_action(id: &str, value: &Value) -> Result<Action, String> {
    let ty = value
        .get("type")
        .ok_or_else(|| format!("binds.{id}.do: missing `type`"))?
        .as_str()
        .ok_or_else(|| format!("binds.{id}.do.type must be a string"))?;
    match ty {
        "spawn" => {
            let command = string_array(
                value
                    .get("command")
                    .ok_or_else(|| format!("binds.{id}.do: spawn needs `command`"))?,
                &format!("binds.{id}.do.command"),
            )?;
            if command.is_empty() {
                return Err(format!("binds.{id}.do: spawn command is empty"));
            }
            Ok(Action::Spawn(command))
        }
        "spawn-sh" => {
            let command = value
                .get("command")
                .ok_or_else(|| format!("binds.{id}.do: spawn-sh needs `command`"))?
                .as_str()
                .ok_or_else(|| format!("binds.{id}.do.command must be a string for spawn-sh"))?;
            if command.trim().is_empty() {
                return Err(format!("binds.{id}.do: spawn-sh command is empty"));
            }
            Ok(Action::SpawnSh(command.to_string()))
        }
        "niri" => {
            let action = value
                .get("action")
                .ok_or_else(|| format!("binds.{id}.do: niri needs `action`"))?
                .as_str()
                .ok_or_else(|| format!("binds.{id}.do.action must be a string"))?;
            let args = match value.get("args") {
                None => Vec::new(),
                Some(v) => string_array(v, &format!("binds.{id}.do.args"))?,
            };
            Ok(Action::Niri {
                action: action.to_string(),
                args,
            })
        }
        "ipc" => {
            let call = value
                .get("call")
                .ok_or_else(|| format!("binds.{id}.do: ipc needs `call`"))?
                .as_str()
                .ok_or_else(|| format!("binds.{id}.do.call must be a string"))?;
            Ok(Action::Ipc {
                call: call.to_string(),
            })
        }
        other => Err(format!(
            "binds.{id}.do.type: unknown action type `{other}` (expected spawn, spawn-sh, niri, ipc)"
        )),
    }
}

fn string_array(value: &Value, what: &str) -> Result<Vec<String>, String> {
    let items = value
        .as_array()
        .ok_or_else(|| format!("{what} must be an array of strings, got {}", value.type_name()))?;
    items
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{what}: expected a string, got {}", v.type_name()))
        })
        .collect()
}
