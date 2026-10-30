pub mod auth;
pub mod channel;
pub mod monitor;
pub mod settings;
pub mod status_page;

/// Whether the text shows at least one character: whitespace, control characters and
/// zero-width formatting characters (U+200B, U+FEFF…) do not count.
pub fn has_visible_char(text: &str) -> bool {
    text.chars()
        .any(|c| !c.is_whitespace() && !c.is_control() && !is_invisible_format(c))
}

/// Length as the browser's `maxlength` counts it, in UTF-16 code units: an emoji counts two.
pub fn text_len(text: &str) -> usize {
    text.encode_utf16().count()
}

pub fn is_invisible_format(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'
            | '\u{1160}'
            | '\u{17B4}'
            | '\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{E0000}'..='\u{E007F}'
    )
}

#[cfg(test)]
mod tests {
    use super::has_visible_char;

    #[test]
    fn invisible_text_is_detected() {
        for text in [
            "",
            " \t",
            "\u{200B}",
            "\u{00A0}\u{200D}",
            "\u{FEFF} ",
            "\u{3164}",
        ] {
            assert!(!has_visible_char(text), "{text:?}");
        }
        for text in ["a", " é ", "\u{200B}x", "日本"] {
            assert!(has_visible_char(text), "{text:?}");
        }
    }
}
