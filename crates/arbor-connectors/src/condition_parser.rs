//! Parses arbor's policy-condition text grammar into a `ConditionInput`, for
//! `CsvPolicyColumns::condition` (a single free-text column, the way a real
//! source would actually carry a condition -- not pre-built AST).
//!
//! Grammar (`or` binds loosest, `not` tightest; `()` groups):
//!
//! ```text
//! expr       := or_expr
//! or_expr    := and_expr ( "or" and_expr )*
//! and_expr   := unary ( "and" unary )*
//! unary      := "not" unary | "(" expr ")" | comparison
//! comparison := operand ( binop operand )?
//! binop      := "==" | "!=" | "<=" | ">=" | "<" | ">"
//!             | "in" | "contains_all" | "contains_any" | "contains"
//!             | "starts_with" | "ends_with" | "string_contains" | "like"
//!             | "in_hierarchy"
//! operand    := variable | string | number | "true" | "false" | set
//! variable   := ("principal" | "resource" | "context") ( "." ident )*
//! set        := "(" operand ( "," operand )* ")"
//! ```
//!
//! `in_hierarchy`'s right-hand side must be a quoted entity UUID (e.g.
//! `principal in_hierarchy "018e...")`), resolved against `uuid_to_index` at
//! graph-build time -- consistent with how `policies.csv`'s own
//! `principal_id`/`resource_id` columns already require literal UUIDs rather
//! than names.
//!
//! Deliberately not supported yet (no ingestion path needs them): `has_attribute`,
//! `is_type`, `in_network`. Same grammar shape extends to them later.

use arbor_types::{ArborError, ArborResult, ConditionInput, OperandInput, VariableScope};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    Str(String),
    Int(i64),
    Float(f64),
    Dot,
    Comma,
    LParen,
    RParen,
    Eq,
    Neq,
    Lt,
    Lte,
    Gt,
    Gte,
}

fn tokenize(src: &str) -> ArborResult<Vec<Token>> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut tokens = Vec::new();

    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '.' => { tokens.push(Token::Dot); i += 1; }
            ',' => { tokens.push(Token::Comma); i += 1; }
            '(' => { tokens.push(Token::LParen); i += 1; }
            ')' => { tokens.push(Token::RParen); i += 1; }
            '=' if chars.get(i + 1) == Some(&'=') => { tokens.push(Token::Eq); i += 2; }
            '!' if chars.get(i + 1) == Some(&'=') => { tokens.push(Token::Neq); i += 2; }
            '<' if chars.get(i + 1) == Some(&'=') => { tokens.push(Token::Lte); i += 2; }
            '>' if chars.get(i + 1) == Some(&'=') => { tokens.push(Token::Gte); i += 2; }
            '<' => { tokens.push(Token::Lt); i += 1; }
            '>' => { tokens.push(Token::Gt); i += 1; }
            '"' => {
                let mut s = String::new();
                i += 1;
                loop {
                    match chars.get(i) {
                        Some('"') => { i += 1; break; }
                        Some('\\') if chars.get(i + 1) == Some(&'"') => { s.push('"'); i += 2; }
                        Some(&ch) => { s.push(ch); i += 1; }
                        None => return Err(condition_err(src, "unterminated string literal")),
                    }
                }
                tokens.push(Token::Str(s));
            }
            c if c.is_ascii_digit() || (c == '-' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) => {
                let start = i;
                i += 1;
                let mut is_float = false;
                while let Some(&d) = chars.get(i) {
                    if d.is_ascii_digit() {
                        i += 1;
                    } else if d == '.' && !is_float && chars.get(i + 1).is_some_and(|n| n.is_ascii_digit()) {
                        is_float = true;
                        i += 1;
                    } else {
                        break;
                    }
                }
                let text: String = chars[start..i].iter().collect();
                if is_float {
                    let f = text.parse().map_err(|e| condition_err(src, format!("invalid float {text:?}: {e}")))?;
                    tokens.push(Token::Float(f));
                } else {
                    let n = text.parse().map_err(|e| condition_err(src, format!("invalid integer {text:?}: {e}")))?;
                    tokens.push(Token::Int(n));
                }
            }
            c if c.is_alphabetic() || c == '_' => {
                let start = i;
                while chars.get(i).is_some_and(|c| c.is_alphanumeric() || *c == '_') {
                    i += 1;
                }
                tokens.push(Token::Ident(chars[start..i].iter().collect()));
            }
            other => return Err(condition_err(src, format!("unexpected character {other:?}"))),
        }
    }
    Ok(tokens)
}

fn condition_err(src: &str, msg: impl std::fmt::Display) -> ArborError {
    ArborError::ConversionError(format!("condition {src:?}: {msg}"))
}

