use std::collections::{BTreeMap, BTreeSet};

use krelpin_ast::ast::Annotation;
use krelpin_ast::token::{LitKind, Token, TokenKind};
use krelpin_errors::{DiagCtxt, Diagnostic};
use krelpin_span::Span;

pub const WELL_KNOWN: &[&str] = &[
    "debug_assertions",
    "edition",
    "target_arch",
    "target_endian",
    "target_env",
    "target_family",
    "target_os",
    "target_pointer_width",
    "target_vendor",
    "test",
    "unix",
    "windows",
];

#[derive(Clone, Debug, Default)]
pub struct CfgSet {
    entries: BTreeSet<(String, Option<String>)>,
    check: Option<CheckCfg>,
}

#[derive(Clone, Debug, Default)]
pub struct CheckCfg {
    names: BTreeMap<String, Option<BTreeSet<Option<String>>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Predicate {
    Option(String, Option<String>, Span),
    Not(Box<Predicate>),
    All(Vec<Predicate>),
    Any(Vec<Predicate>),
}

impl CfgSet {
    pub fn insert(&mut self, name: &str, value: Option<&str>) {
        self.entries
            .insert((name.to_owned(), value.map(str::to_owned)));
    }

    pub fn contains(&self, name: &str, value: Option<&str>) -> bool {
        self.entries
            .contains(&(name.to_owned(), value.map(str::to_owned)))
    }

    pub fn insert_spec(&mut self, spec: &str) -> Result<(), String> {
        let (name, value) = parse_option(spec)?;
        self.insert(&name, value.as_deref());
        Ok(())
    }

    pub fn add_check(&mut self, spec: &str) -> Result<(), String> {
        let check = self.check.get_or_insert_with(CheckCfg::default);
        check.parse(spec)
    }

    pub fn lines(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|(name, value)| match value {
                Some(value) => format!("{name}=\"{value}\""),
                None => name.clone(),
            })
            .collect()
    }

    pub(crate) fn evaluate(&self, annotation: &Annotation, dcx: &DiagCtxt) -> bool {
        let tokens = annotation
            .args
            .as_ref()
            .map_or(&[][..], |args| args.tokens.as_slice());
        let mut parser = PredicateParser {
            tokens,
            at: 0,
            span: annotation.span,
        };
        match parser.predicate_list() {
            Ok(predicates) if predicates.len() == 1 && parser.at == parser.tokens.len() => {
                let predicate = &predicates[0];
                self.check_names(predicate, dcx);
                self.holds(predicate)
            }
            Ok(_) => {
                dcx.emit(
                    Diagnostic::error("`@Cfg` takes exactly one condition")
                        .with_span(annotation.span)
                        .with_help("combine conditions with `all(...)` or `any(...)`"),
                );
                true
            }
            Err((message, span)) => {
                dcx.emit(Diagnostic::error(message).with_span(span).with_help(
                    "write a condition like `@Cfg(unix)`, `@Cfg(target_os = \"linux\")` \
                             or `@Cfg(not(test))`",
                ));
                true
            }
        }
    }

    fn holds(&self, predicate: &Predicate) -> bool {
        match predicate {
            Predicate::Option(name, value, _) => self.contains(name, value.as_deref()),
            Predicate::Not(inner) => !self.holds(inner),
            Predicate::All(all) => all.iter().all(|predicate| self.holds(predicate)),
            Predicate::Any(any) => any.iter().any(|predicate| self.holds(predicate)),
        }
    }

    fn check_names(&self, predicate: &Predicate, dcx: &DiagCtxt) {
        match predicate {
            Predicate::Option(name, value, span) => {
                if let Some(check) = &self.check
                    && let Some(problem) = check.problem(name, value.as_deref())
                {
                    dcx.emit(
                        Diagnostic::lint("unexpected_cfgs", problem)
                            .with_span(*span)
                            .with_help("declare it with `--check-cfg`"),
                    );
                }
            }
            Predicate::Not(inner) => self.check_names(inner, dcx),
            Predicate::All(list) | Predicate::Any(list) => {
                for predicate in list {
                    self.check_names(predicate, dcx);
                }
            }
        }
    }
}

