//! Python `repr()`, byte for byte, for the values Kern hashes. `Def.hash`, `content_hash`, the why interface anchor
//! and the report cache key are sha256 over Python's repr of nested tuples (kern/syntax.py), and every `.why` anchor
//! and `.strength` baseline depends on them, so the Rust toolchain builds the same text.

use sha2::{Digest, Sha256};

/// A Python value, as far as hashing needs one.
#[derive(Clone, Debug, PartialEq)]
pub enum Py {
    None,
    Bool(bool),
    Int(i128),
    Str(String),
    Tuple(Vec<Py>),
    List(Vec<Py>),
}

impl Py {
    pub fn s(x: &str) -> Py {
        Py::Str(x.to_string())
    }
}

pub fn repr(v: &Py) -> String {
    let mut out = String::new();
    write(v, &mut out);
    out
}

fn write(v: &Py, out: &mut String) {
    match v {
        Py::None => out.push_str("None"),
        Py::Bool(b) => out.push_str(if *b { "True" } else { "False" }),
        Py::Int(n) => out.push_str(&n.to_string()),
        Py::Str(s) => out.push_str(&repr_str(s)),
        Py::Tuple(items) => {
            out.push('(');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(x, out);
            }
            if items.len() == 1 {
                out.push(',');
            }
            out.push(')');
        }
        Py::List(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write(x, out);
            }
            out.push(']');
        }
    }
}

/// Python's str repr: single quotes unless the text has a ' and no ", the usual escapes, \xNN / \uNNNN / \UNNNNNNNN
/// for non-printable characters.
pub fn repr_str(s: &str) -> String {
    let quote = if s.contains('\'') && !s.contains('"') { '"' } else { '\'' };
    let mut out = String::new();
    out.push(quote);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if is_printable(c) => out.push(c),
            c => {
                let n = c as u32;
                if n < 0x100 {
                    out.push_str(&format!("\\x{:02x}", n));
                } else if n < 0x10000 {
                    out.push_str(&format!("\\u{:04x}", n));
                } else {
                    out.push_str(&format!("\\U{:08x}", n));
                }
            }
        }
    }
    out.push(quote);
    out
}

/// Close to Python's str.isprintable for the characters Kern sources contain: ASCII controls, DEL, C1 controls and
/// the Unicode separators other than the plain space are not printable.
fn is_printable(c: char) -> bool {
    let n = c as u32;
    if n < 0x20 || n == 0x7f || (0x80..0xa0).contains(&n) {
        return false;
    }
    if c == ' ' {
        return true;
    }
    !(c.is_whitespace() || n == 0xad || (0x200b..=0x200f).contains(&n) || (0x2028..=0x202e).contains(&n)
        || (0x2060..=0x2064).contains(&n) || n == 0xfeff || (0xd800..=0xdfff).contains(&n))
}

/// The first `n` hex digits of sha256(text).
pub fn sha_hex(text: &str, n: usize) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    hex[..n].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let t = Py::Tuple(vec![Py::s("a"), Py::Int(-3), Py::Bool(false), Py::None, Py::List(vec![])]);
        assert_eq!(repr(&t), "('a', -3, False, None, [])");
        assert_eq!(repr(&Py::Tuple(vec![Py::Int(1)])), "(1,)");
        assert_eq!(repr(&Py::Tuple(vec![])), "()");
        assert_eq!(repr_str("it's"), "\"it's\"");
        assert_eq!(repr_str("'\""), "'\\'\"'");
        assert_eq!(repr_str("a\\b\n\x01é"), "'a\\\\b\\n\\x01é'");
    }
}
