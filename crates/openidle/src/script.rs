//! Reading script files into blocks, GlassBox-style:
//!
//! ```text
//! generator Miner          # a kind and an id
//!     label Gold miner     # lines: a key and the rest
//!     cost Gold 10 * 1.15 ^ owned
//! end
//! ```
//!
//! `#` starts a comment (at the start of a line or after a space). `$name` is a parameter,
//! replaced before anything is read; `$$` is a dollar sign.

use std::collections::BTreeMap;
use std::fmt;

/// An error, with where it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Error {
    pub file: String,
    pub line: usize,
    pub msg: String,
}

impl Error {
    pub fn new(file: &str, line: usize, msg: impl Into<String>) -> Error {
        Error {
            file: file.to_string(),
            line,
            msg: msg.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.line > 0 {
            write!(f, "{}:{}: {}", self.file, self.line, self.msg)
        } else if !self.file.is_empty() {
            write!(f, "{}: {}", self.file, self.msg)
        } else {
            f.write_str(&self.msg)
        }
    }
}

impl std::error::Error for Error {}

#[derive(Clone, Debug)]
pub struct Line {
    pub no: usize,
    pub key: String,
    pub rest: String,
}

#[derive(Clone, Debug)]
pub struct Block {
    pub kind: String,
    pub id: String,
    pub file: String,
    pub no: usize,
    pub lines: Vec<Line>,
}

impl Block {
    pub fn err(&self, line: &Line, msg: impl Into<String>) -> Error {
        Error::new(&self.file, line.no, msg)
    }
}

pub const KINDS: &[&str] = &[
    "resource",
    "generator",
    "rule",
    "upgrade",
    "action",
    "milestone",
    "prestige",
];

fn strip_comment(line: &str) -> &str {
    let b = line.as_bytes();
    for (i, &c) in b.iter().enumerate() {
        if c == b'#' && (i == 0 || b[i - 1].is_ascii_whitespace()) {
            return &line[..i];
        }
    }
    line
}

/// Replaces `$name` by the parameter's value in one line of text.
pub fn substitute(text: &str, params: &BTreeMap<String, String>) -> Result<String, String> {
    let mut out = String::with_capacity(text.len());
    let mut it = text.char_indices().peekable();
    while let Some((_, c)) = it.next() {
        if c != '$' {
            out.push(c);
            continue;
        }
        if let Some(&(_, '$')) = it.peek() {
            it.next();
            out.push('$');
            continue;
        }
        let mut name = String::new();
        while let Some(&(_, d)) = it.peek() {
            if d.is_alphanumeric() || d == '_' {
                name.push(d);
                it.next();
            } else {
                break;
            }
        }
        if name.is_empty() {
            out.push('$');
            continue;
        }
        let v = params
            .get(&name)
            .ok_or_else(|| format!("no parameter `{name}` (params.toml declares them)"))?;
        out.push_str(v);
    }
    Ok(out)
}

/// Substitutes parameters in a whole file, leaving comments alone.
pub fn substitute_file(file: &str, text: &str, params: &BTreeMap<String, String>) -> Result<String, Error> {
    let mut out = String::with_capacity(text.len());
    for (i, raw) in text.lines().enumerate() {
        let code = strip_comment(raw);
        let line = substitute(code, params).map_err(|m| Error::new(file, i + 1, m))?;
        out.push_str(&line);
        out.push_str(&raw[code.len()..]);
        out.push('\n');
    }
    Ok(out)
}

/// The blocks of one file (parameters already substituted).
pub fn blocks(file: &str, text: &str) -> Result<Vec<Block>, Error> {
    let mut out = Vec::new();
    let mut cur: Option<Block> = None;
    for (i, raw) in text.lines().enumerate() {
        let no = i + 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        let (key, rest) = match line.split_once(char::is_whitespace) {
            Some((k, r)) => (k, r.trim()),
            None => (line, ""),
        };
        match cur.as_mut() {
            None => {
                let kind = match key {
                    "unitRule" => "rule",
                    k => k,
                };
                if !KINDS.contains(&kind) {
                    return Err(Error::new(
                        file,
                        no,
                        format!("`{key}` is not a block; blocks are {}", KINDS.join(", ")),
                    ));
                }
                let id = rest;
                if !valid_id(id) {
                    return Err(Error::new(
                        file,
                        no,
                        format!("`{kind}` needs an id of letters, digits and _ (not `{id}`)"),
                    ));
                }
                cur = Some(Block {
                    kind: kind.into(),
                    id: id.into(),
                    file: file.into(),
                    no,
                    lines: Vec::new(),
                });
            }
            Some(b) => {
                if key == "end" {
                    out.push(cur.take().unwrap());
                } else if KINDS.contains(&key) && valid_id(rest) && !rest.is_empty() {
                    return Err(Error::new(file, no, format!("`end` missing for {} {}", b.kind, b.id)));
                } else {
                    b.lines.push(Line {
                        no,
                        key: key.into(),
                        rest: rest.into(),
                    });
                }
            }
        }
    }
    if let Some(b) = cur {
        return Err(Error::new(file, b.no, format!("`end` missing for {} {}", b.kind, b.id)));
    }
    Ok(out)
}

pub fn valid_id(s: &str) -> bool {
    let mut c = s.chars();
    matches!(c.next(), Some(f) if f.is_alphabetic() || f == '_') && c.all(|x| x.is_alphanumeric() || x == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_blocks() {
        let b = blocks("a.idle", "# hi\nresource Gold\n  label Gold # coins\n  start 5\nend\n").unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].id, "Gold");
        assert_eq!(b[0].lines[0].rest, "Gold");
        assert_eq!(b[0].lines[1].no, 4);
        assert!(blocks("a", "resource Gold\n").unwrap_err().msg.contains("end"));
        assert!(blocks("a", "thing X\nend").is_err());
    }

    #[test]
    fn parameters() {
        let p: BTreeMap<_, _> = [("rate".to_string(), "2.5".to_string())].into();
        let t = substitute_file("f", "out Gold $rate  # $nothere\ncost $$", &p).unwrap();
        assert_eq!(t, "out Gold 2.5  # $nothere\ncost $\n");
        assert!(substitute_file("f", "x $nope", &p).is_err());
    }
}