impl CheckCfg {
    fn parse(&mut self, spec: &str) -> Result<(), String> {
        let invalid = || {
            format!(
                "invalid `--check-cfg` argument `{spec}`; expected `cfg(name, ...)` or \
                 `cfg(name, values(\"a\", \"b\"))`"
            )
        };
        let inner = spec
            .trim()
            .strip_prefix("cfg(")
            .and_then(|rest| rest.strip_suffix(')'))
            .ok_or_else(invalid)?;
        let (names, values) = match inner.find("values(") {
            Some(at) => {
                let values = inner[at..]
                    .strip_prefix("values(")
                    .and_then(|rest| rest.strip_suffix(')'))
                    .ok_or_else(invalid)?;
                (&inner[..at], Some(values))
            }
            None => (inner, None),
        };
        let values = match values {
            Some(values) if values.trim() == "any()" => None,
            Some(values) => {
                let mut set = BTreeSet::new();
                for value in values.split(',').map(str::trim).filter(|v| !v.is_empty()) {
                    if value == "none()" {
                        set.insert(None);
                        continue;
                    }
                    let value = value
                        .strip_prefix('"')
                        .and_then(|v| v.strip_suffix('"'))
                        .ok_or_else(invalid)?;
                    set.insert(Some(value.to_owned()));
                }
                Some(set)
            }
            None => Some(BTreeSet::from([None])),
        };
        for name in names.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            if !is_ident(name) {
                return Err(invalid());
            }
            match (self.names.get_mut(name), &values) {
                (Some(Some(existing)), Some(values)) => existing.extend(values.iter().cloned()),
                (Some(existing), None) => *existing = None,
                (Some(None), Some(_)) => {}
                (None, values) => {
                    self.names.insert(name.to_owned(), values.clone());
                }
            }
        }
        Ok(())
    }

    fn problem(&self, name: &str, value: Option<&str>) -> Option<String> {
        if WELL_KNOWN.contains(&name) {
            return None;
        }
        match self.names.get(name) {
            None => Some(format!("unexpected `@Cfg` condition name `{name}`")),
            Some(None) => None,
            Some(Some(values)) if values.contains(&value.map(str::to_owned)) => None,
            Some(Some(_)) => Some(match value {
                Some(value) => format!("unexpected `@Cfg` value `\"{value}\"` for `{name}`"),
                None => format!("`{name}` needs a value in `@Cfg`"),
            }),
        }
    }
}

fn parse_option(spec: &str) -> Result<(String, Option<String>), String> {
    let (name, value) = match spec.split_once('=') {
        Some((name, value)) => {
            let value = value.trim();
            let value = value
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .unwrap_or(value);
            (name.trim(), Some(value.to_owned()))
        }
        None => (spec.trim(), None),
    };
    if !is_ident(name) {
        return Err(format!(
            "invalid `--cfg` argument `{spec}`; expected `name` or `name=\"value\"`"
        ));
    }
    Ok((name.to_owned(), value))
}

fn is_ident(text: &str) -> bool {
    let mut chars = text.chars();
    chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

struct PredicateParser<'t> {
    tokens: &'t [Token],
    at: usize,
    span: Span,
}

type Parsed<T> = Result<T, (String, Span)>;

impl PredicateParser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn span(&self) -> Span {
        self.peek().map_or(self.span, |token| token.span)
    }

    fn eat(&mut self, kind: TokenKind) -> bool {
        if self.peek().is_some_and(|token| token.kind == kind) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn predicate_list(&mut self) -> Parsed<Vec<Predicate>> {
        if self.eat(TokenKind::OpenParen) {
            let list = self.comma_list()?;
            if !self.eat(TokenKind::CloseParen) {
                return Err(("expected `)` in `@Cfg`".to_owned(), self.span()));
            }
            return Ok(list);
        }
        self.comma_list()
    }

    fn comma_list(&mut self) -> Parsed<Vec<Predicate>> {
        let mut list = Vec::new();
        while self
            .peek()
            .is_some_and(|token| token.kind != TokenKind::CloseParen)
        {
            list.push(self.predicate()?);
            if !self.eat(TokenKind::Comma) {
                break;
            }
        }
        Ok(list)
    }

    fn predicate(&mut self) -> Parsed<Predicate> {
        let span = self.span();
        let Some(TokenKind::Ident(name)) = self.peek().map(|token| token.kind) else {
            return Err(("expected a condition in `@Cfg`".to_owned(), span));
        };
        self.at += 1;
        let name = name.as_str();
        match name {
            "not" | "all" | "any" if self.eat(TokenKind::OpenParen) => {
                let list = self.comma_list()?;
                if !self.eat(TokenKind::CloseParen) {
                    return Err(("expected `)` in `@Cfg`".to_owned(), self.span()));
                }
                match name {
                    "not" if list.len() == 1 => Ok(Predicate::Not(Box::new(
                        list.into_iter()
                            .next()
                            .unwrap_or(Predicate::All(Vec::new())),
                    ))),
                    "not" => Err(("`not` takes exactly one condition".to_owned(), span)),
                    "all" => Ok(Predicate::All(list)),
                    _ => Ok(Predicate::Any(list)),
                }
            }
            _ => {
                if !self.eat(TokenKind::Eq) {
                    return Ok(Predicate::Option(name.to_owned(), None, span));
                }
                match self.peek().map(|token| token.kind) {
                    Some(TokenKind::Literal(lit)) if lit.kind == LitKind::Str => {
                        self.at += 1;
                        Ok(Predicate::Option(
                            name.to_owned(),
                            Some(lit.symbol.as_str().to_owned()),
                            span,
                        ))
                    }
                    _ => Err((
                        format!("expected a string after `{name} =` in `@Cfg`"),
                        self.span(),
                    )),
                }
            }
        }
    }
}
