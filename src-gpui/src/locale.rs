#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    English,
    Japanese,
}

impl Locale {
    pub fn system() -> Self {
        if let Some(locale) = Self::environment_locale(|name| std::env::var(name).ok()) {
            return locale;
        }

        #[cfg(target_os = "macos")]
        if let Some(locale) = macos_preferred_language() {
            return locale;
        }

        Self::English
    }

    fn environment_locale(read: impl Fn(&str) -> Option<String>) -> Option<Self> {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .find_map(|name| read(name).filter(|value| !value.is_empty()))
            .map(|value| Self::from_identifier(&value))
    }

    fn from_identifier(id: &str) -> Self {
        match id.split(['-', '_', '.', '@']).next() {
            Some("ja") => Self::Japanese,
            _ => Self::English,
        }
    }

    pub fn label(self, english: &'static str, japanese: &'static str) -> &'static str {
        match self {
            Self::English => english,
            Self::Japanese => japanese,
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_preferred_language() -> Option<Locale> {
    let output = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLanguages"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_apple_languages(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(any(target_os = "macos", test))]
fn parse_apple_languages(output: &str) -> Option<Locale> {
    output.lines().find_map(|line| {
        let id = line.trim().trim_end_matches(',').trim_matches('"');
        match id.split(['-', '_']).next() {
            Some("ja") => Some(Locale::Japanese),
            Some("en") => Some(Locale::English),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::{Locale, parse_apple_languages};

    #[test]
    fn chooses_first_supported_macos_preferred_language() {
        assert!(
            parse_apple_languages("(\n    \"fr-FR\",\n    \"ja-JP\",\n    \"en-US\"\n)")
                == Some(Locale::Japanese)
        );
        assert!(parse_apple_languages("(\n    \"fr-FR\"\n)").is_none());
    }

    #[test]
    fn locale_overrides_are_checked_in_priority_order() {
        let locale = |values: &[(&str, &str)]| {
            Locale::environment_locale(|name| {
                values
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| (*value).to_owned())
            })
        };
        assert!(
            locale(&[("LC_ALL", "C"), ("LC_MESSAGES", "ja"), ("LANG", "ja")])
                == Some(Locale::English)
        );
        assert!(
            locale(&[("LC_ALL", ""), ("LC_MESSAGES", "ja"), ("LANG", "en")])
                == Some(Locale::Japanese)
        );
        assert!(locale(&[("LANG", "ja")]) == Some(Locale::Japanese));
        assert!(locale(&[]).is_none());
    }

    #[test]
    fn c_and_posix_locales_use_english() {
        for id in ["C", "POSIX", "C.UTF-8"] {
            assert!(Locale::from_identifier(id) == Locale::English);
        }
    }

    #[test]
    fn chooses_japanese_for_both_os_locale_formats() {
        for id in ["ja-JP", "ja_JP.UTF-8", "ja"] {
            assert_eq!(
                Locale::from_identifier(id).label("Restart", "再起動"),
                "再起動"
            );
        }
        assert_eq!(
            Locale::from_identifier("en_US").label("Restart", "再起動"),
            "Restart"
        );
    }
}
