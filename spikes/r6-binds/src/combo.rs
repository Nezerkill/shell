//! Key combos: shell-side spelling -> niri KDL spelling.
//!
//! Facts established by `niri validate` (26.04), see RESULT.md:
//!   * a combo is `<modifier>+...+<XKB key name>`, see the binds comment of the
//!     built-in default config;
//!   * niri accepts the modifiers Mod, Ctrl/Control, Alt, Shift, Super, Win,
//!     ISO_Level3_Shift, ISO_Level5_Shift — and rejects Meta/Hyper/Logo/Mod4;
//!   * modifier order is irrelevant: `Mod+Shift+Q` and `Shift+Mod+Q` are the
//!     *same* bind, so niri reports the second one as a duplicate;
//!   * `Super` is a literal modifier, not a synonym of `Mod`: `Mod+Return` and
//!     `Super+Return` are two different binds.

/// Shell-side modifier name (upper-case) -> niri modifier, in output order.
const ALIASES: &[(&str, Modifier)] = &[
    ("SUPER", Modifier::Mod),
    ("META", Modifier::Mod),
    ("WIN", Modifier::Mod),
    ("LOGO", Modifier::Mod),
    ("MOD4", Modifier::Mod),
    ("MOD", Modifier::Mod),
    ("CTRL", Modifier::Ctrl),
    ("CONTROL", Modifier::Ctrl),
    ("ALT", Modifier::Alt),
    ("SHIFT", Modifier::Shift),
];

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Modifier {
    Mod,
    Ctrl,
    Alt,
    Shift,
}

impl Modifier {
    fn kdl(self) -> &'static str {
        match self {
            Modifier::Mod => "Mod",
            Modifier::Ctrl => "Ctrl",
            Modifier::Alt => "Alt",
            Modifier::Shift => "Shift",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub modifiers: Vec<Modifier>,
    pub key: String,
}

impl Combo {
    /// Parses `SUPER+SHIFT+Q` (shell spelling) into a canonical niri combo.
    pub fn parse(input: &str) -> Result<Combo, String> {
        let mut parts: Vec<&str> = input.split('+').map(str::trim).collect();
        // A literal `+` key would be spelled `PLUS`, so an empty piece is a typo.
        if parts.iter().any(|p| p.is_empty()) {
            return Err(format!("empty component in combo `{input}`"));
        }
        let key = parts.pop().expect("at least one component").to_string();
        let mut modifiers = Vec::new();
        for raw in parts {
            let upper = raw.to_ascii_uppercase();
            let m = ALIASES
                .iter()
                .find(|(alias, _)| *alias == upper)
                .map(|(_, m)| *m)
                .ok_or_else(|| {
                    let known: Vec<&str> = ALIASES.iter().map(|(a, _)| *a).collect();
                    format!(
                        "unknown modifier `{raw}` in `{input}` (known: {})",
                        known.join(", ")
                    )
                })?;
            if !modifiers.contains(&m) {
                modifiers.push(m);
            }
        }
        if modifiers.is_empty() && !is_media_key(&key) {
            // niri accepts a bare key, but a shell hotkey without a modifier
            // would swallow the whole keyboard: refuse it here. Media keys
            // (XF86*) are the exception — the default niri config binds them raw.
            return Err(format!(
                "combo `{input}` has no modifier (shell requires at least one, except for XF86*/Print media keys)"
            ));
        }
        modifiers.sort();
        Ok(Combo { modifiers, key })
    }

    /// niri spelling, e.g. `Mod+Shift+Q`.
    pub fn kdl(&self) -> String {
        let mut out = String::new();
        for m in &self.modifiers {
            out.push_str(m.kdl());
            out.push('+');
        }
        out.push_str(&self.key);
        out
    }
}

fn is_media_key(key: &str) -> bool {
    key.starts_with("XF86") || key == "Print"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn super_becomes_mod() {
        assert_eq!(Combo::parse("SUPER+Return").unwrap().kdl(), "Mod+Return");
        assert_eq!(Combo::parse("Super+Shift+Q").unwrap().kdl(), "Mod+Shift+Q");
    }

    #[test]
    fn media_keys_may_be_unmodified() {
        assert_eq!(Combo::parse("XF86AudioMute").unwrap().kdl(), "XF86AudioMute");
    }

    #[test]
    fn order_is_canonical() {
        assert_eq!(Combo::parse("SHIFT+SUPER+Q").unwrap().kdl(), "Mod+Shift+Q");
        assert_eq!(
            Combo::parse("SHIFT+SUPER+Q").unwrap().kdl(),
            Combo::parse("Super+Shift+Q").unwrap().kdl()
        );
    }

    #[test]
    fn control_spellings_collide() {
        assert_eq!(Combo::parse("CTRL+ALT+Delete").unwrap().kdl(), "Ctrl+Alt+Delete");
        assert_eq!(
            Combo::parse("CONTROL+ALT+Delete").unwrap().kdl(),
            "Ctrl+Alt+Delete"
        );
    }

    #[test]
    fn rejects_unmodified_and_unknown() {
        assert!(Combo::parse("Return").is_err());
        assert!(Combo::parse("HYPER+Return").is_err());
        assert!(Combo::parse("SUPER++Return").is_err());
    }
}
