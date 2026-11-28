use gpui::{Font, FontFallbacks, FontFeatures, FontStyle, FontWeight};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub const DEFAULT_FONT_SIZE: f32 = 14.0;
pub const DEFAULT_LINE_HEIGHT: f32 = 1.25;
pub const MIN_FONT_SIZE: f32 = 8.0;
pub const MAX_FONT_SIZE: f32 = 48.0;

/// Nerd Font / symbol PUA fallback chain (aligned with src/terminal/fonts.ts).
pub const NERD_FONT_FALLBACKS: &[&str] = &[
    "Symbols Nerd Font Mono",
    "Symbols Nerd Font",
    "UDEV Gothic 35NF",
    "UDEV Gothic NF",
    "JetBrainsMono Nerd Font Mono",
    "Hack Nerd Font Mono",
    "MesloLGS NF",
    "FiraCode Nerd Font Mono",
    "CaskaydiaCove Nerd Font Mono",
];

/// Japanese monospace/gothic fallback chain for macOS and Linux.
pub const JAPANESE_FONT_FALLBACKS: &[&str] = &[
    "Hiragino Sans",
    "Hiragino Kaku Gothic ProN",
    "Noto Sans Mono CJK JP",
    "Noto Sans JP",
    "BIZ UDGothic",
];

/// System color emoji fallback chain.
pub const EMOJI_FONT_FALLBACKS: &[&str] = &["Apple Color Emoji", "Noto Color Emoji"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontPreset {
    Default,
    UdevGothic35Nf,
    Menlo,
}

impl FontPreset {
    pub const ALL: [FontPreset; 3] = [
        FontPreset::Default,
        FontPreset::UdevGothic35Nf,
        FontPreset::Menlo,
    ];

    pub fn from_cli_name(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "default" => Some(FontPreset::Default),
            "udev" | "udev-gothic" | "udev35nf" | "udev-gothic-35nf" => {
                Some(FontPreset::UdevGothic35Nf)
            }
            "menlo" => Some(FontPreset::Menlo),
            _ => None,
        }
    }

    pub fn next(self) -> Self {
        match self {
            FontPreset::Default => FontPreset::UdevGothic35Nf,
            FontPreset::UdevGothic35Nf => FontPreset::Menlo,
            FontPreset::Menlo => FontPreset::Default,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FontPreset::Default => "Default",
            FontPreset::UdevGothic35Nf => "UDEV Gothic 35NF",
            FontPreset::Menlo => "Menlo",
        }
    }

    pub fn primary_family(self) -> &'static str {
        match self {
            // Note: macOS CoreText does not expose "SF Mono" by family name to GPUI;
            // passing "SF Mono" silently falls back to proportional Helvetica.
            // Use Menlo (always installed on macOS) as the safe default monospace primary.
            FontPreset::Default => "Menlo",
            FontPreset::UdevGothic35Nf => "UDEV Gothic 35NF",
            FontPreset::Menlo => "Menlo",
        }
    }
}

#[cfg(any(target_os = "linux", test))]
const LINUX_MONOSPACE: &[&str] = &[
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Noto Sans Mono",
    "JetBrains Mono",
];

#[cfg(target_os = "linux")]
static LINUX_FONT_FAMILIES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

#[cfg(target_os = "linux")]
pub fn init_linux_font_families(families: Vec<String>) {
    let _ = LINUX_FONT_FAMILIES.set(families);
}

#[cfg(any(target_os = "linux", test))]
fn linux_primary_family(preferred: &'static str, families: Option<&[String]>) -> &'static str {
    match families {
        Some(installed) => std::iter::once(preferred)
            .chain(LINUX_MONOSPACE.iter().copied())
            .find(|candidate| {
                installed
                    .iter()
                    .any(|family| family.eq_ignore_ascii_case(candidate))
            })
            .unwrap_or(LINUX_MONOSPACE[0]),
        None => {
            if preferred.eq_ignore_ascii_case("Menlo") {
                LINUX_MONOSPACE[0]
            } else {
                preferred
            }
        }
    }
}

/// Builds a GPUI `Font` with deduplicated Nerd Font, Japanese, and Emoji fallbacks.
pub fn build_terminal_font(preset: FontPreset, nerd_font_fallback: bool) -> Font {
    build_terminal_font_styled(preset, nerd_font_fallback, false, false)
}

/// Builds a GPUI `Font` with bold/italic variants and deduplicated fallbacks.
pub fn build_terminal_font_styled(
    preset: FontPreset,
    nerd_font_fallback: bool,
    bold: bool,
    italic: bool,
) -> Font {
    let primary = preset.primary_family();
    #[cfg(target_os = "linux")]
    let primary = linux_primary_family(primary, LINUX_FONT_FAMILIES.get().map(Vec::as_slice));
    let mut fallbacks: Vec<String> = Vec::new();
    let mut push_unique = |name: &str| {
        if !name.eq_ignore_ascii_case(primary)
            && !fallbacks
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(name))
        {
            fallbacks.push(name.to_string());
        }
    };

    if preset == FontPreset::Default {
        push_unique("JetBrains Mono");
        push_unique("Monaco");
    }
    #[cfg(target_os = "linux")]
    for name in LINUX_MONOSPACE {
        push_unique(name);
    }
    if nerd_font_fallback {
        for name in NERD_FONT_FALLBACKS {
            push_unique(name);
        }
    }
    for name in JAPANESE_FONT_FALLBACKS {
        push_unique(name);
    }
    for name in EMOJI_FONT_FALLBACKS {
        push_unique(name);
    }

    Font {
        family: primary.into(),
        features: FontFeatures::default(),
        fallbacks: Some(FontFallbacks::from_fonts(fallbacks)),
        weight: if bold {
            FontWeight::BOLD
        } else {
            FontWeight::NORMAL
        },
        style: if italic {
            FontStyle::Italic
        } else {
            FontStyle::Normal
        },
    }
}

