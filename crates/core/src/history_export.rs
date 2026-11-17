use crate::{
    error::{AppError, Result},
    types::ResultRecord,
};
use serde::Deserialize;
use std::io::Write;
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Scope {
    Selected { ids: Vec<Uuid> },
    Filtered { query: String },
}
impl Scope {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Selected { ids } if ids.is_empty() || ids.len() > 5000 => {
                Err(AppError::invalid("请选择 1 至 5000 条记录。"))
            }
            Self::Filtered { query } if query.len() > 512 => {
                Err(AppError::invalid("搜索文字过长。"))
            }
            _ => Ok(()),
        }
    }
}
#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Txt,
    Csv,
}
impl Format {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Txt => "txt",
            Self::Csv => "csv",
        }
    }
}
pub fn io_error(_: std::io::Error) -> AppError {
    AppError::new(
        "ExportFailed",
        "未能保存导出文件，请检查可用空间和文件夹权限后重试。",
        true,
    )
}

/// Quoted RFC 4180 fields, with a leading text marker for spreadsheet-dangerous
/// values (including whitespace/control/full-width prefixes). TXT is lossless;
/// CSV is intended for viewing, not a byte-identical round-trip of those cells.
fn cell(w: &mut impl Write, value: &str) -> std::io::Result<()> {
    w.write_all(b"\"")?;
    let first = value
        .trim_start_matches(|c: char| c.is_whitespace() || c.is_control() || c == '\u{feff}')
        .chars()
        .next();
    if value.starts_with(['\t', '\r', '\n'])
        || first.is_some_and(|c| matches!(c, '=' | '+' | '-' | '@' | '＝' | '＋' | '－' | '＠'))
    {
        w.write_all(b"text: ")?;
    }
    for part in value.split_inclusive('"') {
        w.write_all(part.as_bytes())?;
        if part.ends_with('"') {
            w.write_all(b"\"")?;
        }
    }
    w.write_all(b"\"")
}
pub fn header(w: &mut impl Write, format: Format) -> Result<()> {
    w.write_all(match format {
        Format::Txt => "SpeakMore history\n\n".as_bytes(),
        Format::Csv => b"\xef\xbb\xbfsession_id,created_at_unix_ms,duration_ms,language,model_id,delivery_status,delivery_reason,polish_outcome,needs_review,raw_text,polished_text\r\n",
    }).map_err(io_error)
}
pub fn record(w: &mut impl Write, format: Format, r: &ResultRecord) -> Result<()> {
    match format {
        Format::Txt => writeln!(w,
            "Session: {}\nCreated (UTC): {}\nDuration (ms): {}\nLanguage: {}\nModel: {}\nDelivery: {}\nManual delivery reason: {}\nRefinement: {}\nNeeds review: {}\n\nOriginal:\n{}\n\nRefined:\n{}\n\n--------\n",
            r.session_id, chrono::DateTime::from_timestamp_millis(r.created_at).map(|time| time.to_rfc3339()).unwrap_or_else(|| format!("Unix milliseconds: {}", r.created_at)), r.duration_ms, r.language, r.model_id,
            r.delivery_status, r.delivery_reason.map_or("", |reason| reason.as_str()), r.polish_outcome, r.needs_review, r.raw_text,
            r.polished_text.as_deref().unwrap_or("")).map_err(io_error),
        Format::Csv => {
            let fields = [r.session_id.to_string(), r.created_at.to_string(), r.duration_ms.to_string(),
                r.language.clone(), r.model_id.clone(), r.delivery_status.clone(), r.delivery_reason.map_or("", |reason| reason.as_str()).into(), r.polish_outcome.clone(),
                r.needs_review.to_string(), r.raw_text.clone(), r.polished_text.clone().unwrap_or_default()];
            for (i, value) in fields.iter().enumerate() {
                if i > 0 { w.write_all(b",").map_err(io_error)?; }
                cell(w, value).map_err(io_error)?;
            }
            w.write_all(b"\r\n").map_err(io_error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_cannot_escape_fields_and_marks_formula_prefixes() {
        for value in [
            "=1+1",
            "+1",
            "-1",
            "@SUM(1)",
            " \t=1",
            "\u{feff}＝1",
            "\rhello",
        ] {
            let mut bytes = vec![];
            cell(&mut bytes, value).unwrap();
            assert!(bytes.starts_with(b"\"text: "));
        }
        let mut bytes = vec![];
        cell(&mut bytes, "你好,\"quote\"\r\nnext;=1").unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            "\"你好,\"\"quote\"\"\r\nnext;=1\""
        );
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::StorageFull.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert_eq!(
            header(&mut Broken, Format::Csv).unwrap_err().code,
            "ExportFailed"
        );
    }
}
