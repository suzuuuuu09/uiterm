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
        if let Some(locale) = locale.filter(|value| value != "C" && value != "POSIX") {
            return Self::from_identifier(&locale);
        }

        #[cfg(target_os = "macos")]
        if let Some(locale) = macos_locale() {
            return Self::from_identifier(&locale);
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
fn macos_locale() -> Option<String> {
    let output = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::Locale;

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