/// Computes terminal cell width (1 or 2) for a single extended grapheme cluster.
/// PUA Nerd Font glyphs default to 1 cell; CJK full-width and emoji default to 2 cells.
pub fn grapheme_cell_width(grapheme: &str) -> usize {
    if grapheme.is_empty() {
        return 0;
    }
    let mut chars = grapheme.chars();
    if let Some(first) = chars.next() {
        let code = first as u32;
        // BMP PUA (U+E000..=U+F8FF) and Supplementary PUA-A/B (U+F0000..=U+10FFFD)
        let is_pua = (0xE000..=0xF8FF).contains(&code) || (0xF0000..=0x10FFFD).contains(&code);
        if is_pua {
            return 1;
        }
        // Regional indicator pairs / ZWJ sequences / VS16 emoji presentation
        if grapheme.contains('\u{200D}') || grapheme.contains('\u{FE0F}') {
            return 2;
        }
        if (0x1F1E6..=0x1F1FF).contains(&code) {
            return 2;
        }
    }
    UnicodeWidthStr::width(grapheme).max(1)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionedCell<'a> {
    pub col: usize,
    pub width: usize,
    pub grapheme: &'a str,
}

/// Splits a string into terminal-grid cells with deterministic column offsets.
pub fn layout_cells(text: &str) -> Vec<PositionedCell<'_>> {
    let mut col = 0;
    let mut cells = Vec::new();
    for grapheme in text.graphemes(true) {
        let width = grapheme_cell_width(grapheme);
        cells.push(PositionedCell {
            col,
            width,
            grapheme,
        });
        col += width;
    }
    cells
}

/// Total terminal column width of a string.
pub fn string_cell_width(text: &str) -> usize {
    layout_cells(text)
        .last()
        .map(|cell| cell.col + cell.width)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_uses_an_installed_monospace_when_the_preset_is_missing() {
        let installed = ["Liberation Mono".to_owned(), "UDEV Gothic 35NF".to_owned()];
        assert_eq!(
            linux_primary_family("Menlo", Some(&installed)),
            "Liberation Mono"
        );
        assert_eq!(
            linux_primary_family("UDEV Gothic 35NF", Some(&installed)),
            "UDEV Gothic 35NF"
        );
        let case_mismatch = ["udev gothic 35nf".to_owned()];
        assert_eq!(
            linux_primary_family("UDEV Gothic 35NF", Some(&case_mismatch)),
            "UDEV Gothic 35NF"
        );
        assert_eq!(linux_primary_family("Menlo", None), "DejaVu Sans Mono");
        assert_eq!(
            linux_primary_family("UDEV Gothic 35NF", None),
            "UDEV Gothic 35NF"
        );
        assert_eq!(linux_primary_family("Menlo", Some(&[])), "DejaVu Sans Mono");
    }

    #[test]
    fn font_fallback_deduplicates_primary_and_respects_nerd_toggle() {
        let with_nf = build_terminal_font(FontPreset::UdevGothic35Nf, true);
        let list = with_nf.fallbacks.as_ref().unwrap().fallback_list();
        assert!(!list.iter().any(|f| f == "UDEV Gothic 35NF"));
        assert!(list.iter().any(|f| f == "Symbols Nerd Font Mono"));
        assert!(list.iter().any(|f| f == "Hiragino Sans"));
        assert!(list.iter().any(|f| f == "Apple Color Emoji"));

        let without_nf = build_terminal_font(FontPreset::Menlo, false);
        let list = without_nf.fallbacks.as_ref().unwrap().fallback_list();
        assert!(!list.iter().any(|f| f == "Symbols Nerd Font Mono"));
        assert!(!list.iter().any(|f| f == "UDEV Gothic 35NF"));
        assert!(list.iter().any(|f| f == "Hiragino Sans"));
    }

    #[test]
    fn cell_width_aligns_ascii_40_and_cjk_20_and_pua_and_zwj() {
        let ascii40 = "0123456789012345678901234567890123456789";
        let cjk20 = "あいうえおかきくけこさしすせそたちつてと";
        let half_kana40 = "ｱｲｳｴｵｶｷｸｹｺｻｼｽｾｿﾀﾁﾂﾃﾄﾅﾆﾇﾈﾉﾊﾋﾌﾍﾎﾏﾐﾑﾒﾓﾔﾕﾖﾗﾘ";
        assert_eq!(string_cell_width(ascii40), 40);
        assert_eq!(string_cell_width(cjk20), 40);
        assert_eq!(string_cell_width(half_kana40), 40);

        // NFD combining dakuten: か + U+3099 is 1 grapheme of width 2
        assert_eq!(string_cell_width("か\u{3099}"), 2);
        // BMP PUA and Supplementary PUA-A Nerd Font icons occupy 1 cell
        assert_eq!(string_cell_width("\u{E0B0}\u{E718}\u{F015}\u{F024B}"), 4);
        // ZWJ emoji and flag occupy 2 cells each
        assert_eq!(string_cell_width("👨‍👩‍👧‍👦"), 2);
        assert_eq!(string_cell_width("🇯🇵"), 2);
        assert_eq!(string_cell_width("🦀"), 2);
    }
}
