//! Turning formula text into an [`Expr`]: lexing and parsing.

use super::functions::FUNCTIONS;
use super::*;
use crate::error::invalid;

// Lexing.

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(String),
    Text(String),
    Column(String),
    Ident(String),
    Op(&'static str),
    LParen,
    RParen,
    Comma,
}

fn lex(source: &str) -> Result<Vec<Token>> {
    let chars: Vec<char> = source.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        let token = match c {
            '(' => Token::LParen,
            ')' => Token::RParen,
            ',' => Token::Comma,
            '[' => {
                let end = chars[i..]
                    .iter()
                    .position(|&c| c == ']')
                    .ok_or_else(|| invalid!(FormulaMissingBracket))?;
                let name: String = chars[i + 1..i + end].iter().collect();
                i += end;
                Token::Column(name)
            }
            '"' => {
                let mut text = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        None => return Err(invalid!(FormulaMissingQuote)),
                        Some('"') if chars.get(i + 1) == Some(&'"') => {
                            text.push('"');
                            i += 1;
                        }
                        Some('"') => break,
                        Some(&c) => text.push(c),
                    }
                    i += 1;
                }
                Token::Text(text)
            }
            c if c.is_ascii_digit()
                || (c == '.' && chars.get(i + 1).is_some_and(char::is_ascii_digit)) =>
            {
                while chars
                    .get(i + 1)
                    .is_some_and(|c| c.is_ascii_digit() || *c == '.')
                {
                    i += 1;
                }
                Token::Number(chars[start..=i].iter().collect())
            }
            c if c.is_alphabetic() || c == '_' => {
                while chars
                    .get(i + 1)
                    .is_some_and(|c| c.is_alphanumeric() || *c == '_')
                {
                    i += 1;
                }
                Token::Ident(chars[start..=i].iter().collect())
            }
            _ => {
                let two: String = chars[i..(i + 2).min(chars.len())].iter().collect();
                let op = ["<=", ">=", "<>", "!="]
                    .into_iter()
                    .find(|op| *op == two)
                    .or_else(|| {
                        ["+", "-", "*", "/", "%", "&", "=", "<", ">"]
                            .into_iter()
                            .find(|op| op.starts_with(c))
                    })
                    .ok_or_else(|| invalid!(FormulaUnexpected, text = c))?;
                i += op.len() - 1;
                Token::Op(if op == "!=" { "<>" } else { op })
            }
        };
        tokens.push(token);
        i += 1;
    }
    Ok(tokens)
}

// Parsing (precedence climbing).

/// Deepest nesting of brackets, signs, `not` and chained operators: parsing,
/// type checking and evaluation call themselves once per level, so a formula
/// nested thousands deep (in a recipe, say) would overflow the stack.
pub const MAX_DEPTH: usize = 64;

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Levels open right now.
    depth: usize,
}

impl Parser {
    /// Opens a level, refusing past [`MAX_DEPTH`].
    fn enter(&mut self, more: usize) -> Result<()> {
        if self.depth + more > MAX_DEPTH {
            return Err(invalid!(FormulaTooDeep, max = MAX_DEPTH));
        }
        self.depth += 1;
        Ok(())
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        token
    }

    fn keyword(&self, word: &str) -> bool {
        matches!(self.peek(), Some(Token::Ident(w)) if w.eq_ignore_ascii_case(word))
    }

    fn binary_op(&self) -> Option<(BinOp, u8)> {
        let op = match self.peek()? {
            Token::Ident(w) if w.eq_ignore_ascii_case("or") => BinOp::Or,
            Token::Ident(w) if w.eq_ignore_ascii_case("and") => BinOp::And,
            Token::Op(op) => match *op {
                "=" => BinOp::Eq,
                "<>" => BinOp::Ne,
                "<" => BinOp::Lt,
                "<=" => BinOp::Le,
                ">" => BinOp::Gt,
                ">=" => BinOp::Ge,
                "&" => BinOp::Concat,
                "+" => BinOp::Add,
                "-" => BinOp::Sub,
                "*" => BinOp::Mul,
                "/" => BinOp::Div,
                "%" => BinOp::Rem,
                _ => return None,
            },
            _ => return None,
        };
        let precedence = match op {
            BinOp::Or => 1,
            BinOp::And => 2,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => 4,
            BinOp::Concat => 5,
            BinOp::Add | BinOp::Sub => 6,
            BinOp::Mul | BinOp::Div | BinOp::Rem => 7,
        };
        Some((op, precedence))
    }

