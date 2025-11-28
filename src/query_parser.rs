//! KQL (KDL Query Language) parser using winnow.
//!
//! This module provides a KQL parser that uses the winnow parser combinator
//! library, matching the v2_parser.rs patterns.

use std::sync::Arc;

use miette::Severity;
use winnow::{
    combinator::{alt, cut_err, delimited, opt, peek, preceded, repeat, separated, terminated},
    error::{AddContext, ErrMode, ErrorKind, ParserError},
    prelude::*,
    stream::{Location, Stream},
    token::{one_of, take_while},
    LocatingSlice,
};

use crate::{KdlDiagnostic, KdlValue};

// Re-use the query types from the main query module
pub(crate) use crate::query::{
    KdlQuery, KdlQueryAttributeOp, KdlQueryMatcher, KdlQueryMatcherAccessor,
    KdlQueryMatcherDetails, KdlQuerySelector, KdlQuerySelectorSegment, KdlSegmentCombinator,
};

type Input<'a> = LocatingSlice<&'a str>;
type PResult<T> = winnow::PResult<T, QueryParseError>;

/// Error type for query parsing.
#[derive(Debug, Clone, Default)]
pub(crate) struct QueryParseError {
    pub(crate) message: Option<String>,
    pub(crate) label: Option<String>,
    pub(crate) help: Option<String>,
    pub(crate) offset: usize,
    pub(crate) len: usize,
}

impl<I: Stream> ParserError<I> for QueryParseError {
    fn from_error_kind(_input: &I, _kind: ErrorKind) -> Self {
        Self::default()
    }

    fn append(
        self,
        _input: &I,
        _token_start: &<I as Stream>::Checkpoint,
        _kind: ErrorKind,
    ) -> Self {
        self
    }
}

impl<I: Stream + Location> AddContext<I, &'static str> for QueryParseError {
    fn add_context(
        mut self,
        input: &I,
        _token_start: &<I as Stream>::Checkpoint,
        ctx: &'static str,
    ) -> Self {
        if self.label.is_none() {
            self.label = Some(ctx.to_string());
        }
        self.offset = input.location();
        self
    }
}

/// Parse a KQL query string.
pub(crate) fn parse_query(input: &str) -> Result<KdlQuery, KdlDiagnostic> {
    let mut input = LocatingSlice::new(input);
    match query.parse_next(&mut input) {
        Ok(q) => {
            // Check for trailing content
            wss.parse_next(&mut input).ok();
            if !input.is_empty() {
                return Err(KdlDiagnostic {
                    input: Arc::new(input.to_string()),
                    span: (input.location()..input.location() + 1).into(),
                    message: Some("Unexpected content after query".into()),
                    label: Some("unexpected".into()),
                    help: Some("Remove trailing content or check query syntax".into()),
                    severity: Severity::Error,
                });
            }
            Ok(q)
        }
        Err(e) => {
            let err = match e {
                ErrMode::Backtrack(e) | ErrMode::Cut(e) => e,
                ErrMode::Incomplete(_) => QueryParseError::default(),
            };
            Err(KdlDiagnostic {
                input: Arc::new(input.to_string()),
                span: (err.offset..err.offset + err.len.max(1)).into(),
                message: err.message,
                label: err.label,
                help: err.help.or(Some(
                    "The syntax for queries is '(type)nodename[prop=value], another'. See QUERY-SPEC.md"
                        .into(),
                )),
                severity: Severity::Error,
            })
        }
    }
}

/// `query := selector (',' selector)*`
fn query(input: &mut Input<'_>) -> PResult<KdlQuery> {
    separated(1.., query_selector, (wss, ",", wss))
        .map(KdlQuery)
        .parse_next(input)
}

