//! Python's json.dumps, byte for byte, for serde_json values (insertion-ordered maps): the default separators
//! (", " and ": "), `indent=N` (items on their own lines, "," and ": "), and ensure_ascii (non-ASCII as \uXXXX).
//! The MCP server answers with exactly the text the Python server wrote.

use serde_json::Value;

/// A JSON string literal as Python writes it (ensure_ascii=True).
pub fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            c if (' '..='~').contains(&c) => out.push(c),
            c => {
                let mut buf = [0u16; 2];
                for u in c.encode_utf16(&mut buf) {
                    out.push_str(&format!("\\u{:04x}", u));
                }
            }
        }
    }
    out.push('"');
    out
}

/// json.dumps(v) (indent None) or json.dumps(v, indent=n).
pub fn dumps(v: &Value, indent: Option<usize>) -> String {
    let mut out = String::new();
    write(v, indent, 0, &mut out);
    out
}

fn write(v: &Value, indent: Option<usize>, level: usize, out: &mut String) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&string(s)),
        Value::Array(xs) => {
            if xs.is_empty() {
                out.push_str("[]");
                return;
            }
            out.push('[');
            for (i, x) in xs.iter().enumerate() {
                sep(i, indent, level + 1, out);
                write(x, indent, level + 1, out);
            }
            close(indent, level, out);
            out.push(']');
        }
        Value::Object(m) => {
            if m.is_empty() {
                out.push_str("{}");
                return;
            }
            out.push('{');
            for (i, (k, x)) in m.iter().enumerate() {
                sep(i, indent, level + 1, out);
                out.push_str(&string(k));
                out.push_str(": ");
                write(x, indent, level + 1, out);
            }
            close(indent, level, out);
            out.push('}');
        }
    }
}

fn sep(i: usize, indent: Option<usize>, level: usize, out: &mut String) {
    match indent {
        Some(n) => {
            if i > 0 {
                out.push(',');
            }
            out.push('\n');
            out.push_str(&" ".repeat(n * level));
        }
        None => {
            if i > 0 {
                out.push_str(", ");
            }
        }
    }
}

fn close(indent: Option<usize>, level: usize, out: &mut String) {
    if let Some(n) = indent {
        out.push('\n');
        out.push_str(&" ".repeat(n * level));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn like_python() {
        let v = json!({"a": [1, {"b": null}], "é": "x\u{7f}y\n", "e": [], "f": {}});
        assert_eq!(dumps(&v, None), r#"{"a": [1, {"b": null}], "\u00e9": "x\u007fy\n", "e": [], "f": {}}"#);
        assert_eq!(
            dumps(&v, Some(1)),
            "{\n \"a\": [\n  1,\n  {\n   \"b\": null\n  }\n ],\n \"\\u00e9\": \"x\\u007fy\\n\",\n \"e\": [],\n \"f\": {}\n}"
        );
        assert_eq!(string("😀"), "\"\\ud83d\\ude00\"");
    }
}
