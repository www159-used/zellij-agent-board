//! Shared UI text for native and WASM callers. Protocol values and keys stay stable.
use fluent_bundle::{concurrent::FluentBundle, FluentArgs, FluentResource};
use serde::{Deserialize, Deserializer, Serialize};
use std::sync::OnceLock;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub enum Locale {
    #[default]
    #[serde(rename = "en")]
    English,
    #[serde(rename = "zh-CN")]
    SimplifiedChinese,
}
impl Locale {
    pub const ALL: [Self; 2] = [Self::English, Self::SimplifiedChinese];

    pub fn native_name(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::SimplifiedChinese => "简体中文",
        }
    }

    pub fn from_tag(tag: &str) -> Self {
        let tag = tag
            .split(['.', '@'])
            .next()
            .unwrap_or_default()
            .replace('_', "-")
            .to_ascii_lowercase();
        match tag.as_str() {
            "zh" | "zh-cn" | "zh-hans" | "zh-hans-cn" => Self::SimplifiedChinese,
            _ => Self::English,
        }
    }
}
impl<'de> Deserialize<'de> for Locale {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(|tag| Self::from_tag(&tag))
    }
}

macro_rules! messages {
    ($($name:ident => $id:literal),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Message { $($name),+ }
        impl Message {
            pub fn id(self) -> &'static str { match self { $(Self::$name => $id),+ } }
            #[cfg(test)]
            const ALL: &'static [Self] = &[$(Self::$name),+];
        }
    };
}
messages! {
    NavHome => "nav_home", NavMonitor => "nav_monitor", NavSettings => "nav_settings",
    NavQuit => "nav_quit", SettingsTitle => "settings_title", SettingsHeading => "settings_heading",
    SettingsIdleAction => "settings_idle_action", SettingsIdleTime => "settings_idle_time",
    SettingsSaverStyle => "settings_saver_style", SettingsLanguage => "settings_language",
    ChoiceScreensaver => "choice_screensaver", ChoiceMonitor => "choice_monitor",
    ChoiceClose => "choice_close", ChoiceDisabled => "choice_disabled", ChoiceBlank => "choice_blank",
    ChoiceClock => "choice_clock", PreviewScreensaver => "preview_screensaver", Back => "back",
    Seconds => "seconds", Minutes => "minutes", Resume => "resume",
    SettingsLoadError => "settings_load_error", SettingsSaveError => "settings_save_error",
    InvalidIdleSeconds => "invalid_idle_seconds",
}

type Catalog = FluentBundle<FluentResource>;
fn catalog(locale: Locale) -> &'static Catalog {
    static EN: OnceLock<Catalog> = OnceLock::new();
    static ZH_CN: OnceLock<Catalog> = OnceLock::new();
    let (cache, tag, source) = match locale {
        Locale::English => (&EN, "en", include_str!("../locales/en.ftl")),
        Locale::SimplifiedChinese => (&ZH_CN, "zh-CN", include_str!("../locales/zh-CN.ftl")),
    };
    cache.get_or_init(|| {
        let resource =
            FluentResource::try_new(source.to_owned()).expect("valid embedded Fluent resource");
        let mut bundle = Catalog::new_concurrent(vec![tag.parse().expect("valid locale tag")]);
        // Terminal cell layout has no bidi rendering; avoid invisible isolation marks.
        bundle.set_use_isolating(false);
        bundle
            .add_resource(resource)
            .expect("unique UI message keys");
        bundle
    })
}
fn translated(
    key: Message,
    selected: &Catalog,
    fallback: &Catalog,
    args: &FluentArgs<'_>,
) -> String {
    for bundle in [selected, fallback] {
        if let Some(pattern) = bundle
            .get_message(key.id())
            .and_then(|message| message.value())
        {
            let mut errors = vec![];
            let text = bundle.format_pattern(pattern, Some(args), &mut errors);
            if errors.is_empty() {
                return text.into_owned();
            }
        }
    }
    key.id().to_owned()
}

/// A cheap value passed to renderers, so changing language needs no global mutation.
#[derive(Clone, Copy, Debug, Default)]
pub struct I18n {
    locale: Locale,
}
impl I18n {
    pub fn new(locale: Locale) -> Self {
        Self { locale }
    }
    pub fn text(self, key: Message) -> String {
        self.format(key, &[])
    }
    /// Insert values once, without treating braces inside those values as templates.
    pub fn format(self, key: Message, arguments: &[(&str, &str)]) -> String {
        let mut args = FluentArgs::new();
        for (name, value) in arguments {
            args.set(*name, *value);
        }
        translated(key, catalog(self.locale), catalog(Locale::English), &args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogs_cover_messages_and_missing_translations_fall_back_to_english() {
        let english = catalog(Locale::English);
        let chinese = catalog(Locale::SimplifiedChinese);
        for key in Message::ALL {
            for bundle in [english, chinese] {
                let message = bundle
                    .get_message(key.id())
                    .unwrap_or_else(|| panic!("{}", key.id()));
                let mut args = FluentArgs::new();
                for name in ["seconds", "minutes", "error"] {
                    args.set(name, "test");
                }
                let mut errors = vec![];
                let _ = bundle.format_pattern(message.value().unwrap(), Some(&args), &mut errors);
                assert!(errors.is_empty(), "{}: {errors:?}", key.id());
            }
        }
        let empty = Catalog::new_concurrent(vec!["zh-CN".parse().unwrap()]);
        assert_eq!(
            translated(Message::NavSettings, &empty, english, &FluentArgs::new()),
            "Settings"
        );
        assert_eq!(
            I18n::new(Locale::from_tag("fr-FR")).text(Message::NavSettings),
            "Settings"
        );
    }
    #[test]
    fn locale_tags_and_parameters_are_stable() {
        for tag in ["zh-CN", "zh_CN.UTF-8", "zh-Hans", "zh"] {
            assert_eq!(Locale::from_tag(tag), Locale::SimplifiedChinese);
        }
        let chinese = I18n::new(Locale::SimplifiedChinese);
        assert_eq!(
            chinese.format(Message::Seconds, &[("seconds", "30")]),
            "30秒"
        );
        assert_eq!(
            chinese.format(Message::SettingsSaveError, &[("error", "disk {error}")]),
            "修改已生效，但保存失败：disk {error}"
        );
    }
}