/// Parse a single selector.
fn query_selector(input: &mut Input<'_>) -> PResult<KdlQuerySelector> {
    let mut segments = Vec::new();
    let mut is_scope = true;

    loop {
        wss.parse_next(input)?;
        let matchers = node_matchers(is_scope).parse_next(input)?;
        wss.parse_next(input)?;
        let op = opt(segment_combinator).parse_next(input)?;

        let is_last = op.is_none();
        segments.push(KdlQuerySelectorSegment {
            op,
            matcher: KdlQueryMatcher(matchers),
        });

        if is_last {
            break;
        }
        is_scope = false;
    }

    wss.parse_next(input)?;
    Ok(KdlQuerySelector(segments))
}

/// Parse segment combinator: `>`, `>>`, `+`, `++`
fn segment_combinator(input: &mut Input<'_>) -> PResult<KdlSegmentCombinator> {
    alt((
        ">>".map(|_| KdlSegmentCombinator::Descendant),
        ">".map(|_| KdlSegmentCombinator::Child),
        "++".map(|_| KdlSegmentCombinator::Sibling),
        "+".map(|_| KdlSegmentCombinator::Neighbor),
    ))
    .parse_next(input)
}

/// Parse node matchers within a selector segment.
fn node_matchers(
    is_scope: bool,
) -> impl FnMut(&mut Input<'_>) -> PResult<Vec<KdlQueryMatcherDetails>> {
    move |input: &mut Input<'_>| {
        let mut matchers = Vec::new();
        wss.parse_next(input)?;

        // Check for scope() accessor
        let start = input.checkpoint();
        if let Some(()) = opt(scope_accessor).parse_next(input)? {
            if is_scope {
                matchers.push(KdlQueryMatcherDetails {
                    op: KdlQueryAttributeOp::Equal,
                    accessor: KdlQueryMatcherAccessor::Scope,
                    value: None,
                });
                return Ok(matchers);
            } else {
                input.reset(&start);
                return Err(ErrMode::Cut(QueryParseError {
                    message: Some("scope() must be the first item in a selector".into()),
                    label: Some("scope()".into()),
                    help: Some("Move scope() to the beginning".into()),
                    offset: input.location(),
                    len: 7,
                }));
            }
        }

        // Check for type annotation `(type)`
        if let Some(details) = opt(annotation_matcher).parse_next(input)? {
            matchers.push(details);
            // Can't have two annotations
            if opt(annotation_matcher).parse_next(input)?.is_some() {
                return Err(ErrMode::Cut(QueryParseError {
                    message: Some("Only one type annotation per selector".into()),
                    label: Some("type annotation".into()),
                    help: Some("Syntax: (type)node[attr=value]".into()),
                    offset: input.location(),
                    len: 1,
                }));
            }
        }

        // Check for node name
        if let Some(ident) = opt(identifier).parse_next(input)? {
            matchers.push(KdlQueryMatcherDetails {
                op: KdlQueryAttributeOp::Equal,
                value: Some(KdlValue::String(ident)),
                accessor: KdlQueryMatcherAccessor::Node,
            });
        }

        // No type annotation after node name
        if opt(peek(annotation_matcher)).parse_next(input)?.is_some() {
            return Err(ErrMode::Cut(QueryParseError {
                message: Some("Type annotation must come before node name".into()),
                label: Some("type annotation".into()),
                help: Some("Syntax: (type)node[attr=value]".into()),
                offset: input.location(),
                len: 1,
            }));
        }

        // Parse attribute matchers `[...]`
        let mut attrs: Vec<KdlQueryMatcherDetails> =
            repeat(0.., attribute_matcher).parse_next(input)?;
        matchers.append(&mut attrs);

        if matchers.is_empty() {
            return Err(ErrMode::Backtrack(QueryParseError {
                message: Some("Empty node matcher".into()),
                label: Some("node matcher".into()),
                help: Some("Provide at least a node name or attribute matcher".into()),
                offset: input.location(),
                len: 0,
            }));
        }

        Ok(matchers)
    }
}

/// Parse `scope()` accessor.
fn scope_accessor(input: &mut Input<'_>) -> PResult<()> {
    ("scope(", wss, ")").void().parse_next(input)
}

