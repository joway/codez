//! Lightweight, stateless, per-line syntax highlighting.
//!
//! Each line is tokenized independently so highlighting composes with the
//! virtualized renderer (we only ever tokenize the visible rows, even in a
//! 100MB file). The trade-off is that multi-line constructs — block comments,
//! multi-line strings — aren't tracked across lines; for a diff/preview viewer
//! that's an acceptable approximation.

use std::path::Path;

use egui::Color32;

use crate::theme;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Rust,
    CLike,
    Python,
    Shell,
    Web, // js/ts/json/css
    Plain,
}

impl Lang {
    pub fn from_path(path: &Path) -> Lang {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        match ext.as_str() {
            "rs" => Lang::Rust,
            "c" | "h" | "cpp" | "cc" | "hpp" | "cxx" | "java" | "go" | "swift" | "kt" => {
                Lang::CLike
            }
            "py" | "rb" => Lang::Python,
            "sh" | "bash" | "zsh" | "toml" | "yaml" | "yml" | "ini" | "cfg" => Lang::Shell,
            "js" | "ts" | "jsx" | "tsx" | "json" | "css" | "scss" | "html" => Lang::Web,
            _ => Lang::Plain,
        }
    }

    /// The line-comment marker for this language, if any.
    fn line_comment(self) -> &'static str {
        match self {
            Lang::Python | Lang::Shell => "#",
            Lang::Plain => "\0", // never matches
            _ => "//",
        }
    }
}

/// A contiguous colored span as a byte range into the source line.
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub color: Color32,
}

/// Tokenize `line`, returning contiguous spans that fully cover it.
#[allow(unused_assignments)] // `last` is reset by the final flush! for symmetry
pub fn spans(line: &str, lang: Lang) -> Vec<Span> {
    let mut out = Vec::new();
    if lang == Lang::Plain || line.is_empty() {
        out.push(Span {
            start: 0,
            end: line.len(),
            color: theme::TEXT,
        });
        return out;
    }

    let comment = lang.line_comment();
    let bytes = line.as_bytes();
    let mut chars = line.char_indices().peekable();
    let mut last = 0usize;

    // Emit a default-colored gap [last, to) if non-empty.
    macro_rules! flush {
        ($to:expr) => {{
            let to = $to;
            if to > last {
                out.push(Span {
                    start: last,
                    end: to,
                    color: theme::TEXT,
                });
            }
            last = to;
        }};
    }

    while let Some(&(i, c)) = chars.peek() {
        // Line comment to end of line.
        if comment != "\0" && line[i..].starts_with(comment) {
            flush!(i);
            out.push(Span {
                start: i,
                end: line.len(),
                color: theme::COMMENT,
            });
            last = line.len();
            break;
        }

        if c == '"' || c == '\'' {
            flush!(i);
            let quote = c;
            chars.next(); // opening quote
            let mut end = line.len();
            let mut escaped = false;
            for (j, d) in chars.by_ref() {
                if escaped {
                    escaped = false;
                } else if d == '\\' {
                    escaped = true;
                } else if d == quote {
                    end = j + d.len_utf8();
                    break;
                }
            }
            out.push(Span {
                start: i,
                end,
                color: theme::STRING,
            });
            last = end;
            continue;
        }

        if c.is_ascii_digit() {
            flush!(i);
            let mut end = i + 1;
            while let Some(&(j, d)) = chars.peek() {
                if d.is_ascii_alphanumeric() || d == '.' || d == '_' {
                    end = j + d.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            out.push(Span {
                start: i,
                end,
                color: theme::NUMBER,
            });
            last = end;
            continue;
        }

        if c.is_alphabetic() || c == '_' {
            flush!(i);
            let mut end = i + c.len_utf8();
            while let Some(&(j, d)) = chars.peek() {
                if d.is_alphanumeric() || d == '_' {
                    end = j + d.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let word = &line[i..end];
            let color = if is_keyword(word) {
                theme::KEYWORD
            } else if bytes.get(end) == Some(&b'(') {
                theme::IDENT_FN
            } else {
                theme::TEXT
            };
            out.push(Span {
                start: i,
                end,
                color,
            });
            last = end;
            continue;
        }

        chars.next();
    }

    flush!(line.len());
    out
}

/// A union of common keywords across the supported languages. A viewer can
/// afford the occasional false positive (e.g. `match` in a non-Rust file).
fn is_keyword(w: &str) -> bool {
    matches!(
        w,
        "fn" | "let" | "mut" | "const" | "static" | "struct" | "enum" | "trait" | "impl"
            | "pub" | "use" | "mod" | "match" | "if" | "else" | "for" | "while" | "loop"
            | "return" | "break" | "continue" | "where" | "as" | "ref" | "move" | "async"
            | "await" | "dyn" | "self" | "Self" | "super" | "crate" | "type" | "unsafe"
            | "extern" | "true" | "false" | "None" | "Some" | "Ok" | "Err"
            | "def" | "class" | "import" | "from" | "lambda" | "pass" | "with" | "yield"
            | "and" | "or" | "not" | "in" | "is" | "elif" | "try" | "except" | "finally"
            | "raise" | "global" | "nonlocal" | "del" | "assert"
            | "function" | "var" | "new" | "this" | "typeof" | "instanceof" | "void"
            | "null" | "undefined" | "export" | "default" | "extends" | "super_"
            | "int" | "char" | "long" | "short" | "float" | "double" | "bool" | "boolean"
            | "string" | "unsigned" | "signed" | "public" | "private" | "protected"
            | "namespace" | "template" | "typename" | "virtual" | "override" | "final"
            | "package" | "func" | "go" | "defer" | "chan" | "interface" | "map" | "range"
            | "switch" | "case" | "do" | "then" | "fi" | "done" | "echo" | "local"
            | "export_" | "throw" | "catch" | "finally_" | "abstract"
    )
}
