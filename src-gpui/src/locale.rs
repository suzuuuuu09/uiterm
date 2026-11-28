#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Locale {
    English,
    Japanese,
}

impl Locale {
    pub fn system() -> Self {
        let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.is_empty());
        if let Some(locale) = locale {
            return Self::from_identifier(&locale);
        }

        #[cfg(target_os = "macos")]
        if let Some(locale) = macos_preferred_language() {
            return locale;
        }

        Self::English
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
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| {
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
    use super::Locale;

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
