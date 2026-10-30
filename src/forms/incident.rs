use crate::forms::{has_visible_char, text_len};
use serde::Deserialize;

pub const NOTE_MAX_CHARS: usize = 2000;

/// The note form of an incident, as submitted.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct NoteForm {
    pub note: String,
    pub public: Option<String>,
}

/// A validated note: `None` clears it.
#[derive(Debug, PartialEq, Eq)]
pub struct NoteInput {
    pub note: Option<String>,
    pub public: bool,
}

impl NoteForm {
    /// Trimmed; blank clears the note. Line breaks arrive as CRLF but count once, as the
    /// textarea's `maxlength` counts them; refused beyond `NOTE_MAX_CHARS`, which `maxlength`
    /// already prevents. The public box is kept as ticked, even without a note.
    pub fn validate(&self) -> Result<NoteInput, String> {
        let note = self.note.replace("\r\n", "\n");
        let note = note.trim();
        if text_len(note) > NOTE_MAX_CHARS {
            return Err(format!("A note is at most {NOTE_MAX_CHARS} characters."));
        }
        let note = has_visible_char(note).then(|| note.to_string());
        Ok(NoteInput {
            public: self.public.is_some(),
            note,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_is_trimmed_blank_clears_and_length_is_bounded() {
        let form = |note: &str, public: bool| NoteForm {
            note: note.to_string(),
            public: public.then(|| "on".to_string()),
        };
        assert_eq!(
            form("  Disk full \n", true).validate(),
            Ok(NoteInput {
                note: Some("Disk full".to_string()),
                public: true
            })
        );
        assert_eq!(
            form(" \u{200B} ", true).validate(),
            Ok(NoteInput {
                note: None,
                public: true
            })
        );
        assert!(
            form(&"a".repeat(NOTE_MAX_CHARS + 1), false)
                .validate()
                .is_err()
        );
        assert!(form(&"a".repeat(NOTE_MAX_CHARS), false).validate().is_ok());
        let crlf = format!("{}{}", "a\r\n".repeat(10), "a".repeat(NOTE_MAX_CHARS - 20));
        assert_eq!(
            form(&crlf, false)
                .validate()
                .unwrap()
                .note
                .unwrap()
                .matches('\n')
                .count(),
            10
        );
    }
}
