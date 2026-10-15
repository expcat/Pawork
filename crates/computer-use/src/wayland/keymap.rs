//! Fixed XKB keymap for the dedicated virtual keyboard (CU-26).
//!
//! Every printable ASCII character owns one keycode with a single-level
//! keysym, so text never depends on a Shift level or on the guest's layout;
//! named keys and the four modifiers follow. Characters outside the table
//! are rejected before any event is sent.

/// Evdev keycode of the first table entry (XKB keycode 9).
const FIRST_KEY: u32 = 1;
const MODIFIER_KEYS: [(&str, &str, &str); 4] = [
    ("LFSH", "Shift_L", "Shift"),
    ("LCTL", "Control_L", "Control"),
    ("LALT", "Alt_L", "Mod1"),
    ("LWIN", "Super_L", "Mod4"),
];

/// Non-printing keys the seat can press; the same vocabulary as the
/// isolated desktop's key names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeatKey {
    Return,
    Tab,
    Space,
    Backspace,
    Escape,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    Left,
    Up,
    Right,
    Down,
}

const NAMED_KEYS: [(SeatKey, &str); 14] = [
    (SeatKey::Return, "Return"),
    (SeatKey::Tab, "Tab"),
    (SeatKey::Space, "space"),
    (SeatKey::Backspace, "BackSpace"),
    (SeatKey::Escape, "Escape"),
    (SeatKey::Delete, "Delete"),
    (SeatKey::Home, "Home"),
    (SeatKey::End, "End"),
    (SeatKey::PageUp, "Prior"),
    (SeatKey::PageDown, "Next"),
    (SeatKey::Left, "Left"),
    (SeatKey::Up, "Up"),
    (SeatKey::Right, "Right"),
    (SeatKey::Down, "Down"),
];

impl SeatKey {
    pub fn from_name(name: &str) -> Option<Self> {
        let key = match name.to_ascii_lowercase().as_str() {
            "return" | "enter" => Self::Return,
            "tab" => Self::Tab,
            "space" => Self::Space,
            "backspace" => Self::Backspace,
            "escape" | "esc" => Self::Escape,
            "delete" => Self::Delete,
            "home" => Self::Home,
            "end" => Self::End,
            "pageup" => Self::PageUp,
            "pagedown" => Self::PageDown,
            "left" => Self::Left,
            "up" => Self::Up,
            "right" => Self::Right,
            "down" => Self::Down,
            _ => return None,
        };
        Some(key)
    }
}

/// Modifier state for one key press. Command maps to Super, Option to Alt,
/// matching the isolated desktop's Linux mapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SeatModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl SeatModifiers {
    /// Real-modifier mask for `zwp_virtual_keyboard_v1.modifiers`; the
    /// keymap's modifier_map pins Alt to Mod1 and Super to Mod4.
    pub(crate) fn mask(self) -> u32 {
        (self.shift as u32)
            | (self.control as u32) << 2
            | (self.alt as u32) << 3
            | (self.super_key as u32) << 6
    }
}

const PUNCTUATION: [(char, &str); 33] = [
    (' ', "space"),
    ('!', "exclam"),
    ('"', "quotedbl"),
    ('#', "numbersign"),
    ('$', "dollar"),
    ('%', "percent"),
    ('&', "ampersand"),
    ('\'', "apostrophe"),
    ('(', "parenleft"),
    (')', "parenright"),
    ('*', "asterisk"),
    ('+', "plus"),
    (',', "comma"),
    ('-', "minus"),
    ('.', "period"),
    ('/', "slash"),
    (':', "colon"),
    (';', "semicolon"),
    ('<', "less"),
    ('=', "equal"),
    ('>', "greater"),
    ('?', "question"),
    ('@', "at"),
    ('[', "bracketleft"),
    ('\\', "backslash"),
    (']', "bracketright"),
    ('^', "asciicircum"),
    ('_', "underscore"),
    ('`', "grave"),
    ('{', "braceleft"),
    ('|', "bar"),
    ('}', "braceright"),
    ('~', "asciitilde"),
];

fn char_keysym(c: char) -> Option<String> {
    if c.is_ascii_alphanumeric() {
        return Some(c.to_string());
    }
    PUNCTUATION
        .iter()
        .find(|(candidate, _)| *candidate == c)
        .map(|(_, name)| (*name).to_string())
}

/// Evdev keycode for a printable ASCII character; None for anything else
/// (control characters, non-ASCII). Table order is 0x20..=0x7e.
pub(crate) fn char_keycode(c: char) -> Option<u32> {
    if (' '..='~').contains(&c) {
        Some(FIRST_KEY + (c as u32 - 0x20))
    } else {
        None
    }
}

pub(crate) fn key_keycode(key: SeatKey) -> u32 {
    let index = NAMED_KEYS
        .iter()
        .position(|(candidate, _)| *candidate == key)
        .expect("every SeatKey has a table entry");
    FIRST_KEY + 95 + index as u32
}

/// The NUL-free keymap text; the caller appends the terminating NUL the
/// protocol requires.
pub(crate) fn keymap_text() -> String {
    let mut entries: Vec<(String, String)> = (0x20u8..=0x7e)
        .map(|byte| {
            let c = byte as char;
            (
                format!("K{}", char_keycode(c).expect("printable")),
                char_keysym(c).expect("printable ASCII has a keysym"),
            )
        })
        .collect();
    for (key, keysym) in NAMED_KEYS {
        entries.push((format!("K{}", key_keycode(key)), keysym.to_string()));
    }
    let modifier_base = FIRST_KEY + 95 + NAMED_KEYS.len() as u32;
    let mut text = String::from("xkb_keymap {\nxkb_keycodes \"pawork\" {\nminimum = 8;\n");
    text.push_str(&format!(
        "maximum = {};\n",
        modifier_base + MODIFIER_KEYS.len() as u32 + 8
    ));
    for (name, _) in &entries {
        let evdev: u32 = name[1..].parse().expect("generated name");
        text.push_str(&format!("<{name}> = {};\n", evdev + 8));
    }
    for (offset, (name, _, _)) in MODIFIER_KEYS.iter().enumerate() {
        text.push_str(&format!(
            "<{name}> = {};\n",
            modifier_base + offset as u32 + 8
        ));
    }
    text.push_str("};\nxkb_types \"pawork\" { include \"complete\" };\n");
    text.push_str("xkb_compat \"pawork\" { include \"complete\" };\n");
    text.push_str("xkb_symbols \"pawork\" {\n");
    for (name, keysym) in &entries {
        text.push_str(&format!("key <{name}> {{ [ {keysym} ] }};\n"));
    }
    for (name, keysym, modifier) in MODIFIER_KEYS {
        text.push_str(&format!("key <{name}> {{ [ {keysym} ] }};\n"));
        text.push_str(&format!("modifier_map {modifier} {{ <{name}> }};\n"));
    }
    text.push_str("};\n};\n");
    text
}