/// Parse annotation/type matcher `(type)`.
fn annotation_matcher(input: &mut Input<'_>) -> PResult<KdlQueryMatcherDetails> {
    delimited("(", (wss, opt(identifier), wss), ")")
        .map(|(_, ty, _)| KdlQueryMatcherDetails {
            op: KdlQueryAttributeOp::Equal,
            value: ty.map(KdlValue::String),
            accessor: KdlQueryMatcherAccessor::Annotation,
        })
        .parse_next(input)
}

/// Parse attribute matcher `[...]`.
fn attribute_matcher(input: &mut Input<'_>) -> PResult<KdlQueryMatcherDetails> {
    delimited(
        "[",
        (wss, attribute_matcher_inner, wss),
        cut_err("]").context("closing ']'"),
    )
    .map(|(_, m, _)| m)
    .parse_next(input)
}

/// Parse inner content of attribute matcher.
fn attribute_matcher_inner(input: &mut Input<'_>) -> PResult<KdlQueryMatcherDetails> {
    let accessor = opt(accessor).parse_next(input)?;

    if let Some(xsr) = accessor {
        wss.parse_next(input)?;
        let op = opt(attribute_op).parse_next(input)?;
        wss.parse_next(input)?;

        if let Some(op) = op {
            let value = opt(kdl_value).parse_next(input)?;
            if let Some(value) = value {
                // String operators require string values
                if matches!(
                    op,
                    KdlQueryAttributeOp::StartsWith
                        | KdlQueryAttributeOp::EndsWith
                        | KdlQueryAttributeOp::Contains
                ) && !matches!(value, KdlValue::String(_))
                {
                    return Err(ErrMode::Cut(QueryParseError {
                        message: Some("String operators require string values".into()),
                        label: Some("non-string value".into()),
                        help: Some("Use ^=, $=, *= only with strings".into()),
                        offset: input.location(),
                        len: 1,
                    }));
                }
                Ok(KdlQueryMatcherDetails {
                    op,
                    value: Some(value),
                    accessor: xsr,
                })
            } else {
                Err(ErrMode::Cut(QueryParseError {
                    message: Some("Expected value after operator".into()),
                    label: Some("operator value".into()),
                    help: Some("Provide a valid KDL value".into()),
                    offset: input.location(),
                    len: 0,
                }))
            }
        } else {
            Ok(KdlQueryMatcherDetails {
                op: KdlQueryAttributeOp::Equal,
                value: None,
                accessor: xsr,
            })
        }
    } else {
        Ok(KdlQueryMatcherDetails {
            op: KdlQueryAttributeOp::Equal,
            value: None,
            accessor: KdlQueryMatcherAccessor::Node,
        })
    }
}

/// Parse attribute operator.
fn attribute_op(input: &mut Input<'_>) -> PResult<KdlQueryAttributeOp> {
    alt((
        "!=".map(|_| KdlQueryAttributeOp::NotEqual),
        ">=".map(|_| KdlQueryAttributeOp::Gte),
        "<=".map(|_| KdlQueryAttributeOp::Lte),
        "^=".map(|_| KdlQueryAttributeOp::StartsWith),
        "$=".map(|_| KdlQueryAttributeOp::EndsWith),
        "*=".map(|_| KdlQueryAttributeOp::Contains),
        "=".map(|_| KdlQueryAttributeOp::Equal),
        ">".map(|_| KdlQueryAttributeOp::Gt),
        "<".map(|_| KdlQueryAttributeOp::Lt),
    ))
    .parse_next(input)
}

/// Parse an accessor: `type()`, `arg()`, `arg(n)`, `prop(name)`, or bare property name.
fn accessor(input: &mut Input<'_>) -> PResult<KdlQueryMatcherAccessor> {
    alt((
        type_accessor,
        arg_accessor,
        prop_accessor,
        prop_name_accessor,
    ))
    .parse_next(input)
}

