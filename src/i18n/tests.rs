use super::*;

#[test]
fn codes_round_trip() {
    for lang in Lang::ALL {
        assert_eq!(Lang::from_code(lang.code()), Some(lang));
    }
    assert_eq!(Lang::from_code("FR"), Some(Lang::Fr));
    assert_eq!(Lang::from_code("it"), None);
}

#[test]
fn accept_language_picks_best_known() {
    assert_eq!(
        Lang::from_accept_language("fr-FR,fr;q=0.9,en;q=0.8"),
        Some(Lang::Fr)
    );
    assert_eq!(
        Lang::from_accept_language("it, de;q=0.5, es;q=0.7"),
        Some(Lang::Es)
    );
    assert_eq!(Lang::from_accept_language("en;q=0.2, de"), Some(Lang::De));
    assert_eq!(Lang::from_accept_language("it, pt"), None);
    assert_eq!(Lang::from_accept_language("de;q=0"), None);
    assert_eq!(Lang::from_accept_language(""), None);
}

#[test]
fn parse_reads_blocks_and_skips_comments() {
    let catalogs = parse(
        "# note\nSave\nfr: Enregistrer\nde: Speichern\n\nTwo\\nlines: yes\nfr: Deux\\nlignes : oui\n",
    );
    let fr = &catalogs[&Lang::Fr];
    assert_eq!(fr.get("Save").map(String::as_str), Some("Enregistrer"));
    assert_eq!(
        fr.get("Two\nlines: yes").map(String::as_str),
        Some("Deux\nlignes : oui")
    );
    assert_eq!(fr.len(), 2);
    assert_eq!(catalogs[&Lang::De].len(), 1);
}

#[test]
fn outside_a_request_is_english() {
    assert_eq!(current(), Lang::En);
    assert_eq!(t("Save"), "Save");
}

#[test]
fn scope_sets_current_language() {
    assert_eq!(sync_scope(Lang::De, current), Lang::De);
}

#[test]
fn tf_fills_placeholders_in_order() {
    assert_eq!(tf("{} of {}", &[&1, &"two"]), "1 of two");
    assert_eq!(tf("no args {}", &[]), "no args ");
}

#[test]
fn plural_rules() {
    assert_eq!(tn(1, "{} day", "{} days"), "1 day");
    assert_eq!(tn(0, "{} day", "{} days"), "0 days");
    assert!(Lang::Fr.is_singular(0));
    assert!(!Lang::De.is_singular(0));
    assert!(!Lang::Fr.is_singular(2));
}

#[test]
fn language_handle_stores_lang() {
    let language = Language::default();
    assert_eq!(language.get(), Lang::En);
    language.set(Lang::Es);
    assert_eq!(language.clone().get(), Lang::Es);
}

// --- Catalog completeness ---

/// Reads a Rust or askama string literal starting at `s[0] == '"'`; returns its value.
fn literal(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s[1..].chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                // Line continuation: the next line's indentation is skipped.
                '\n' => while chars.next_if(|c| c.is_whitespace()).is_some() {},
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
    None
}

/// The literal after `call` (`t(`, `tf(`), skipping whitespace.
fn after(text: &str, at: usize) -> Option<String> {
    let rest = text[at..].trim_start();
    rest.starts_with('"').then(|| literal(rest)).flatten()
}

/// Every English text the sources translate: `t("…")`, `tf("…"`, `tn(n, "…", "…")` in Rust,
/// `"…"|t`, `"…"|tf(…)`, `"…"|tf2(…)` and `|tn("…", "…")` in templates.
fn used_keys() -> std::collections::BTreeSet<String> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut keys = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    let mut dirs = vec![root.join("src"), root.join("templates")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if !path.ends_with("i18n") {
                    dirs.push(path);
                }
            } else {
                files.push(path);
            }
        }
    }
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        let is_rust = path.extension().is_some_and(|e| e == "rs");
        // Tests may quote English text without translating it.
        let text = match text.find("\n#[cfg(test)]\nmod tests") {
            Some(at) if is_rust => &text[..at],
            _ => &text[..],
        };
        let bytes = text.as_bytes();
        let ident = |i: usize| {
            i > 0 && (bytes[i - 1].is_ascii_alphanumeric() || b"_.:".contains(&bytes[i - 1]))
        };
        for (at, call) in text.match_indices(['t']) {
            let _ = call;
            if is_rust {
                if ident(at) {
                    continue;
                }
                let rest = &text[at..];
                if let Some(r) = rest.strip_prefix("t(").or_else(|| rest.strip_prefix("tf(")) {
                    if let Some(key) = after(r, 0) {
                        keys.insert(key);
                    }
                } else if let Some(r) = rest.strip_prefix("tn(") {
                    // Skips the count, then reads both forms.
                    let mut depth = 0;
                    let comma = r.find(|c| {
                        match c {
                            '(' => depth += 1,
                            ')' => depth -= 1,
                            ',' if depth == 0 => return true,
                            _ => {}
                        }
                        false
                    });
                    let Some(comma) = comma else { continue };
                    let r = r[comma + 1..].trim_start();
                    if let Some(one) = literal(r) {
                        keys.insert(one.clone());
                        let r = r[one.len() + 2..].trim_start().trim_start_matches(',');
                        if let Some(other) = after(r, 0) {
                            keys.insert(other);
                        }
                    }
                }
            }
        }
        if !is_rust {
            // `"…"|t`, `"…"|tf(`, `"…"|tf2(`: the literal just before the pipe.
            for (at, _) in text.match_indices("\"|t") {
                let start = text[..at].rfind("{{ \"").map(|s| s + 3);
                if let Some(start) = start
                    && let Some(key) = literal(&text[start..])
                {
                    keys.insert(key);
                }
            }
            for (at, _) in text.match_indices("|tn(") {
                let r = &text[at + 4..];
                if let Some(one) = literal(r) {
                    keys.insert(one.clone());
                    let r = r[one.len() + 2..].trim_start().trim_start_matches(',');
                    if let Some(other) = after(r, 0) {
                        keys.insert(other);
                    }
                }
            }
        }
    }
    keys
}

#[test]
fn every_text_has_a_translation_and_no_entry_is_stale() {
    let used = used_keys();
    assert!(
        used.contains("Settings saved."),
        "extraction finds Rust keys"
    );
    assert!(used.contains("Log in"), "extraction finds template keys");
    for lang in [Lang::Fr, Lang::Es, Lang::De] {
        let catalog = lang.catalog().unwrap();
        let missing: Vec<_> = used
            .iter()
            .filter(|k| !catalog.contains_key(k.as_str()))
            .collect();
        let stale: Vec<_> = catalog
            .keys()
            .filter(|k| !used.contains(k.as_ref() as &str))
            .collect();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "{}: {} missing {missing:#?}\nstale {stale:#?}",
            lang.code(),
            missing.len()
        );
    }
}

#[test]
fn translations_keep_placeholders() {
    for lang in [Lang::Fr, Lang::Es, Lang::De] {
        for (english, translation) in lang.catalog().unwrap() {
            assert_eq!(
                english.matches("{}").count(),
                translation.matches("{}").count(),
                "{}: {english:?} → {translation:?}",
                lang.code()
            );
            for tag in ["<a ", "<code>", "<strong"] {
                assert_eq!(
                    english.matches(tag).count(),
                    translation.matches(tag).count(),
                    "{english:?}"
                );
            }
        }
    }
}
