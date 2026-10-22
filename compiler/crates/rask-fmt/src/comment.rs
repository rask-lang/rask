// SPDX-License-Identifier: (MIT OR Apache-2.0)

use rask_ast::Span;

#[derive(Debug, Clone)]
pub struct Comment {
    pub span: Span,
    pub text: String,
}

/// Sorted list of comments with a cursor for sequential consumption.
pub struct CommentList {
    comments: Vec<Comment>,
    cursor: usize,
}

impl CommentList {
    pub fn new(comments: Vec<Comment>) -> Self {
        Self { comments, cursor: 0 }
    }

    /// Take all comments whose start position is before `pos`.
    pub fn take_before(&mut self, pos: usize) -> Vec<Comment> {
        let mut result = Vec::new();
        while self.cursor < self.comments.len()
            && self.comments[self.cursor].span.start < pos
        {
            result.push(self.comments[self.cursor].clone());
            self.cursor += 1;
        }
        result
    }

    /// Peek at the next unconsumed comment without advancing.
    pub fn peek_next(&self) -> Option<&Comment> {
        self.comments.get(self.cursor)
    }

    /// Advance cursor by one (consume the peeked comment).
    pub fn advance(&mut self) -> Option<Comment> {
        if self.cursor < self.comments.len() {
            let c = self.comments[self.cursor].clone();
            self.cursor += 1;
            Some(c)
        } else {
            None
        }
    }

    /// Drain any remaining comments.
    pub fn take_rest(&mut self) -> Vec<Comment> {
        let mut result = Vec::new();
        while self.cursor < self.comments.len() {
            result.push(self.comments[self.cursor].clone());
            self.cursor += 1;
        }
        result
    }
}

/// Extract all comments from source, skipping string literals.
pub fn extract_comments(source: &str) -> Vec<Comment> {
    let mut comments = Vec::new();
    let bytes = source.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        match bytes[i] {
            b'"' => {
                i += 1;
                if i + 1 < len && bytes[i] == b'"' && bytes[i + 1] == b'"' {
                    i += 2;
                    while i + 2 < len {
                        if bytes[i] == b'"' && bytes[i + 1] == b'"' && bytes[i + 2] == b'"' {
                            i += 3;
                            break;
                        }
                        i += 1;
                    }
                } else {
                    while i < len && bytes[i] != b'"' {
                        if bytes[i] == b'\\' {
                            i += 1;
                        }
                        i += 1;
                    }
                    if i < len {
                        i += 1;
                    }
                }
            }
            b'\'' => {
                i += 1;
                if i < len && bytes[i] == b'\\' {
                    i += 2;
                } else if i < len {
                    i += 1;
                }
                if i < len && bytes[i] == b'\'' {
                    i += 1;
                }
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'/' => {
                let start = i;
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                comments.push(Comment {
                    span: Span::new(start, i),
                    text: source[start..i].to_string(),
                });
            }
            b'/' if i + 1 < len && bytes[i + 1] == b'*' => {
                let start = i;
                i += 2;
                let mut depth = 1;
                while i + 1 < len && depth > 0 {
                    if bytes[i] == b'/' && bytes[i + 1] == b'*' {
                        depth += 1;
                        i += 2;
                    } else if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                        depth -= 1;
                        i += 2;
                    } else {
                        i += 1;
                    }
                }
                comments.push(Comment {
                    span: Span::new(start, i),
                    text: source[start..i].to_string(),
                });
            }
            _ => {
                i += 1;
            }
        }
    }

    comments
}