/// Parse `type()` accessor.
fn type_accessor(input: &mut Input<'_>) -> PResult<KdlQueryMatcherAccessor> {
    ("type", wss, "(", wss, ")")
        .map(|_| KdlQueryMatcherAccessor::Annotation)
        .parse_next(input)
}

/// Parse `arg()` or `arg(n)` accessor.
fn arg_accessor(input: &mut Input<'_>) -> PResult<KdlQueryMatcherAccessor> {
    preceded("arg", delimited("(", (wss, opt(integer), wss), ")"))
        .map(|(_, idx, _)| KdlQueryMatcherAccessor::Arg(idx))
        .parse_next(input)
}

/// Parse `prop(name)` accessor.
fn prop_accessor(input: &mut Input<'_>) -> PResult<KdlQueryMatcherAccessor> {
    preceded("prop", delimited("(", (wss, identifier, wss), ")"))
        .map(|(_, name, _)| KdlQueryMatcherAccessor::Prop(name))
        .parse_next(input)
}

/// Parse bare property name accessor.
fn prop_name_accessor(input: &mut Input<'_>) -> PResult<KdlQueryMatcherAccessor> {
    // Make sure it's not followed by `(` which would indicate a function call
    terminated(identifier, peek(not_paren))
        .map(KdlQueryMatcherAccessor::Prop)
        .parse_next(input)
}

/// Check that next char is not `(`
fn not_paren(input: &mut Input<'_>) -> PResult<()> {
    if input.starts_with("(") {
        Err(ErrMode::Backtrack(QueryParseError::default()))
    } else {
        Ok(())
    }
}

/// Parse a simple identifier (for property names, node names).
fn identifier(input: &mut Input<'_>) -> PResult<String> {
    alt((quoted_identifier, bare_identifier)).parse_next(input)
}

/// Parse a bare identifier.
fn bare_identifier(input: &mut Input<'_>) -> PResult<String> {
    take_while(1.., |c: char| {
        !c.is_whitespace()
            && !matches!(
                c,
                '(' | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | ','
                    | '='
                    | '>'
                    | '<'
                    | '!'
                    | '^'
                    | '$'
                    | '*'
                    | '"'
                    | '\''
            )
    })
    .map(|s: &str| s.to_string())
    .parse_next(input)
}

/// Parse a quoted identifier.
fn quoted_identifier(input: &mut Input<'_>) -> PResult<String> {
    delimited('"', take_while(0.., |c| c != '"'), '"')
        .map(|s: &str| s.to_string())
        .parse_next(input)
}

/// Parse an integer for arg(n).
fn integer(input: &mut Input<'_>) -> PResult<usize> {
    take_while(1.., |c: char| c.is_ascii_digit())
        .verify_map(|s: &str| s.parse::<usize>().ok())
        .parse_next(input)
}

/// Parse a KDL value (simplified for query context).
fn kdl_value(input: &mut Input<'_>) -> PResult<KdlValue> {
    alt((
        // Boolean keywords
        "#true".map(|_| KdlValue::Bool(true)),
        "#false".map(|_| KdlValue::Bool(false)),
        "true".map(|_| KdlValue::Bool(true)),
        "false".map(|_| KdlValue::Bool(false)),
        // Null
        "#null".map(|_| KdlValue::Null),
        "null".map(|_| KdlValue::Null),
        // Special floats
        "#inf".map(|_| KdlValue::Float(f64::INFINITY)),
        "#-inf".map(|_| KdlValue::Float(f64::NEG_INFINITY)),
        "#nan".map(|_| KdlValue::Float(f64::NAN)),
        // String
        quoted_string,
        // Number (float or integer)
        number,
    ))
    .parse_next(input)
}

/// Parse a quoted string value.
fn quoted_string(input: &mut Input<'_>) -> PResult<KdlValue> {
    delimited('"', take_while(0.., |c| c != '"'), '"')
        .map(|s: &str| KdlValue::String(s.to_string()))
        .parse_next(input)
}

