//! Display-only translations. USB identities, configuration and wire states stay unchanged.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io,
    path::Path,
    sync::{
        OnceLock,
        atomic::{AtomicU8, Ordering},
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    #[default]
    En,
    Ru,
    De,
    Fr,
    Zh,
}
pub const LANGUAGES: [(Language, &str); 5] = [
    (Language::En, "English"),
    (Language::Ru, "Русский"),
    (Language::De, "Deutsch"),
    (Language::Fr, "Français"),
    (Language::Zh, "简体中文"),
];
impl Language {
    pub fn parse(value: &str) -> io::Result<Self> {
        match value.to_ascii_lowercase().as_str() {
            "en" | "en-us" | "en-gb" => Ok(Self::En),
            "ru" => Ok(Self::Ru),
            "de" => Ok(Self::De),
            "fr" => Ok(Self::Fr),
            "zh" | "zh-cn" | "zh-hans" => Ok(Self::Zh),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "language must be en, ru, de, fr or zh",
            )),
        }
    }
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Ru => "ru",
            Self::De => "de",
            Self::Fr => "fr",
            Self::Zh => "zh",
        }
    }
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    #[serde(default)]
    pub language: Language,
}
fn preferences_path(config: &Path) -> std::path::PathBuf {
    if let (Some(common), Some(local)) = (
        std::env::var_os("PROGRAMDATA"),
        std::env::var_os("LOCALAPPDATA"),
    ) {
        let root = std::path::PathBuf::from(common).join("USB to Network Bridge");
        if let Ok(relative) = config.strip_prefix(root) {
            return std::path::PathBuf::from(local)
                .join("USB to Network Bridge")
                .join(relative)
                .with_extension("preferences.json");
        }
    }
    config.with_extension("preferences.json")
}
pub fn preferred(config: &Path) -> Language {
    crate::settings::read::<Preferences>(&preferences_path(config))
        .unwrap_or_default()
        .language
}
pub fn save_language(config: &Path, language: Language) -> io::Result<()> {
    crate::settings::save(&preferences_path(config), &Preferences { language })
}
static ACTIVE: AtomicU8 = AtomicU8::new(0);
pub fn set_language(language: Language) {
    ACTIVE.store(language as u8, Ordering::Release);
}
pub fn language() -> Language {
    LANGUAGES[usize::from(ACTIVE.load(Ordering::Acquire))].0
}
type Catalog = BTreeMap<String, BTreeMap<String, String>>;
fn catalog() -> &'static Catalog {
    static VALUE: OnceLock<Catalog> = OnceLock::new();
    VALUE.get_or_init(|| {
        serde_json::from_str(include_str!("locales.json")).expect("embedded translation catalog")
    })
}
pub fn text_in(language: Language, source: &str) -> &str {
    catalog()
        .get(source)
        .and_then(|entry| entry.get(language.code()))
        .map(String::as_str)
        .unwrap_or(source)
}
pub fn t(source: &str) -> &str {
    text_in(language(), source)
}
/// Values are substituted once, after selecting the template. Their contents are never translated.
pub fn format(source: &str, values: &[(&str, String)]) -> String {
    render(t(source), values)
}
fn render(template: &str, values: &[(&str, String)]) -> String {
    let mut result = String::new();
    let mut rest = template;
    let mut positional = 0;
    while let Some(start) = rest.find('{') {
        result.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}').map(|n| start + n) else {
            result.push_str(&rest[start..]);
            return result;
        };
        let key = &rest[start + 1..end];
        let auto = positional.to_string();
        let key = if key.is_empty() {
            positional += 1;
            &auto
        } else {
            key
        };
        if let Some((_, value)) = values.iter().find(|(name, _)| *name == key) {
            result.push_str(value);
        } else {
            result.push_str(&rest[start..=end]);
        }
        rest = &rest[end + 1..];
    }
    result.push_str(rest);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn placeholders(value: &str) -> Vec<&str> {
        let mut rest = value;
        let mut found = Vec::new();
        while let Some(start) = rest.find('{') {
            let end = rest[start..].find('}').unwrap() + start;
            found.push(&rest[start..=end]);
            rest = &rest[end + 1..];
        }
        found.sort();
        found
    }
    #[test]
    fn all_languages_cover_every_message_and_keep_parameters() {
        for (key, entry) in catalog() {
            for (language, _) in LANGUAGES {
                let value = entry
                    .get(language.code())
                    .unwrap_or_else(|| panic!("missing {}: {key}", language.code()));
                assert!(!value.is_empty());
                assert_eq!(
                    placeholders(key),
                    placeholders(value),
                    "{}: {key}",
                    language.code()
                );
            }
        }
    }
    #[test]
    fn english_is_default_and_device_names_are_not_retranslated() {
        assert_eq!(Preferences::default().language, Language::En);
        assert_eq!(text_in(Language::En, "Включено"), "Enabled");
        assert_eq!(text_in(Language::Zh, "Включено"), "已启用");
        assert_eq!(
            text_in(Language::En, "USB\\VID_1C4F&PID_0048"),
            "USB\\VID_1C4F&PID_0048"
        );
        assert_eq!(
            render(
                "Error: {message}",
                &[("message", "Сервер {message}".into())]
            ),
            "Error: Сервер {message}"
        );
        assert_eq!(
            render("{} / {}", &[("0", "A".into()), ("1", "B".into())]),
            "A / B"
        );
    }
}
