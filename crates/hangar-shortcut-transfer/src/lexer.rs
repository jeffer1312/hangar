//! Palavras de uma linha de shell como o `shlex.shlex(posix=True, punctuation_chars=";&|<>()")` com
//! `whitespace_split`: pontuação junta vira uma palavra (`&&`, `2>&1` dá `2`, `>&`, `1`) e `#` comenta.
use std::str::Chars;

const PUNCTUATION: &str = ";&|<>()";
const WHITESPACE: &str = " \t\r\n";

/// Aspas sem fechar ou barra invertida no fim da linha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LexError;

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Space,
    Word,
    Punctuation,
    Quote(char),
    Escape,
    End,
}

pub(crate) struct Lexer<'a> {
    chars: Chars<'a>,
    pushback: Vec<char>,
    state: State,
}

/// Todas as palavras de `line`.
///
/// # Errors
/// [`LexError`] onde o `shlex` levantaria `ValueError`.
pub fn split_line(line: &str) -> Result<Vec<String>, LexError> {
    let mut lexer = Lexer::new(line);
    let mut words = Vec::new();
    while let Some(word) = lexer.next_word()? {
        words.push(word);
    }
    Ok(words)
}

impl<'a> Lexer<'a> {
    pub(crate) fn new(line: &'a str) -> Self {
        Self { chars: line.chars(), pushback: Vec::new(), state: State::Space }
    }

    pub(crate) fn next_word(&mut self) -> Result<Option<String>, LexError> {
        let mut quoted = false;
        let mut escaped_from = State::Word;
        let mut token = String::new();
        loop {
            let next = self.pushback.pop().or_else(|| self.chars.next());
            match self.state {
                State::End => {
                    token.clear();
                    break;
                }
                State::Space => match next {
                    None => {
                        self.state = State::End;
                        break;
                    }
                    Some(c) if WHITESPACE.contains(c) => {
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some('#') => self.skip_line(),
                    Some('\\') => {
                        escaped_from = State::Word;
                        self.state = State::Escape;
                    }
                    Some(c) if PUNCTUATION.contains(c) => {
                        token.push(c);
                        self.state = State::Punctuation;
                    }
                    Some(c @ ('\'' | '"')) => self.state = State::Quote(c),
                    Some(c) => {
                        token.push(c);
                        self.state = State::Word;
                    }
                },
                State::Quote(quote) => {
                    quoted = true;
                    match next {
                        None => return Err(LexError),
                        Some(c) if c == quote => self.state = State::Word,
                        Some('\\') if quote == '"' => {
                            escaped_from = State::Quote(quote);
                            self.state = State::Escape;
                        }
                        Some(c) => token.push(c),
                    }
                }
                State::Escape => {
                    let Some(c) = next else { return Err(LexError) };
                    // Dentro de aspas só a própria aspa e a barra se escapam; o resto guarda a barra.
                    if matches!(escaped_from, State::Quote(quote) if c != '\\' && c != quote) {
                        token.push('\\');
                    }
                    token.push(c);
                    self.state = escaped_from;
                }
                State::Word | State::Punctuation => match next {
                    None => {
                        self.state = State::End;
                        break;
                    }
                    Some(c) if WHITESPACE.contains(c) => {
                        self.state = State::Space;
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some('#') => {
                        self.skip_line();
                        self.state = State::Space;
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                    Some(c) if self.state == State::Punctuation => {
                        if PUNCTUATION.contains(c) {
                            token.push(c);
                        } else {
                            self.pushback.push(c);
                            self.state = State::Space;
                            break;
                        }
                    }
                    Some(c @ ('\'' | '"')) => self.state = State::Quote(c),
                    Some('\\') => {
                        escaped_from = State::Word;
                        self.state = State::Escape;
                    }
                    Some(c) if !PUNCTUATION.contains(c) => token.push(c),
                    Some(c) => {
                        self.pushback.push(c);
                        self.state = State::Space;
                        if !token.is_empty() || quoted {
                            break;
                        }
                    }
                },
            }
        }
        Ok((quoted || !token.is_empty()).then_some(token))
    }

    fn skip_line(&mut self) {
        for c in self.chars.by_ref() {
            if c == '\n' {
                break;
            }
        }
    }
}
