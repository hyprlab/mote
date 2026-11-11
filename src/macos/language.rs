//! The language Mote speaks. On Linux the session sets LANG; an app opened
//! from Finder or the Dock gets no LANG at all, and would speak English
//! whatever macOS is set to. Here the languages come from macOS's own list
//! (System Settings > General > Language & Region) and become what
//! gettext reads: LANG, a locale the C library has, and LANGUAGE, the order
//! to look for translations in.

use objc2_foundation::NSLocale;

/// SAFETY: as [`super::prepare`].
pub(super) unsafe fn prepare() {
    if ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .any(|name| std::env::var_os(name).is_some())
    {
        return;
    }
    let preferred: Vec<String> = NSLocale::preferredLanguages()
        .iter()
        .map(|tag| tag.to_string())
        .collect();
    let region = NSLocale::currentLocale()
        .regionCode()
        .map(|code| code.to_string());
    let (lang, language) = environment(&preferred, region.as_deref(), |locale| {
        std::path::Path::new("/usr/share/locale").join(locale).is_dir()
    });
    // SAFETY: the caller's.
    unsafe {
        std::env::set_var("LANG", lang);
        if !language.is_empty() {
            std::env::set_var("LANGUAGE", language);
        }
    }
}

/// LANG and LANGUAGE for macOS's preferred languages, given as tags such
/// as `fr-CA` or `zh-Hans-CN`, and its region. LANG has to be a locale the
/// C library has (`exists`) or gettext falls back to English; English in
/// the region is the last resort, as the C library has it everywhere.
fn environment(
    preferred: &[String],
    region: Option<&str>,
    exists: impl Fn(&str) -> bool,
) -> (String, String) {
    let mut language = Vec::new();
    let mut lang = None;
    for tag in preferred {
        let mut parts = tag.split('-');
        let Some(code) = parts.next().filter(|c| !c.is_empty()) else {
            continue;
        };
        // A script (Hans, Latn) is four letters; a region two, or digits.
        let own = parts.find(|p| p.len() != 4).or(region);
        if let Some(own) = own {
            let locale = format!("{code}_{own}");
            if lang.is_none() && exists(&format!("{locale}.UTF-8")) {
                lang = Some(format!("{locale}.UTF-8"));
            }
            language.push(locale);
        }
        language.push(code.to_owned());
    }
    let lang = lang.unwrap_or_else(|| {
        let fallback = format!("en_{}.UTF-8", region.unwrap_or("US"));
        if exists(&fallback) {
            fallback
        } else {
            "en_US.UTF-8".to_owned()
        }
    });
    language.dedup();
    (lang, language.join(":"))
}

#[cfg(test)]
mod tests {
    use super::environment;

    fn tags(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|t| (*t).to_owned()).collect()
    }

    #[test]
    fn takes_the_first_language_the_c_library_has() {
        let (lang, language) = environment(&tags(&["fr-CA", "en-US"]), Some("CA"), |l| {
            l == "fr_CA.UTF-8" || l == "en_US.UTF-8"
        });
        assert_eq!(lang, "fr_CA.UTF-8");
        assert_eq!(language, "fr_CA:fr:en_US:en");
    }

    #[test]
    fn a_language_without_a_region_takes_the_macs() {
        let (lang, language) = environment(&tags(&["fr"]), Some("BE"), |l| l == "fr_BE.UTF-8");
        assert_eq!(lang, "fr_BE.UTF-8");
        assert_eq!(language, "fr_BE:fr");
    }

    #[test]
    fn a_script_is_not_a_region() {
        let (_, language) = environment(&tags(&["zh-Hans-CN"]), None, |_| false);
        assert_eq!(language, "zh_CN:zh");
    }

    #[test]
    fn falls_back_to_english() {
        let (lang, _) = environment(&tags(&["xx-YY"]), None, |l| l == "en_US.UTF-8");
        assert_eq!(lang, "en_US.UTF-8");
    }
}