/// Parse a number (integer or float).
fn number(input: &mut Input<'_>) -> PResult<KdlValue> {
    let sign = opt(one_of(['+', '-'])).parse_next(input)?;
    let digits: &str =
        take_while(1.., |c: char| c.is_ascii_digit() || c == '.').parse_next(input)?;

    let s = if let Some(sign) = sign {
        format!("{}{}", sign, digits)
    } else {
        digits.to_string()
    };

    if s.contains('.') {
        s.parse::<f64>()
            .map(KdlValue::Float)
            .map_err(|_| ErrMode::Backtrack(QueryParseError::default()))
    } else {
        s.parse::<i128>()
            .map(KdlValue::Integer)
            .map_err(|_| ErrMode::Backtrack(QueryParseError::default()))
    }
}

/// Whitespace (zero or more).
fn wss(input: &mut Input<'_>) -> PResult<()> {
    repeat(0.., ws).parse_next(input)
}

/// Single whitespace character.
fn ws(input: &mut Input<'_>) -> PResult<()> {
    alt((unicode_space, newline)).parse_next(input)
}

static UNICODE_SPACES: [char; 18] = [
    '\u{0009}', '\u{0020}', '\u{00A0}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}',
    '\u{2004}', '\u{2005}', '\u{2006}', '\u{2007}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{202F}',
    '\u{205F}', '\u{3000}',
];

fn unicode_space(input: &mut Input<'_>) -> PResult<()> {
    one_of(UNICODE_SPACES).void().parse_next(input)
}

static NEWLINES: [&str; 7] = [
    "\r\n", "\r", "\n", "\u{0085}", "\u{000C}", "\u{2028}", "\u{2029}",
];

fn newline(input: &mut Input<'_>) -> PResult<()> {
    alt(NEWLINES).void().parse_next(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_node() {
        let q = parse_query("foo").unwrap();
        assert_eq!(q.0.len(), 1);
    }

    #[test]
    fn parse_node_with_type() {
        let q = parse_query("(string)foo").unwrap();
        assert_eq!(q.0.len(), 1);
    }

    #[test]
    fn parse_node_with_attribute() {
        let q = parse_query("foo[bar]").unwrap();
        assert_eq!(q.0.len(), 1);
    }

    #[test]
    fn parse_node_with_attribute_value() {
        let q = parse_query(r#"foo[bar="baz"]"#).unwrap();
        assert_eq!(q.0.len(), 1);
    }

    #[test]
    fn parse_child_combinator() {
        let q = parse_query("foo > bar").unwrap();
        assert_eq!(q.0.len(), 1);
        assert_eq!(q.0[0].0.len(), 2);
    }

    #[test]
    fn parse_descendant_combinator() {
        let q = parse_query("foo >> bar").unwrap();
        assert_eq!(q.0.len(), 1);
    }

    #[test]
    fn parse_multiple_selectors() {
        let q = parse_query("foo, bar").unwrap();
        assert_eq!(q.0.len(), 2);
    }

    #[test]
    fn parse_scope() {
        let q = parse_query("scope()").unwrap();
        assert_eq!(q.0.len(), 1);
    }

    #[test]
    fn parse_arg_accessor() {
        let q = parse_query("[arg(0)]").unwrap();
        assert_eq!(q.0.len(), 1);
    }

    #[test]
    fn parse_prop_accessor() {
        let q = parse_query("[prop(name)]").unwrap();
        assert_eq!(q.0.len(), 1);
    }

    #[test]
    fn parse_type_accessor() {
        let q = parse_query(r#"[type()="string"]"#).unwrap();
        assert_eq!(q.0.len(), 1);
    }

    #[test]
    fn parse_numeric_comparison() {
        let q = parse_query("[arg(0) >= 10]").unwrap();
        assert_eq!(q.0.len(), 1);
    }
}
