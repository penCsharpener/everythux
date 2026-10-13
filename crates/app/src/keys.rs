//! Parsing of the `keybindings` strings from `config.json` into egui shortcuts and click bindings.

use egui::{Key, KeyboardShortcut, Modifiers, PointerButton};
use everythux_core::config::Keybindings;

/// Why a binding string could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum BindingError {
    /// The string is empty.
    #[error("empty binding")]
    Empty,
    /// A part before the last `+` is not a modifier.
    #[error("unknown modifier {0:?}")]
    UnknownModifier(String),
    /// The last part is neither a key nor a mouse button.
    #[error("unknown key {0:?}")]
    UnknownKey(String),
    /// A keyboard binding was expected but a click was given, or the other way round.
    #[error("expected a {expected} binding, got {got:?}")]
    WrongKind {
        /// What the setting needs.
        expected: &'static str,
        /// The offending string.
        got: String,
    },
}

/// A mouse binding such as `Ctrl+Shift+LeftClick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClickBinding {
    /// Modifiers that must be held (exactly).
    pub(crate) modifiers: Modifiers,
    /// Which button.
    pub(crate) button: PointerButton,
}

impl ClickBinding {
    /// Whether a click with `button` while `held` modifiers are down triggers this binding.
    pub(crate) fn matches(self, button: PointerButton, held: Modifiers) -> bool {
        button == self.button && held.matches_exact(self.modifiers)
    }
}

/// Either half of a binding string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parsed {
    Key(KeyboardShortcut),
    Click(ClickBinding),
}

fn parse_modifier(part: &str, modifiers: &mut Modifiers) -> Result<(), BindingError> {
    match part.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => modifiers.ctrl = true,
        "shift" => modifiers.shift = true,
        "alt" | "option" => modifiers.alt = true,
        "cmd" | "command" | "meta" | "super" => {
            modifiers.command = true;
            modifiers.ctrl = true;
        }
        _ => return Err(BindingError::UnknownModifier(part.to_owned())),
    }
    Ok(())
}

fn parse_button(name: &str) -> Option<PointerButton> {
    match name.to_ascii_lowercase().as_str() {
        "leftclick" | "click" | "primaryclick" => Some(PointerButton::Primary),
        "rightclick" | "secondaryclick" => Some(PointerButton::Secondary),
        "middleclick" => Some(PointerButton::Middle),
        _ => None,
    }
}

fn parse(binding: &str) -> Result<Parsed, BindingError> {
    let binding = binding.trim();
    if binding.is_empty() {
        return Err(BindingError::Empty);
    }
    // A lone "+" key would break the split, so split from the right.
    let (mods_part, last) = match binding.rsplit_once('+') {
        Some((head, tail)) if !tail.is_empty() => (head, tail),
        Some((head, _)) => (head.trim_end_matches('+'), "+"),
        None => ("", binding),
    };
    let mut modifiers = Modifiers::NONE;
    for part in mods_part.split('+').map(str::trim).filter(|p| !p.is_empty()) {
        parse_modifier(part, &mut modifiers)?;
    }
    let last = last.trim();
    if let Some(button) = parse_button(last) {
        return Ok(Parsed::Click(ClickBinding { modifiers, button }));
    }
    Key::from_name(last)
        .map(|key| Parsed::Key(KeyboardShortcut::new(modifiers, key)))
        .ok_or_else(|| BindingError::UnknownKey(last.to_owned()))
}

/// Parses `"Ctrl+Shift+R"`, `"F5"`, ... into a keyboard shortcut.
pub(crate) fn parse_shortcut(binding: &str) -> Result<KeyboardShortcut, BindingError> {
    match parse(binding)? {
        Parsed::Key(shortcut) => Ok(shortcut),
        Parsed::Click(_) => Err(BindingError::WrongKind {
            expected: "keyboard",
            got: binding.to_owned(),
        }),
    }
}

/// Parses `"Ctrl+Shift+LeftClick"` into a click binding.
pub(crate) fn parse_click(binding: &str) -> Result<ClickBinding, BindingError> {
    match parse(binding)? {
        Parsed::Click(click) => Ok(click),
        Parsed::Key(_) => Err(BindingError::WrongKind {
            expected: "mouse",
            got: binding.to_owned(),
        }),
    }
}

/// Human-readable form for menus: `Ctrl+Shift+R`.
pub(crate) fn describe_shortcut(shortcut: KeyboardShortcut) -> String {
    let mut out = String::new();
    if shortcut.modifiers.ctrl || shortcut.modifiers.command {
        out.push_str("Ctrl+");
    }
    if shortcut.modifiers.alt {
        out.push_str("Alt+");
    }
    if shortcut.modifiers.shift {
        out.push_str("Shift+");
    }
    out.push_str(shortcut.logical_key.symbol_or_name());
    out
}

/// All bindings the app reacts to, parsed once from the config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bindings {
    pub(crate) reindex_all: KeyboardShortcut,
    pub(crate) quick_rescan: KeyboardShortcut,
    pub(crate) toggle_preview: KeyboardShortcut,
    pub(crate) focus_search: KeyboardShortcut,
    pub(crate) toggle_match_path: KeyboardShortcut,
    pub(crate) exclude_click: ClickBinding,
    /// One message per binding that was invalid and replaced by its default.
    pub(crate) warnings: Vec<String>,
}