    fn expression(&mut self, min: u8) -> Result<Expr> {
        self.enter(0)?;
        let mut left = self.unary()?;
        // Each operator in a chain (`a + b + c`) nests the left side once more.
        let mut chain = 0;
        while let Some((op, precedence)) = self.binary_op() {
            if precedence < min {
                break;
            }
            self.next();
            let right = self.expression(precedence + 1)?;
            chain += 1;
            if self.depth + chain > MAX_DEPTH {
                return Err(invalid!(FormulaTooDeep, max = MAX_DEPTH));
            }
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        self.depth -= 1;
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr> {
        if self.keyword("not") {
            self.next();
            // Binds looser than comparisons: `not a = b` is `not (a = b)`.
            return Ok(Expr::Not(Box::new(self.expression(3)?)));
        }
        if self.peek() == Some(&Token::Op("-")) {
            self.next();
            self.enter(0)?;
            let inner = self.unary()?;
            self.depth -= 1;
            return Ok(Expr::Neg(Box::new(inner)));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr> {
        match self.next() {
            Some(Token::Number(n)) => {
                if n.contains('.') {
                    n.parse()
                        .map(Expr::Float)
                        .map_err(|_| invalid!(FormulaNotNumber, text = n))
                } else {
                    n.parse()
                        .map(Expr::Int)
                        .map_err(|_| invalid!(FormulaTooLarge, text = n))
                }
            }
            Some(Token::Text(t)) => Ok(Expr::Text(t)),
            Some(Token::Column(c)) => Ok(Expr::Column(c)),
            Some(Token::LParen) => {
                let inner = self.expression(0)?;
                match self.next() {
                    Some(Token::RParen) => Ok(inner),
                    _ => Err(invalid!(FormulaMissingParen)),
                }
            }
            Some(Token::Ident(word)) => {
                let lower = word.to_lowercase();
                if self.peek() == Some(&Token::LParen) {
                    self.next();
                    let mut args = Vec::new();
                    if self.peek() == Some(&Token::RParen) {
                        self.next();
                    } else {
                        loop {
                            args.push(self.expression(0)?);
                            match self.next() {
                                Some(Token::Comma) => {}
                                Some(Token::RParen) => break,
                                _ => {
                                    return Err(invalid!(
                                        FormulaMissingParenAfter,
                                        function = word
                                    ));
                                }
                            }
                        }
                    }
                    let Some(&(_, min, max, usage)) =
                        FUNCTIONS.iter().find(|(name, ..)| *name == lower)
                    else {
                        return Err(invalid!(UnknownFunction, function = word));
                    };
                    if args.len() < min || args.len() > max {
                        return Err(invalid!(FormulaUsage, usage = usage));
                    }
                    return Ok(Expr::Call(lower, args));
                }
                Ok(match lower.as_str() {
                    "true" => Expr::Bool(true),
                    "false" => Expr::Bool(false),
                    "null" => Expr::Null,
                    _ => Expr::Column(word),
                })
            }
            Some(Token::Op(op)) => Err(invalid!(FormulaUnexpected, text = op)),
            Some(Token::RParen) => Err(invalid!(FormulaUnexpected, text = ")")),
            Some(Token::Comma) => Err(invalid!(FormulaUnexpected, text = ",")),
            None => Err(invalid!(FormulaIncomplete)),
        }
    }
}

/// Parses an expression.
pub fn parse(source: &str) -> Result<Expr> {
    let tokens = lex(source)?;
    if tokens.is_empty() {
        return Err(invalid!(FormulaEmpty));
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        depth: 0,
    };
    let expr = parser.expression(0)?;
    if let Some(token) = parser.peek() {
        return Err(invalid!(FormulaTrailing, text = format!("{token:?}")));
    }
    Ok(expr)
}
