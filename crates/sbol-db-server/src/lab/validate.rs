//! Shared wire shape for `/lab/api/{sql,sparql}/validate`.
//!
//! Validation parses without executing. The response is uniform across
//! dialects so the Monaco marker provider can be one piece of client
//! code that switches on language.

use serde::Serialize;

#[derive(Serialize)]
pub struct ValidateResp {
    pub ok: bool,
    pub errors: Vec<ValidateError>,
}

#[derive(Serialize, Clone)]
pub struct ValidateError {
    pub message: String,
    /// 1-indexed line where the error starts.
    pub line: u32,
    /// 1-indexed column where the error starts.
    pub column: u32,
    /// 1-indexed end position if the parser pinpoints a span. When
    /// `None`, the Monaco marker provider extends the squiggly by one
    /// character so the user has something to hover.
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
}

impl ValidateResp {
    pub fn ok() -> Self {
        Self {
            ok: true,
            errors: Vec::new(),
        }
    }

    pub fn err(error: ValidateError) -> Self {
        Self {
            ok: false,
            errors: vec![error],
        }
    }
}

/// Translate a byte offset into a 1-indexed (line, column) pair against
/// the original query string. Handles UTF-8 cleanly — the position
/// counts characters past the last newline, not bytes, so multi-byte
/// codepoints don't push columns out of register with the editor.
pub fn offset_to_line_col(source: &str, byte_offset: usize) -> (u32, u32) {
    let mut line: u32 = 1;
    let mut col: u32 = 1;
    let mut consumed_bytes = 0usize;
    for ch in source.chars() {
        if consumed_bytes >= byte_offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
        consumed_bytes += ch.len_utf8();
    }
    (line, col)
}