struct Parser<'a> {
    src: &'a str,
    tokens: Vec<Token>,
    pos: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn expect_ident(&mut self, expected: &str) -> bool {
        if let Some(Token::Ident(s)) = self.peek() {
            if s.eq_ignore_ascii_case(expected) {
                self.pos += 1;
                return true;
            }
        }
        false
    }

    fn err(&self, msg: impl std::fmt::Display) -> ArborError {
        condition_err(self.src, msg)
    }

    fn parse_expr(&mut self) -> ArborResult<ConditionInput> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> ArborResult<ConditionInput> {
        let mut parts = vec![self.parse_and()?];
        while self.expect_ident("or") {
            parts.push(self.parse_and()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { ConditionInput::Or(parts) })
    }

    fn parse_and(&mut self) -> ArborResult<ConditionInput> {
        let mut parts = vec![self.parse_unary()?];
        while self.expect_ident("and") {
            parts.push(self.parse_unary()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { ConditionInput::And(parts) })
    }

    fn parse_unary(&mut self) -> ArborResult<ConditionInput> {
        if self.expect_ident("not") {
            return Ok(ConditionInput::Not(Box::new(self.parse_unary()?)));
        }
        if matches!(self.peek(), Some(Token::LParen)) {
            self.pos += 1;
            let inner = self.parse_expr()?;
            match self.advance() {
                Some(Token::RParen) => return Ok(inner),
                _ => return Err(self.err("expected closing ')'")),
            }
        }
        self.parse_comparison()
    }

    fn parse_comparison(&mut self) -> ArborResult<ConditionInput> {
        let lhs = self.parse_operand()?;

        macro_rules! binop {
            ($variant:ident) => {{
                let rhs = self.parse_operand()?;
                return Ok(ConditionInput::$variant(lhs, rhs));
            }};
        }

        match self.peek() {
            Some(Token::Eq) => { self.pos += 1; binop!(Eq) }
            Some(Token::Neq) => { self.pos += 1; binop!(Neq) }
            Some(Token::Lt) => { self.pos += 1; binop!(Lt) }
            Some(Token::Lte) => { self.pos += 1; binop!(Lte) }
            Some(Token::Gt) => { self.pos += 1; binop!(Gt) }
            Some(Token::Gte) => { self.pos += 1; binop!(Gte) }
            Some(Token::Ident(s)) => {
                let kw = s.to_ascii_lowercase();
                match kw.as_str() {
                    "in" => { self.pos += 1; binop!(In) }
                    "contains_all" => { self.pos += 1; binop!(ContainsAll) }
                    "contains_any" => { self.pos += 1; binop!(ContainsAny) }
                    "contains" => { self.pos += 1; binop!(Contains) }
                    "starts_with" => { self.pos += 1; binop!(StartsWith) }
                    "ends_with" => { self.pos += 1; binop!(EndsWith) }
                    "string_contains" => { self.pos += 1; binop!(StringContains) }
                    "like" => { self.pos += 1; binop!(Like) }
                    "in_hierarchy" => {
                        self.pos += 1;
                        let rhs = self.parse_entity_ref_operand()?;
                        return Ok(ConditionInput::InHierarchy(lhs, rhs));
                    }
                    _ => Ok(ConditionInput::Operand(lhs)),
                }
            }
            _ => Ok(ConditionInput::Operand(lhs)),
        }
    }

    /// `in_hierarchy`'s right-hand side: a quoted entity UUID, resolved
    /// against the graph at build time -- not a variable path or scalar.
    fn parse_entity_ref_operand(&mut self) -> ArborResult<OperandInput> {
        match self.advance() {
            Some(Token::Str(s)) => {
                let uuid = Uuid::parse_str(&s).map_err(|e| self.err(format!("invalid uuid {s:?} after in_hierarchy: {e}")))?;
                Ok(OperandInput::EntityRef(uuid))
            }
            other => Err(self.err(format!("expected a quoted entity UUID after in_hierarchy, found {other:?}"))),
        }
    }

    fn parse_operand(&mut self) -> ArborResult<OperandInput> {
        match self.advance() {
            Some(Token::Str(s)) => Ok(OperandInput::String(s)),
            Some(Token::Int(n)) => Ok(OperandInput::Integer(n)),
            Some(Token::Float(f)) => Ok(OperandInput::Float(ordered_float::OrderedFloat(f))),
            Some(Token::LParen) => {
                let mut items = vec![self.parse_operand()?];
                while matches!(self.peek(), Some(Token::Comma)) {
                    self.pos += 1;
                    items.push(self.parse_operand()?);
                }
                match self.advance() {
                    Some(Token::RParen) => Ok(OperandInput::Set(items)),
                    other => Err(self.err(format!("expected closing ')' in set literal, found {other:?}"))),
                }
            }
            Some(Token::Ident(s)) => {
                let scope = match s.to_ascii_lowercase().as_str() {
                    "true" => return Ok(OperandInput::Bool(true)),
                    "false" => return Ok(OperandInput::Bool(false)),
                    "principal" => VariableScope::Principal,
                    "resource" => VariableScope::Resource,
                    "context" => VariableScope::Context,
                    other => return Err(self.err(format!(
                        "unexpected identifier {other:?} (expected a variable starting with \
                         principal/resource/context, a literal, or a set)"
                    ))),
                };
                let mut path = Vec::new();
                while matches!(self.peek(), Some(Token::Dot)) {
                    self.pos += 1;
                    match self.advance() {
                        Some(Token::Ident(segment)) => path.push(segment),
                        other => return Err(self.err(format!("expected attribute name after '.', found {other:?}"))),
                    }
                }
                Ok(OperandInput::Variable(scope, path))
            }
            other => Err(self.err(format!("expected an operand, found {other:?}"))),
        }
    }
}

/// Parses one condition expression. Returns `Err` on any syntax error --
/// there is no partial/best-effort result, matching how a malformed UUID or
/// action name elsewhere in a CSV row is already a hard ingestion error.
pub fn parse_condition(src: &str) -> ArborResult<ConditionInput> {
    let tokens = tokenize(src)?;
    let mut parser = Parser { src, tokens, pos: 0 };
    let result = parser.parse_expr()?;
    if parser.pos != parser.tokens.len() {
        return Err(condition_err(src, format!("unexpected trailing input at token {}", parser.pos)));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_equality() {
        let c = parse_condition(r#"resource.consent_flags.share_with_specialists == true"#).unwrap();
        match c {
            ConditionInput::Eq(OperandInput::Variable(VariableScope::Resource, path), OperandInput::Bool(true)) => {
                assert_eq!(path, vec!["consent_flags", "share_with_specialists"]);
            }
            other => panic!("unexpected parse: {other:?}"),
        }
    }

    #[test]
    fn and_or_not_precedence() {
        // `not` binds tighter than `and`, which binds tighter than `or`:
        // this should parse as (not A) and B) or C.
        let c = parse_condition(r#"not resource.restricted == true and principal.age >= 18 or resource.public == true"#).unwrap();
        assert!(matches!(c, ConditionInput::Or(_)));
        if let ConditionInput::Or(parts) = &c {
            assert_eq!(parts.len(), 2);
            assert!(matches!(parts[0], ConditionInput::And(_)));
        }
    }

    #[test]
    fn parenthesized_grouping_overrides_precedence() {
        let c = parse_condition(r#"resource.a == 1 and (resource.b == 2 or resource.c == 3)"#).unwrap();
        match c {
            ConditionInput::And(parts) => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(parts[1], ConditionInput::Or(_)));
            }
            other => panic!("unexpected parse: {other:?}"),
        }
    }

    #[test]
    fn contains_and_string_ops() {
        let c = parse_condition(r#"resource.tags contains "vip""#).unwrap();
        assert!(matches!(c, ConditionInput::Contains(_, OperandInput::String(s)) if s == "vip"));

        let c = parse_condition(r#"principal.email ends_with "@example.com""#).unwrap();
        assert!(matches!(c, ConditionInput::EndsWith(_, OperandInput::String(s)) if s == "@example.com"));
    }

    #[test]
    fn set_literal_for_in() {
        let c = parse_condition(r#"resource.status in ("active", "pending")"#).unwrap();
        match c {
            ConditionInput::In(_, OperandInput::Set(items)) => assert_eq!(items.len(), 2),
            other => panic!("unexpected parse: {other:?}"),
        }
    }

    #[test]
    fn in_hierarchy_requires_quoted_uuid() {
        let uuid = "018e0000-0000-7000-8000-000000000001";
        let c = parse_condition(&format!(r#"principal in_hierarchy "{uuid}""#)).unwrap();
        match c {
            ConditionInput::InHierarchy(
                OperandInput::Variable(VariableScope::Principal, path),
                OperandInput::EntityRef(id),
            ) => {
                assert!(path.is_empty());
                assert_eq!(id, Uuid::parse_str(uuid).unwrap());
            }
            other => panic!("unexpected parse: {other:?}"),
        }
    }

    #[test]
    fn in_hierarchy_rejects_non_uuid_rhs() {
        assert!(parse_condition(r#"principal in_hierarchy resource"#).is_err());
    }

    #[test]
    fn rejects_trailing_garbage() {
        assert!(parse_condition(r#"resource.a == 1 )"#).is_err());
    }

    #[test]
    fn numeric_literals_int_vs_float() {
        let c = parse_condition(r#"principal.age >= 18"#).unwrap();
        assert!(matches!(c, ConditionInput::Gte(_, OperandInput::Integer(18))));

        let c = parse_condition(r#"resource.score > 3.5"#).unwrap();
        assert!(matches!(c, ConditionInput::Gt(_, OperandInput::Float(f)) if f.0 == 3.5));
    }
}
