//! Minimal JSON writer.
//!
//! The document is built as a tree and written once, either compactly (the one line stdout
//! carries) or indented for `--pretty`. Numbers arrive already formatted, so a NaN or an
//! infinity can never reach the output.

use std::fmt::Write as _;

/// A JSON value.
pub enum Json {
    Null,
    Bool(bool),
    /// A number that is already formatted; the writer never parses or re-formats it.
    Number(String),
    Text(String),
    Array(Vec<Json>),
    Object(Vec<(&'static str, Json)>),
}

impl Json {
    pub fn null() -> Self {
        Json::Null
    }

    pub fn text(value: impl Into<String>) -> Self {
        Json::Text(value.into())
    }

    pub fn count(value: usize) -> Self {
        Json::Number(value.to_string())
    }

    pub fn unsigned(value: u32) -> Self {
        Json::Number(value.to_string())
    }

    /// A fixed-point number. A non-finite value is written as `0`, because JSON has no NaN.
    pub fn fixed(value: f64, decimals: usize) -> Self {
        let value = if value.is_finite() { value } else { 0.0 };
        let value = if value == 0.0 { 0.0 } else { value };
        Json::Number(format!("{:.1$}", value, decimals))
    }

    pub fn array(items: Vec<Json>) -> Self {
        Json::Array(items)
    }

    pub fn object(fields: Vec<(&'static str, Json)>) -> Self {
        Json::Object(fields)
    }

    pub fn to_compact(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, None, 0);
        out
    }

    pub fn to_pretty(&self) -> String {
        let mut out = String::new();
        self.write(&mut out, Some(2), 0);
        out
    }

    fn write(&self, out: &mut String, indent: Option<usize>, depth: usize) {
        match self {
            Json::Null => out.push_str("null"),
            Json::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
            Json::Number(value) => out.push_str(value),
            Json::Text(value) => write_text(out, value),
            Json::Array(items) => {
                if items.is_empty() {
                    out.push_str("[]");
                    return;
                }
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    newline(out, indent, depth + 1);
                    item.write(out, indent, depth + 1);
                }
                newline(out, indent, depth);
                out.push(']');
            }
            Json::Object(fields) => {
                if fields.is_empty() {
                    out.push_str("{}");
                    return;
                }
                out.push('{');
                for (index, (key, value)) in fields.iter().enumerate() {
                    if index > 0 {
                        out.push(',');
                    }
                    newline(out, indent, depth + 1);
                    write_text(out, key);
                    out.push(':');
                    if indent.is_some() {
                        out.push(' ');
                    }
                    value.write(out, indent, depth + 1);
                }
                newline(out, indent, depth);
                out.push('}');
            }
        }
    }
}

fn newline(out: &mut String, indent: Option<usize>, depth: usize) {
    if let Some(width) = indent {
        out.push('\n');
        for _ in 0..width * depth {
            out.push(' ');
        }
    }
}

fn write_text(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_writes_one_line_with_no_spaces() {
        let value = Json::object(vec![
            ("null", Json::null()),
            ("flag", Json::Bool(true)),
            ("count", Json::count(2)),
            ("text", Json::text("a b")),
            ("list", Json::array(vec![Json::fixed(1.5, 2)])),
        ]);
        assert_eq!(
            value.to_compact(),
            "{\"null\":null,\"flag\":true,\"count\":2,\"text\":\"a b\",\"list\":[1.50]}"
        );
        assert!(!value.to_compact().contains('\n'));
    }

    #[test]
    fn pretty_indents_two_spaces_per_level() {
        let value = Json::object(vec![(
            "list",
            Json::array(vec![
                Json::count(1),
                Json::object(vec![("k", Json::null())]),
            ]),
        )]);
        let expected = "{\n  \"list\": [\n    1,\n    {\n      \"k\": null\n    }\n  ]\n}";
        assert_eq!(value.to_pretty(), expected);
    }

    #[test]
    fn empty_containers_stay_on_one_line() {
        assert_eq!(Json::array(Vec::new()).to_pretty(), "[]");
        assert_eq!(Json::object(Vec::new()).to_pretty(), "{}");
    }

    #[test]
    fn text_is_escaped() {
        let value = Json::text("a\"b\\c\nd\te\u{1}");
        assert_eq!(value.to_compact(), "\"a\\\"b\\\\c\\nd\\te\\u0001\"");
    }

    #[test]
    fn numbers_are_fixed_and_finite() {
        assert_eq!(Json::fixed(1.23456, 4).to_compact(), "1.2346");
        assert_eq!(Json::fixed(f64::NAN, 2).to_compact(), "0.00");
        assert_eq!(Json::fixed(f64::INFINITY, 2).to_compact(), "0.00");
        assert_eq!(Json::fixed(-0.0, 2).to_compact(), "0.00");
    }
}