impl Bindings {
    /// Parses the configured bindings; invalid ones fall back to the defaults with a warning.
    pub(crate) fn from_config(config: &Keybindings) -> Self {
        let defaults = Keybindings::default();
        let mut warnings = Vec::new();
        let mut key = |name: &str, value: &str, default: &str| {
            parse_shortcut(value).unwrap_or_else(|e| {
                warnings.push(format!(
                    "Invalid keybinding {name} = {value:?}: {e}; using {default}"
                ));
                parse_shortcut(default).unwrap_or(KeyboardShortcut::new(Modifiers::NONE, Key::F5))
            })
        };
        let reindex_all = key("reindex_all", &config.reindex_all, &defaults.reindex_all);
        let quick_rescan = key("quick_rescan", &config.quick_rescan, &defaults.quick_rescan);
        let toggle_preview = key("toggle_preview", &config.toggle_preview, &defaults.toggle_preview);
        let focus_search = key("focus_search", &config.focus_search, &defaults.focus_search);
        let toggle_match_path = key(
            "toggle_match_path",
            &config.toggle_match_path,
            &defaults.toggle_match_path,
        );
        let exclude_click = parse_click(&config.exclude_path_click).unwrap_or_else(|e| {
            warnings.push(format!(
                "Invalid keybinding exclude_path_click = {:?}: {e}; using {}",
                config.exclude_path_click, defaults.exclude_path_click
            ));
            ClickBinding {
                modifiers: Modifiers::CTRL | Modifiers::SHIFT,
                button: PointerButton::Primary,
            }
        });
        Self {
            reindex_all,
            quick_rescan,
            toggle_preview,
            focus_search,
            toggle_match_path,
            exclude_click,
            warnings,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_shortcuts() {
        assert_eq!(
            parse_shortcut("Ctrl+Shift+R"),
            Ok(KeyboardShortcut::new(Modifiers::CTRL | Modifiers::SHIFT, Key::R))
        );
        assert_eq!(
            parse_shortcut("F5"),
            Ok(KeyboardShortcut::new(Modifiers::NONE, Key::F5))
        );
        assert_eq!(
            parse_shortcut(" ctrl + p "),
            Ok(KeyboardShortcut::new(Modifiers::CTRL, Key::P))
        );
        assert_eq!(
            parse_shortcut("Alt+Enter"),
            Ok(KeyboardShortcut::new(Modifiers::ALT, Key::Enter))
        );
    }

    #[test]
    fn parses_click_bindings() {
        let click = parse_click("Ctrl+Shift+LeftClick").expect("valid");
        assert_eq!(click.modifiers, Modifiers::CTRL | Modifiers::SHIFT);
        assert_eq!(click.button, PointerButton::Primary);
        assert!(click.matches(PointerButton::Primary, Modifiers::CTRL | Modifiers::SHIFT));
        assert!(
            !click.matches(PointerButton::Primary, Modifiers::CTRL),
            "ctrl alone toggles"
        );
        assert!(!click.matches(PointerButton::Primary, Modifiers::SHIFT));
        assert!(!click.matches(PointerButton::Secondary, Modifiers::CTRL | Modifiers::SHIFT));
        let right = parse_click("Alt+RightClick").expect("valid");
        assert_eq!(right.button, PointerButton::Secondary);
    }

    #[test]
    fn rejects_garbage() {
        assert_eq!(parse_shortcut(""), Err(BindingError::Empty));
        assert_eq!(
            parse_shortcut("Hyper+R"),
            Err(BindingError::UnknownModifier("Hyper".to_owned()))
        );
        assert_eq!(
            parse_shortcut("Ctrl+Nope"),
            Err(BindingError::UnknownKey("Nope".to_owned()))
        );
        assert!(matches!(
            parse_shortcut("Ctrl+LeftClick"),
            Err(BindingError::WrongKind { .. })
        ));
        assert!(matches!(
            parse_click("Ctrl+R"),
            Err(BindingError::WrongKind { .. })
        ));
    }

    #[test]
    fn invalid_bindings_fall_back_with_a_warning() {
        let config = Keybindings {
            reindex_all: "Bogus+R".to_owned(),
            exclude_path_click: "Ctrl+R".to_owned(),
            ..Keybindings::default()
        };
        let bindings = Bindings::from_config(&config);
        assert_eq!(bindings.warnings.len(), 2, "{:?}", bindings.warnings);
        assert_eq!(
            bindings.reindex_all,
            KeyboardShortcut::new(Modifiers::CTRL | Modifiers::SHIFT, Key::R)
        );
        assert_eq!(
            bindings.exclude_click.modifiers,
            Modifiers::CTRL | Modifiers::SHIFT
        );
    }

    #[test]
    fn defaults_parse_without_warnings() {
        let bindings = Bindings::from_config(&Keybindings::default());
        assert!(bindings.warnings.is_empty(), "{:?}", bindings.warnings);
    }

    #[test]
    fn describes_shortcuts() {
        let s = KeyboardShortcut::new(Modifiers::CTRL | Modifiers::SHIFT, Key::R);
        assert_eq!(describe_shortcut(s), "Ctrl+Shift+R");
        assert_eq!(
            describe_shortcut(KeyboardShortcut::new(Modifiers::NONE, Key::F5)),
            "F5"
        );
    }
}
