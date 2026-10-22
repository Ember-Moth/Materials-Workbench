//! CIF 1.1 syntax: data blocks, tags, loops, quoted strings and text
//! fields, every token with its 1-based source line.
//!
//! Supported subset: everything a CIF 1.1 data file uses. Save frames
//! (dictionaries only) are skipped with a warning; `global_` and `stop_` are
//! errors. The parser recovers from errors so that one file reports all of
//! them, but a block with a syntax error never yields a structure.

use std::collections::HashSet;

use crate::diagnostic::{Diagnostic, DiagnosticKind};

/// A value as written. Only unquoted `?` and `.` are the special values;
/// quoted, they are ordinary text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Value {
    pub text: String,
    pub quoted: bool,
    pub line: u32,
}

impl Value {
    /// `?`: the value is unknown.
    pub fn is_unknown(&self) -> bool {
        !self.quoted && self.text == "?"
    }

    /// `.`: the value does not apply.
    pub fn is_inapplicable(&self) -> bool {
        !self.quoted && self.text == "."
    }

    /// Either special value.
    pub fn is_missing(&self) -> bool {
        self.is_unknown() || self.is_inapplicable()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tag {
    /// Lowercase, since CIF tags are case-insensitive.
    pub name: String,
    pub line: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Loop {
    pub tags: Vec<Tag>,
    pub values: Vec<Value>,
    pub line: u32,
}

impl Loop {
    pub fn rows(&self) -> impl Iterator<Item = &[Value]> {
        self.values.chunks_exact(self.tags.len().max(1))
    }

    pub fn column(&self, tag: &str) -> Option<usize> {
        self.tags.iter().position(|t| t.name == tag)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Item {
    Pair { tag: Tag, value: Value },
    Loop(Loop),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Block {
    pub name: String,
    pub line: u32,
    pub items: Vec<Item>,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Default)]
pub(crate) struct Document {
    pub blocks: Vec<Block>,
    /// Problems outside any block.
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, PartialEq)]
enum TokenKind {
    Data(String),
    Loop,
    Save(String),
    Reserved(String),
    Tag(String),
    Value { text: String, quoted: bool },
    Error(DiagnosticKind),
}

#[derive(Clone, Debug, PartialEq)]
struct Token {
    kind: TokenKind,
    line: u32,
}

fn is_space(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut lines = text.lines().enumerate().map(|(i, l)| (i as u32 + 1, l));
    while let Some((line, content)) = lines.next() {
        if let Some(first) = content.strip_prefix(';') {
            // A text field runs to the next line that starts with `;`.
            let mut field = first.to_string();
            let mut closed = false;
            for (_, next) in lines.by_ref() {
                if next.starts_with(';') {
                    closed = true;
                    // Anything after the closing `;` is ordinary content, but
                    // CIF files never put any there; it is ignored.
                    break;
                }
                field.push('\n');
                field.push_str(next);
            }
            tokens.push(Token {
                kind: if closed {
                    TokenKind::Value {
                        text: field,
                        quoted: true,
                    }
                } else {
                    TokenKind::Error(DiagnosticKind::UnterminatedTextField)
                },
                line,
            });
            continue;
        }
        tokenize_line(content, line, &mut tokens);
    }
    tokens
}

fn tokenize_line(content: &str, line: u32, tokens: &mut Vec<Token>) {
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if is_space(chars[i]) || chars[i] == '\r' {
            i += 1;
            continue;
        }
        if chars[i] == '#' {
            break;
        }
        if chars[i] == '\'' || chars[i] == '"' {
            // A quote closes only when whitespace or the line end follows.
            let quote = chars[i];
            let start = i + 1;
            let end = (start..chars.len()).find(|&j| {
                chars[j] == quote && chars.get(j + 1).is_none_or(|c| is_space(*c) || *c == '\r')
            });
            let kind = match end {
                Some(end) => TokenKind::Value {
                    text: chars[start..end].iter().collect(),
                    quoted: true,
                },
                None => TokenKind::Error(DiagnosticKind::UnterminatedQuote),
            };
            tokens.push(Token { kind, line });
            i = end.map_or(chars.len(), |end| end + 1);
            continue;
        }
        let start = i;
        while i < chars.len() && !is_space(chars[i]) && chars[i] != '\r' {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        let lower = word.to_ascii_lowercase();
        let kind = if word.starts_with('_') {
            TokenKind::Tag(lower)
        } else if let Some(name) = lower.strip_prefix("data_") {
            TokenKind::Data(word[word.len() - name.len()..].to_string())
        } else if lower == "loop_" {
            TokenKind::Loop
        } else if let Some(name) = lower.strip_prefix("save_") {
            TokenKind::Save(name.to_string())
        } else if lower == "global_" || lower == "stop_" {
            TokenKind::Reserved(word)
        } else {
            TokenKind::Value {
                text: word,
                quoted: false,
            }
        };
        tokens.push(Token { kind, line });
    }
}

/// Parses CIF text into data blocks.
pub(crate) fn parse(text: &str) -> Document {
    let mut document = Document::default();
    let tokens = tokenize(text);
    let mut tokens = tokens.into_iter().peekable();
    let mut seen_tags: HashSet<String> = HashSet::new();

    // Where diagnostics go: the current block, or the file before any block.
    fn report(document: &mut Document, kind: DiagnosticKind, line: u32) {
        let diagnostic = Diagnostic::at(kind, line);
        match document.blocks.last_mut() {
            Some(block) => block.diagnostics.push(diagnostic),
            None => document.diagnostics.push(diagnostic),
        }
    }
    fn push_item(document: &mut Document, item: Item) {
        if let Some(block) = document.blocks.last_mut() {
            block.items.push(item);
        }
    }

    fn check_duplicate(document: &mut Document, seen: &mut HashSet<String>, tag: &Tag) {
        if !seen.insert(tag.name.clone()) {
            report(
                document,
                DiagnosticKind::DuplicateTag {
                    tag: tag.name.clone(),
                },
                tag.line,
            );
        }
    }

    while let Some(token) = tokens.next() {
        let line = token.line;
        if document.blocks.is_empty() && !matches!(token.kind, TokenKind::Data(_)) {
            report(&mut document, DiagnosticKind::ContentOutsideBlock, line);
            // Report once, then skip to the first block.
            while tokens
                .peek()
                .is_some_and(|t| !matches!(t.kind, TokenKind::Data(_)))
            {
                tokens.next();
            }
            continue;
        }
        match token.kind {
            TokenKind::Data(name) => {
                document.blocks.push(Block {
                    name: name.clone(),
                    line,
                    items: Vec::new(),
                    diagnostics: Vec::new(),
                });
                seen_tags.clear();
                if name.is_empty() {
                    report(&mut document, DiagnosticKind::EmptyBlockName, line);
                }
            }
            TokenKind::Tag(name) => {
                let tag = Tag { name, line };
                check_duplicate(&mut document, &mut seen_tags, &tag);
                match tokens.peek().map(|t| &t.kind) {
                    Some(TokenKind::Value { .. }) => {
                        let Some(Token {
                            kind: TokenKind::Value { text, quoted },
                            line: value_line,
                        }) = tokens.next()
                        else {
                            unreachable!("peeked a value");
                        };
                        let value = Value {
                            text,
                            quoted,
                            line: value_line,
                        };
                        push_item(&mut document, Item::Pair { tag, value });
                    }
                    _ => report(
                        &mut document,
                        DiagnosticKind::MissingValue { tag: tag.name },
                        line,
                    ),
                }
            }
            TokenKind::Loop => {
                let mut tags = Vec::new();
                while let Some(Token {
                    kind: TokenKind::Tag(_),
                    ..
                }) = tokens.peek()
                {
                    let Some(Token {
                        kind: TokenKind::Tag(name),
                        line,
                    }) = tokens.next()
                    else {
                        unreachable!("peeked a tag");
                    };
                    let tag = Tag { name, line };
                    check_duplicate(&mut document, &mut seen_tags, &tag);
                    tags.push(tag);
                }
                let mut values = Vec::new();
                while let Some(Token {
                    kind: TokenKind::Value { .. },
                    ..
                }) = tokens.peek()
                {
                    let Some(Token {
                        kind: TokenKind::Value { text, quoted },
                        line,
                    }) = tokens.next()
                    else {
                        unreachable!("peeked a value");
                    };
                    values.push(Value { text, quoted, line });
                }
                if tags.is_empty() {
                    report(&mut document, DiagnosticKind::EmptyLoop, line);
                    continue;
                }
                if values.len() % tags.len() != 0 {
                    report(
                        &mut document,
                        DiagnosticKind::LoopValueCount {
                            tags: tags.len(),
                            values: values.len(),
                        },
                        line,
                    );
                }
                push_item(&mut document, Item::Loop(Loop { tags, values, line }));
            }
            TokenKind::Save(name) => {
                // Skip the frame up to its closing `save_`.
                if !name.is_empty() {
                    report(
                        &mut document,
                        DiagnosticKind::SaveFrameIgnored { name },
                        line,
                    );
                    for next in tokens.by_ref() {
                        if next.kind == TokenKind::Save(String::new()) {
                            break;
                        }
                    }
                }
            }
            TokenKind::Reserved(word) => {
                report(&mut document, DiagnosticKind::ReservedWord { word }, line);
            }
            TokenKind::Value { text, .. } => {
                report(
                    &mut document,
                    DiagnosticKind::UnexpectedValue { text },
                    line,
                );
            }
            TokenKind::Error(kind) => report(&mut document, kind, line),
        }
    }
    if document.blocks.is_empty() {
        document
            .diagnostics
            .push(Diagnostic::new(DiagnosticKind::NoDataBlock, None));
    }
    document
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(diagnostics: &[Diagnostic]) -> Vec<(&'static str, Option<u32>)> {
        diagnostics.iter().map(|d| (d.code(), d.line())).collect()
    }

    #[test]
    fn reads_pairs_loops_and_strings() {
        let document = parse(
            "# comment\n\
             data_Test\n\
             _cell_length_a 3.905(2) # trailing comment\n\
             _Chemical_Name 'it's quoted'\n\
             _note\n\
             ;first line\n\
             second line\n\
             ;\n\
             loop_\n\
             _atom_site_label _atom_site_fract_x\n\
             Sr1 0 \"Ti 1\" .5\n\
             O1 ?\n",
        );
        assert!(document.diagnostics.is_empty());
        let block = &document.blocks[0];
        assert_eq!(block.name, "Test");
        assert!(block.diagnostics.is_empty(), "{:?}", block.diagnostics);
        let Item::Pair { tag, value } = &block.items[1] else {
            panic!("pair");
        };
        assert_eq!(tag.name, "_chemical_name");
        assert_eq!(value.text, "it's quoted");
        let Item::Pair { value, .. } = &block.items[2] else {
            panic!("pair");
        };
        assert_eq!(value.text, "first line\nsecond line");
        assert_eq!(value.line, 6);
        let Item::Loop(table) = &block.items[3] else {
            panic!("loop");
        };
        assert_eq!(table.rows().count(), 3);
        assert_eq!(table.values[2].text, "Ti 1");
        assert!(table.values[5].is_unknown());
        assert_eq!(table.values[5].line, 12);
    }

    #[test]
    fn quoted_special_values_are_text() {
        let document = parse("data_a\n_x '?'\n_y .\n");
        let values: Vec<_> = document.blocks[0]
            .items
            .iter()
            .map(|item| match item {
                Item::Pair { value, .. } => (value.is_unknown(), value.is_inapplicable()),
                Item::Loop(_) => panic!("pair"),
            })
            .collect();
        assert_eq!(values, [(false, false), (false, true)]);
    }

    #[test]
    fn each_block_is_separate() {
        let document = parse("data_one\n_a 1\ndata_two\n_a 2\n");
        assert_eq!(document.blocks.len(), 2);
        assert_eq!(document.blocks[1].line, 3);
        assert!(document.blocks[1].diagnostics.is_empty());
    }

    #[test]
    fn reports_errors_with_lines() {
        let document = parse(
            "_orphan 1\n\
             data_bad\n\
             _a 'unterminated\n\
             _b\n\
             loop_\n\
             _c _d\n\
             1 2 3\n\
             _e 1 extra\n\
             _e 2\n\
             ;never closed\n",
        );
        assert_eq!(
            codes(&document.diagnostics),
            [("syntax.content_outside_block", Some(1))]
        );
        assert_eq!(
            codes(&document.blocks[0].diagnostics),
            [
                ("syntax.missing_value", Some(3)),
                ("syntax.unterminated_quote", Some(3)),
                ("syntax.missing_value", Some(4)),
                ("syntax.loop_value_count", Some(5)),
                ("syntax.unexpected_value", Some(8)),
                ("syntax.duplicate_tag", Some(9)),
                ("syntax.unterminated_text_field", Some(10)),
            ]
        );
    }

    #[test]
    fn empty_file_has_no_block() {
        assert_eq!(
            codes(&parse("# nothing\n").diagnostics),
            [("syntax.no_data_block", None)]
        );
    }
}
