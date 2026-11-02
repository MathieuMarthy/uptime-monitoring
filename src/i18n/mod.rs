//! Interface languages. English text is the key: templates write `{{ "Save"|t }}`, Rust code
//! `t("Save")`. `catalog.txt`, embedded in the binary, gives each English text its translations;
//! a missing one shows the English text. The language of a request is set by a
//! middleware in a task-local, so handlers, forms and templates never pass it around.

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Lang {
    #[default]
    En,
    Fr,
    Es,
    De,
}

impl Lang {
    pub const ALL: [Lang; 4] = [Lang::En, Lang::Fr, Lang::Es, Lang::De];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Fr => "fr",
            Lang::Es => "es",
            Lang::De => "de",
        }
    }

    /// The language's own name, as shown in the language picker.
    pub fn name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Fr => "Français",
            Lang::Es => "Español",
            Lang::De => "Deutsch",
        }
    }

    pub fn from_code(code: &str) -> Option<Lang> {
        Lang::ALL
            .into_iter()
            .find(|lang| lang.code().eq_ignore_ascii_case(code))
    }

    /// The preferred known language of an `Accept-Language` header, by quality then order.
    pub fn from_accept_language(header: &str) -> Option<Lang> {
        let mut best: Option<(f32, Lang)> = None;
        for item in header.split(',') {
            let mut parts = item.split(';');
            let tag = parts.next().unwrap_or("").trim();
            let primary = tag.split('-').next().unwrap_or("");
            let quality = parts
                .find_map(|p| p.trim().strip_prefix("q="))
                .and_then(|q| q.trim().parse::<f32>().ok())
                .unwrap_or(1.0);
            if let Some(lang) = Lang::from_code(primary)
                && quality > 0.0
                && best.is_none_or(|(q, _)| quality > q)
            {
                best = Some((quality, lang));
            }
        }
        best.map(|(_, lang)| lang)
    }

    /// Whether `n` takes the singular form.
    fn is_singular(self, n: i64) -> bool {
        match self {
            Lang::Fr => n.abs() <= 1,
            _ => n.abs() == 1,
        }
    }

    fn catalog(self) -> Option<&'static HashMap<String, String>> {
        static CATALOG: LazyLock<HashMap<Lang, HashMap<String, String>>> =
            LazyLock::new(|| parse(include_str!("catalog.txt")));
        CATALOG.get(&self)
    }

    /// The translation of `english`, or `english` itself.
    pub fn translate(self, english: &str) -> &str {
        self.catalog()
            .and_then(|catalog| catalog.get(english))
            .map_or(english, |s| s.as_str())
    }
}

/// Catalog format: blocks separated by blank lines, each an English line followed by one
/// `fr: …` line per language; `#` starts a comment line; `\n` stands for a line break.
pub(crate) fn parse(source: &str) -> HashMap<Lang, HashMap<String, String>> {
    let mut catalogs: HashMap<Lang, HashMap<String, String>> = HashMap::new();
    let mut english: Option<String> = None;
    for line in source.lines().filter(|line| !line.starts_with('#')) {
        if line.trim().is_empty() {
            english = None;
            continue;
        }
        let translation = line
            .split_once(": ")
            .and_then(|(code, text)| Some((Lang::from_code(code)?, text)));
        match (&english, translation) {
            (Some(english), Some((lang, text))) => {
                catalogs
                    .entry(lang)
                    .or_default()
                    .insert(english.clone(), text.replace("\\n", "\n"));
            }
            _ => english = Some(line.replace("\\n", "\n")),
        }
    }
    catalogs
}

tokio::task_local! {
    static LANG: Lang;
}

/// The language of the current request; English outside one.
pub fn current() -> Lang {
    LANG.try_with(|lang| *lang).unwrap_or_default()
}

/// Runs `future` with `lang` as the current language.
pub async fn scope<F: Future>(lang: Lang, future: F) -> F::Output {
    LANG.scope(lang, future).await
}

/// Runs `f` with `lang` as the current language.
pub fn sync_scope<R>(lang: Lang, f: impl FnOnce() -> R) -> R {
    LANG.sync_scope(lang, f)
}

/// `english` in the current language. Lines with `{}` go through [`tf`].
pub fn t(english: &'static str) -> &'static str {
    current().translate(english)
}

/// `english` in the current language, each `{}` replaced by the next argument.
pub fn tf(english: &'static str, args: &[&dyn Display]) -> String {
    fill(t(english), args)
}

/// `one` or `other` (both with a `{}` for `n`), by the plural rule of the current language.
pub fn tn(n: i64, one: &'static str, other: &'static str) -> String {
    let lang = current();
    let english = if lang.is_singular(n) { one } else { other };
    fill(lang.translate(english), &[&n])
}

fn fill(text: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut args = args.iter();
    let mut rest = text;
    while let Some(at) = rest.find("{}") {
        out.push_str(&rest[..at]);
        if let Some(arg) = args.next() {
            out.push_str(&arg.to_string());
        }
        rest = &rest[at + 2..];
    }
    out.push_str(rest);
    out
}

/// The instance language, shared by the handlers and the notifier; settings change it live.
#[derive(Debug, Clone, Default)]
pub struct Language(Arc<AtomicU8>);

impl Language {
    pub fn get(&self) -> Lang {
        Lang::ALL[usize::from(self.0.load(Ordering::Relaxed)).min(Lang::ALL.len() - 1)]
    }

    pub fn set(&self, lang: Lang) {
        let index = Lang::ALL.iter().position(|l| *l == lang).unwrap_or(0);
        self.0.store(index as u8, Ordering::Relaxed);
    }
}

/// Askama filters: `{{ "Save"|t }}`, `{{ "{} monitors"|tf(count) }}`.
pub mod filters {
    use std::fmt::Display;

    #[askama::filter_fn]
    pub fn t(english: impl Display, _: &dyn askama::Values) -> askama::Result<String> {
        let english = english.to_string();
        Ok(super::current().translate(&english).to_string())
    }

    /// `english` translated, its `{}` replaced by `arg`.
    #[askama::filter_fn]
    pub fn tf<T: Display>(
        english: impl Display,
        _: &dyn askama::Values,
        arg: T,
    ) -> askama::Result<String> {
        let english = english.to_string();
        Ok(super::fill(super::current().translate(&english), &[&arg]))
    }

    /// `english` translated, its two `{}` replaced by `a` then `b`.
    #[askama::filter_fn]
    pub fn tf2<A: Display, B: Display>(
        english: impl Display,
        _: &dyn askama::Values,
        a: A,
        b: B,
    ) -> askama::Result<String> {
        let english = english.to_string();
        Ok(super::fill(super::current().translate(&english), &[&a, &b]))
    }

    /// `{{ n|tn("{} monitor", "{} monitors") }}`.
    #[askama::filter_fn]
    pub fn tn(
        n: impl std::borrow::Borrow<i64>,
        _: &dyn askama::Values,
        one: &'static str,
        other: &'static str,
    ) -> askama::Result<String> {
        let n = *n.borrow();
        let lang = super::current();
        let english = if lang.is_singular(n) { one } else { other };
        Ok(super::fill(lang.translate(english), &[&n]))
    }
}

#[cfg(test)]
mod tests;
