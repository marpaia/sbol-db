//! Shared output helpers used across every subcommand handler. Centralises
//! pretty-printed JSON, simple aligned tables, and short status lines so
//! the handler bodies stay focused on domain calls instead of formatting.

use std::io::Write;

use anyhow::Result;
use serde::Serialize;

/// Pretty-print a value as JSON to stdout, followed by a newline.
pub fn print_json<T: Serialize>(value: &T) -> Result<()> {
    let body = serde_json::to_string_pretty(value)?;
    println!("{body}");
    Ok(())
}

/// Print a value as compact JSON terminated by `\n` — the right shape for
/// newline-delimited streams (e.g. `export-all`, `sequences search-batch`).
pub fn write_jsonl<W: Write, T: Serialize>(out: &mut W, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *out, value)?;
    out.write_all(b"\n")?;
    Ok(())
}

/// Render a simple left-aligned table to stdout. Columns are sized to the
/// widest cell in each column. Suitable for short operator-facing tables
/// where reaching for a real TTY layout library would be overkill.
#[allow(dead_code)] // populated by later inspection/jobs tracks
pub fn print_table(headers: &[&str], rows: &[Vec<String>]) {
    let cols = headers.len();
    let mut widths = headers.iter().map(|h| h.len()).collect::<Vec<_>>();
    for row in rows {
        for (i, cell) in row.iter().take(cols).enumerate() {
            if cell.len() > widths[i] {
                widths[i] = cell.len();
            }
        }
    }
    let mut header_line = String::new();
    for (i, h) in headers.iter().enumerate() {
        if i > 0 {
            header_line.push_str("  ");
        }
        header_line.push_str(&format!("{:<width$}", h, width = widths[i]));
    }
    println!("{header_line}");
    let total: usize = widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1);
    println!("{}", "-".repeat(total));
    for row in rows {
        let mut line = String::new();
        for (i, cell) in row.iter().take(cols).enumerate() {
            if i > 0 {
                line.push_str("  ");
            }
            line.push_str(&format!("{:<width$}", cell, width = widths[i]));
        }
        println!("{line}");
    }
}
