//! Keyboard mapping and app shortcut separation for the GPUI live terminal.
//!
//! Guarantees:
//! - Never fires app shortcuts, Option-as-Meta, or raw PTY writes while IME composition is active.
//! - Lets unmodified / Shift-only printable keys and Space propagate to GPUI's platform
//!   `NSTextInputContext` (or Wayland text-input) so English typing and Japanese IME / macSKK
//!   both work natively through `EntityInputHandler`.
//! - Leaves `Ctrl+C`, `Ctrl+D`, `Ctrl+U`, `Ctrl+R`, `Ctrl+A`, `Ctrl+E`, `Ctrl+[`, `Escape`,
//!   `Tab`, and `Shift+Tab` unhijacked for shells, Neovim, and Codex.

use ghostty_vt::key::{Key, Mods};
use gpui::Keystroke;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalAppAction {
    CopySelection,
    Paste,
    SelectAll,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    CycleFont,
    ToggleNerdFallback,
    RestartSession,
    ToggleRenderCache,
    Quit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyDispatchDecision {
    /// Let `NSTextInputContext` / `InputHandler` handle this keystroke (e.g. printable keys, IME).
    PassToIme,
    /// Suppress without forwarding to PTY (e.g. IME composing fallback).
    ConsumeNoPty,
    /// Cancel stuck local IME mark without sending Escape to PTY.
    CancelStuckImeMark,
    /// Execute a terminal application shortcut.
    AppAction(TerminalAppAction),
    /// Encode key through `libghostty-vt` key encoder and send to PTY.
    SendToTerminal {
        key: Key,
        mods: Mods,
        utf8: Option<String>,
        unshifted_codepoint: Option<char>,
    },
}

/// Evaluates how a `KeyDownEvent` should be routed, respecting IME composition and shortcuts.
pub fn classify_keydown(
    keystroke: &Keystroke,
    is_composing: bool,
    has_selection: bool,
    mac_option_as_meta: bool,
) -> KeyDispatchDecision {
    if is_composing {
        if keystroke.key == "escape"
            && !keystroke.modifiers.control
            && !keystroke.modifiers.alt
            && !keystroke.modifiers.platform
        {
            return KeyDispatchDecision::CancelStuckImeMark;
        }
        return KeyDispatchDecision::ConsumeNoPty;
    }

    if let Some(action) = match_app_action(keystroke, has_selection) {
        return KeyDispatchDecision::AppAction(action);
    }

    // Ignore bare modifier keysyms if any arrive
    if matches!(
        keystroke.key.as_str(),
        "shift" | "control" | "alt" | "platform" | "function" | "capslock"
    ) {
        return KeyDispatchDecision::ConsumeNoPty;
    }

    // Never swallow Command/Super shortcuts that were not matched above (let OS handle Cmd+H, Cmd+Tab, etc.)
    if keystroke.modifiers.platform {
        return KeyDispatchDecision::PassToIme;
    }

    // Unmodified or Shift-only printable keys + Space go through NSTextInputContext / IME so
    // macOS IME / macSKK can intercept romaji/kana, dead keys, and layouts cleanly.
    let no_ctrl_alt_fn =
        !keystroke.modifiers.control && !keystroke.modifiers.alt && !keystroke.modifiers.function;
    if no_ctrl_alt_fn && is_ime_text_candidate(keystroke) {
        return KeyDispatchDecision::PassToIme;
    }

    // If Option-as-Meta is disabled on macOS and only Alt(+Shift) is pressed on a printable key,
    // allow NSTextInputContext to produce the composed Option character (e.g. Option+3 -> £).
    if cfg!(target_os = "macos")
        && !mac_option_as_meta
        && keystroke.modifiers.alt
        && !keystroke.modifiers.control
        && !keystroke.modifiers.platform
        && !keystroke.modifiers.function
        && is_ime_text_candidate(keystroke)
    {
        return KeyDispatchDecision::PassToIme;
    }

    let key = gpui_key_to_ghostty(&keystroke.key);
    if key == Key::Unidentified && keystroke.key_char.is_none() {
        return KeyDispatchDecision::PassToIme;
    }

    let mods = gpui_mods_to_ghostty(&keystroke.modifiers);
    let unshifted_codepoint = unshifted_char_for_key(&keystroke.key);

    // Per ghostty-vt docs: do NOT pass C0 control characters or macOS PUA function codes as utf8.
    let utf8 = if keystroke.modifiers.control
        || keystroke.modifiers.alt
        || keystroke.modifiers.function
        || is_non_printing_named_key(&keystroke.key)
    {
        None
    } else {
        keystroke.key_char.clone().filter(|s| {
            !s.is_empty()
                && s.chars()
                    .all(|ch| !ch.is_control() && !(0xF700..=0xF8FF).contains(&(ch as u32)))
        })
    };

    KeyDispatchDecision::SendToTerminal {
        key,
        mods,
        utf8,
        unshifted_codepoint,
    }
}

pub fn match_app_action(keystroke: &Keystroke, has_selection: bool) -> Option<TerminalAppAction> {
    let m = &keystroke.modifiers;
    let key = keystroke.key.as_str();

    #[cfg(target_os = "macos")]
    {
        if !m.platform || m.control || m.alt {
            return None;
        }
        if m.shift {
            return match key {
                "+" | "=" => Some(TerminalAppAction::ZoomIn),
                "f" => Some(TerminalAppAction::CycleFont),
                "n" => Some(TerminalAppAction::ToggleNerdFallback),
                "r" => Some(TerminalAppAction::RestartSession),
                "c" => Some(TerminalAppAction::ToggleRenderCache),
                _ => None,
            };
        }
        match key {
            "c" if has_selection => Some(TerminalAppAction::CopySelection),
            "v" => Some(TerminalAppAction::Paste),
            "a" => Some(TerminalAppAction::SelectAll),
            "=" | "+" => Some(TerminalAppAction::ZoomIn),
            "-" => Some(TerminalAppAction::ZoomOut),
            "0" => Some(TerminalAppAction::ZoomReset),
            "q" => Some(TerminalAppAction::Quit),
            _ => None,
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        if !m.control || m.alt || m.platform {
            return None;
        }
        if m.shift {
            return match key {
                "c" if has_selection => Some(TerminalAppAction::CopySelection),
                "v" => Some(TerminalAppAction::Paste),
                "a" => Some(TerminalAppAction::SelectAll),
                "+" | "=" => Some(TerminalAppAction::ZoomIn),
                "f" => Some(TerminalAppAction::CycleFont),
                "n" => Some(TerminalAppAction::ToggleNerdFallback),
                "r" => Some(TerminalAppAction::RestartSession),
                _ => None,
            };
        }
        match key {
            "=" | "+" => Some(TerminalAppAction::ZoomIn),
            "-" => Some(TerminalAppAction::ZoomOut),
            "0" => Some(TerminalAppAction::ZoomReset),
            _ => None,
        }
    }
}

fn is_non_printing_named_key(key: &str) -> bool {
    matches!(
        key,
        "enter"
            | "tab"
            | "backspace"
            | "delete"
            | "escape"
            | "up"
            | "down"
            | "left"
            | "right"
            | "home"
            | "end"
            | "pageup"
            | "pagedown"
            | "insert"
            | "f1"
            | "f2"
            | "f3"
            | "f4"
            | "f5"
            | "f6"
            | "f7"
            | "f8"
            | "f9"
            | "f10"
            | "f11"
            | "f12"
            | "f13"
            | "f14"
            | "f15"
            | "f16"
            | "f17"
            | "f18"
            | "f19"
            | "f20"
            | "f21"
            | "f22"
            | "f23"
            | "f24"
    )
}

fn is_ime_text_candidate(keystroke: &Keystroke) -> bool {
    if is_non_printing_named_key(&keystroke.key) {
        return false;
    }
    keystroke.key == "space" || keystroke.key_char.is_some() || keystroke.key.chars().count() == 1
}

pub fn gpui_mods_to_ghostty(mods: &gpui::Modifiers) -> Mods {
    let mut out = Mods::empty();
    if mods.shift {
        out |= Mods::SHIFT;
    }
    if mods.alt {
        out |= Mods::ALT;
    }
    if mods.control {
        out |= Mods::CTRL;
    }
    if mods.platform {
        out |= Mods::SUPER;
    }
    out
}

pub fn gpui_key_to_ghostty(key: &str) -> Key {
    match key {
        "a" => Key::A,
        "b" => Key::B,
        "c" => Key::C,
        "d" => Key::D,
        "e" => Key::E,
        "f" => Key::F,
        "g" => Key::G,
        "h" => Key::H,
        "i" => Key::I,
        "j" => Key::J,
        "k" => Key::K,
        "l" => Key::L,
        "m" => Key::M,
        "n" => Key::N,
        "o" => Key::O,
        "p" => Key::P,
        "q" => Key::Q,
        "r" => Key::R,
        "s" => Key::S,
        "t" => Key::T,
        "u" => Key::U,
        "v" => Key::V,
        "w" => Key::W,
        "x" => Key::X,
        "y" => Key::Y,
        "z" => Key::Z,
        "0" | ")" => Key::Digit0,
        "1" | "!" => Key::Digit1,
        "2" | "@" => Key::Digit2,
        "3" | "#" => Key::Digit3,
        "4" | "$" => Key::Digit4,
        "5" | "%" => Key::Digit5,
        "6" | "^" => Key::Digit6,
        "7" | "&" => Key::Digit7,
        "8" | "*" => Key::Digit8,
        "9" | "(" => Key::Digit9,
        "`" | "~" => Key::Backquote,
        "-" | "_" => Key::Minus,
        "=" | "+" => Key::Equal,
        "[" | "{" => Key::BracketLeft,
        "]" | "}" => Key::BracketRight,
        "\\" | "|" => Key::Backslash,
        ";" | ":" => Key::Semicolon,
        "'" | "\"" => Key::Quote,
        "," | "<" => Key::Comma,
        "." | ">" => Key::Period,
        "/" | "?" => Key::Slash,
        "space" | " " => Key::Space,
        "enter" => Key::Enter,
        "tab" => Key::Tab,
        "backspace" => Key::Backspace,
        "delete" => Key::Delete,
        "escape" => Key::Escape,
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "f1" => Key::F1,
        "f2" => Key::F2,
        "f3" => Key::F3,
        "f4" => Key::F4,
        "f5" => Key::F5,
        "f6" => Key::F6,
        "f7" => Key::F7,
        "f8" => Key::F8,
        "f9" => Key::F9,
        "f10" => Key::F10,
        "f11" => Key::F11,
        "f12" => Key::F12,
        "f13" => Key::F13,
        "f14" => Key::F14,
        "f15" => Key::F15,
        "f16" => Key::F16,
        "f17" => Key::F17,
        "f18" => Key::F18,
        "f19" => Key::F19,
        "f20" => Key::F20,
        "f21" => Key::F21,
        "f22" => Key::F22,
        "f23" => Key::F23,
        "f24" => Key::F24,
        _ => Key::Unidentified,
    }
}

fn unshifted_char_for_key(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let ch = chars.next()?;
    if chars.next().is_some() {
        return match key {
            "space" => Some(' '),
            _ => None,
        };
    }
    Some(match ch {
        ')' => '0',
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        '~' => '`',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Modifiers;

    #[test]
    fn printable_keys_pass_to_ime_while_ctrl_alt_and_special_keys_encode() {
        let plain_a = Keystroke {
            modifiers: Modifiers::none(),
            key: "a".into(),
            key_char: Some("a".into()),
        };
        assert_eq!(
            classify_keydown(&plain_a, false, false, true),
            KeyDispatchDecision::PassToIme
        );

        let shift_j = Keystroke {
            modifiers: Modifiers {
                shift: true,
                ..Modifiers::none()
            },
            key: "j".into(),
            key_char: Some("J".into()),
        };
        assert_eq!(
            classify_keydown(&shift_j, false, false, true),
            KeyDispatchDecision::PassToIme
        );

        let ctrl_c = Keystroke {
            modifiers: Modifiers {
                control: true,
                ..Modifiers::none()
            },
            key: "c".into(),
            key_char: None,
        };
        assert!(matches!(
            classify_keydown(&ctrl_c, false, true, true),
            KeyDispatchDecision::SendToTerminal {
                key: Key::C,
                mods,
                ..
            } if mods == Mods::CTRL
        ));

        let opt_b = Keystroke {
            modifiers: Modifiers {
                alt: true,
                ..Modifiers::none()
            },
            key: "b".into(),
            key_char: Some("∫".into()),
        };
        assert!(matches!(
            classify_keydown(&opt_b, false, false, true),
            KeyDispatchDecision::SendToTerminal {
                key: Key::B,
                mods,
                utf8: None,
                ..
            } if mods == Mods::ALT
        ));
    }

    #[test]
    fn composing_suppresses_shortcuts_option_meta_and_pty_writes() {
        let cmd_c = Keystroke {
            modifiers: Modifiers::command(),
            key: "c".into(),
            key_char: None,
        };
        assert_eq!(
            classify_keydown(&cmd_c, true, true, true),
            KeyDispatchDecision::ConsumeNoPty
        );

        let enter = Keystroke {
            modifiers: Modifiers::none(),
            key: "enter".into(),
            key_char: Some("\n".into()),
        };
        assert_eq!(
            classify_keydown(&enter, true, false, true),
            KeyDispatchDecision::ConsumeNoPty
        );

        let escape = Keystroke {
            modifiers: Modifiers::none(),
            key: "escape".into(),
            key_char: None,
        };
        assert_eq!(
            classify_keydown(&escape, true, false, true),
            KeyDispatchDecision::CancelStuckImeMark
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cmd_q_quits_only_outside_ime_composition() {
        let cmd_q = Keystroke {
            modifiers: Modifiers::command(),
            key: "q".into(),
            key_char: None,
        };
        assert_eq!(
            classify_keydown(&cmd_q, true, false, true),
            KeyDispatchDecision::ConsumeNoPty
        );
        assert_eq!(
            classify_keydown(&cmd_q, false, false, true),
            KeyDispatchDecision::AppAction(TerminalAppAction::Quit)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn cmd_c_copies_only_when_selection_active_and_cmd_w_is_not_hijacked() {
        let cmd_c = Keystroke {
            modifiers: Modifiers::command(),
            key: "c".into(),
            key_char: None,
        };
        assert_eq!(
            classify_keydown(&cmd_c, false, true, true),
            KeyDispatchDecision::AppAction(TerminalAppAction::CopySelection)
        );
        assert_eq!(
            classify_keydown(&cmd_c, false, false, true),
            KeyDispatchDecision::PassToIme
        );
        let cmd_w = Keystroke { key: "w".into(), ..cmd_c };
        assert_eq!(
            classify_keydown(&cmd_w, false, true, true),
            KeyDispatchDecision::PassToIme
        );
    }
    #[cfg(not(target_os = "macos"))]
    #[test]
    fn ctrl_shift_c_copies_selection_and_ctrl_c_reaches_the_pty() {
        let mut keystroke = Keystroke {
            modifiers: Modifiers { control: true, shift: true, ..Default::default() },
            key: "c".into(),
            key_char: None,
        };
        assert_eq!(
            classify_keydown(&keystroke, false, true, false),
            KeyDispatchDecision::AppAction(TerminalAppAction::CopySelection),
        );
        keystroke.modifiers.shift = false;
        assert!(matches!(
            classify_keydown(&keystroke, false, true, false),
            KeyDispatchDecision::SendToTerminal { .. },
        ));
    }

}
