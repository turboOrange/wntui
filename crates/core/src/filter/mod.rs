use fuzzy_matcher::FuzzyMatcher;

use crate::model::{Node, OnlineStatus, Role};

#[derive(Clone, PartialEq, Debug)]
pub enum Filter {
    All(Vec<Filter>),
    Any(Vec<Filter>),
    Not(Box<Filter>),
    Fuzzy(String),
    Condition {
        field: Field,
        operator: Operator,
        value: Value,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Status,
    Role,
    Voltage,
    Firmware,
    Name,
    Address,
    Network,
    Approved,
    BootCount,
    Age,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Operator {
    Matches,
    Less,
    Greater,
}

#[derive(Clone, PartialEq, Debug)]
pub enum Value {
    Text(String),
    Number(f64),
    Version([u32; 4]),
    Duration(u64),
}

#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
pub enum ParseError {
    #[error("unexpected '{0}'")]
    UnexpectedToken(String),
    #[error("unclosed '('")]
    UnclosedParen,
    #[error("missing ')'")]
    MissingParen,
    #[error("unknown field '{0}' (a bare word is fuzzy matched)")]
    UnknownField(String),
    #[error("{field} does not support {operator}")]
    UnsupportedOperator {
        field: &'static str,
        operator: &'static str,
    },
    #[error("bad value for {field}: '{value}'")]
    BadValue { field: &'static str, value: String },
}

impl Field {
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "status" | "state" => Some(Self::Status),
            "role" => Some(Self::Role),
            "voltage" | "volt" | "v" => Some(Self::Voltage),
            "fw" | "firmware" => Some(Self::Firmware),
            "name" => Some(Self::Name),
            "addr" | "address" | "node" => Some(Self::Address),
            "net" | "network" => Some(Self::Network),
            "approved" => Some(Self::Approved),
            "boots" | "boot_count" => Some(Self::BootCount),
            "age" => Some(Self::Age),
            _ => None,
        }
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Status => "status",
            Self::Role => "role",
            Self::Voltage => "voltage",
            Self::Firmware => "fw",
            Self::Name => "name",
            Self::Address => "addr",
            Self::Network => "net",
            Self::Approved => "approved",
            Self::BootCount => "boots",
            Self::Age => "age",
        }
    }

    fn supports(&self, operator: Operator) -> bool {
        match self {
            Self::Status | Self::Role | Self::Name | Self::Approved => {
                operator == Operator::Matches
            }
            Self::Network | Self::Address | Self::Voltage | Self::BootCount => true,
            Self::Firmware => true,
            Self::Age => true,
        }
    }

    fn operator_label(operator: Operator) -> &'static str {
        match operator {
            Operator::Matches => ":",
            Operator::Less => "<",
            Operator::Greater => ">",
        }
    }
}

// The one field table that drives the parser docs (`:help fields`), Tab
// completion, and the reference overlay. `aliases` includes the canonical
// name; enum values are the accepted words for `:` on that field.
pub struct FieldReference {
    pub field: Field,
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub kind: &'static str,
    pub comparators: &'static str,
    pub example: &'static str,
    pub enum_values: &'static [&'static str],
}

impl Field {
    pub fn reference() -> &'static [FieldReference] {
        const {
            &[
                FieldReference {
                    field: Field::Status,
                    name: "status",
                    aliases: &["status", "state"],
                    kind: "enum",
                    comparators: ":",
                    example: "status:online",
                    enum_values: &["online", "offline", "uncertain"],
                },
                FieldReference {
                    field: Field::Role,
                    name: "role",
                    aliases: &["role"],
                    kind: "enum",
                    comparators: ":",
                    example: "role:sub",
                    enum_values: &["sink", "head", "sub", "unknown"],
                },
                FieldReference {
                    field: Field::Voltage,
                    name: "voltage",
                    aliases: &["voltage", "volt", "v"],
                    kind: "number",
                    comparators: ": < >",
                    example: "voltage:<3.2",
                    enum_values: &[],
                },
                FieldReference {
                    field: Field::Firmware,
                    name: "fw",
                    aliases: &["fw", "firmware"],
                    kind: "version",
                    comparators: ": < >",
                    example: "fw:>5.1.0.0",
                    enum_values: &[],
                },
                FieldReference {
                    field: Field::Name,
                    name: "name",
                    aliases: &["name"],
                    kind: "text",
                    comparators: ":",
                    example: "name:pump",
                    enum_values: &[],
                },
                FieldReference {
                    field: Field::Address,
                    name: "addr",
                    aliases: &["addr", "address", "node"],
                    kind: "number",
                    comparators: ": < >",
                    example: "addr:>0x0002 and addr:<0x0008",
                    enum_values: &[],
                },
                FieldReference {
                    field: Field::Network,
                    name: "net",
                    aliases: &["net", "network"],
                    kind: "number",
                    comparators: ": < >",
                    example: "net:42",
                    enum_values: &[],
                },
                FieldReference {
                    field: Field::Approved,
                    name: "approved",
                    aliases: &["approved"],
                    kind: "bool",
                    comparators: ":",
                    example: "approved:true",
                    enum_values: &["true", "false"],
                },
                FieldReference {
                    field: Field::BootCount,
                    name: "boots",
                    aliases: &["boots", "boot_count"],
                    kind: "number",
                    comparators: ": < >",
                    example: "boots:>3",
                    enum_values: &[],
                },
                FieldReference {
                    field: Field::Age,
                    name: "age",
                    aliases: &["age"],
                    kind: "duration",
                    comparators: ": < >",
                    example: "age<1h",
                    enum_values: &[],
                },
            ]
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Token {
    Word(String),
    LParen,
    RParen,
    Colon,
    Less,
    Greater,
    Bang,
}

#[derive(Clone, PartialEq, Eq, Debug)]
struct SpannedToken {
    token: Token,
    start: usize,
    end: usize,
}

fn tokenize(input: &str) -> Vec<SpannedToken> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut word_start = 0;

    let flush =
        |word: &mut String, word_start: &mut usize, end: usize, tokens: &mut Vec<SpannedToken>| {
            if !word.is_empty() {
                tokens.push(SpannedToken {
                    token: Token::Word(std::mem::take(word)),
                    start: *word_start,
                    end,
                });
                word.clear();
                *word_start = end;
            }
        };

    for (offset, character) in input.char_indices() {
        let end = offset + character.len_utf8();
        match character {
            whitespace if whitespace.is_whitespace() => {
                flush(&mut word, &mut word_start, offset, &mut tokens)
            }
            '(' => {
                flush(&mut word, &mut word_start, offset, &mut tokens);
                tokens.push(SpannedToken {
                    token: Token::LParen,
                    start: offset,
                    end,
                });
            }
            ')' => {
                flush(&mut word, &mut word_start, offset, &mut tokens);
                tokens.push(SpannedToken {
                    token: Token::RParen,
                    start: offset,
                    end,
                });
            }
            ':' => {
                flush(&mut word, &mut word_start, offset, &mut tokens);
                tokens.push(SpannedToken {
                    token: Token::Colon,
                    start: offset,
                    end,
                });
            }
            '<' => {
                flush(&mut word, &mut word_start, offset, &mut tokens);
                tokens.push(SpannedToken {
                    token: Token::Less,
                    start: offset,
                    end,
                });
            }
            '>' => {
                flush(&mut word, &mut word_start, offset, &mut tokens);
                tokens.push(SpannedToken {
                    token: Token::Greater,
                    start: offset,
                    end,
                });
            }
            '!' => {
                flush(&mut word, &mut word_start, offset, &mut tokens);
                tokens.push(SpannedToken {
                    token: Token::Bang,
                    start: offset,
                    end,
                });
            }
            other => {
                if word.is_empty() {
                    word_start = offset;
                }
                word.push(other);
            }
        }
    }

    flush(
        &mut word,
        &mut word_start,
        input.chars().count(),
        &mut tokens,
    );
    tokens
}

struct Parser {
    tokens: Vec<SpannedToken>,
    position: usize,
}

// A parse error plus the character span it points at, so the TUI can draw a
// caret under the offending part of the expression. `position` is the first
// byte of the erroring token; `end` the byte after it.
#[derive(Clone, PartialEq, Eq, Debug, thiserror::Error)]
#[error("{}", .error)]
pub struct LocatedParseError {
    #[source]
    pub error: ParseError,
    pub position: usize,
    pub end: usize,
}

pub fn parse(input: &str) -> Result<Filter, ParseError> {
    parse_detailed(input).map_err(|located| located.error)
}

pub fn parse_detailed(input: &str) -> Result<Filter, LocatedParseError> {
    let mut parser = Parser {
        tokens: tokenize(input),
        position: 0,
    };

    if parser.tokens.is_empty() {
        return Ok(Filter::All(Vec::new()));
    }

    let filter = parser.parse_or()?;

    match parser.tokens.get(parser.position) {
        None => Ok(filter),
        Some(spanned) => {
            let error = match &spanned.token {
                Token::RParen => ParseError::MissingParen,
                token => ParseError::UnexpectedToken(render_token(token)),
            };
            Err(LocatedParseError {
                error,
                position: spanned.start,
                end: spanned.end,
            })
        }
    }
}

// A missing ')' is reported at the end of input, where the parser ran out of
// tokens; an unclosed '(' at its own position.
impl Parser {
    fn peek(&self) -> Option<&Token> {
        Some(&self.tokens.get(self.position)?.token)
    }

    fn peek_span(&self) -> Option<&SpannedToken> {
        self.tokens.get(self.position)
    }

    fn next(&mut self) -> Option<SpannedToken> {
        let token = self.tokens.get(self.position).cloned();
        if token.is_some() {
            self.position += 1;
        }
        token
    }

    fn located(&self, error: ParseError) -> LocatedParseError {
        let (position, end) = match self.peek_span() {
            Some(spanned) => (spanned.start, spanned.end),
            // At end of input the caret points just past the last token.
            None => match self.tokens.last() {
                Some(last) => (last.end, last.end),
                None => (0, 0),
            },
        };

        LocatedParseError {
            error,
            position,
            end,
        }
    }

    fn keyword(&self) -> Option<&str> {
        match self.peek() {
            Some(Token::Word(word)) if word == "and" || word == "or" || word == "not" => {
                Some(word.as_str())
            }
            _ => None,
        }
    }

    fn starts_primary(&self) -> bool {
        match self.peek() {
            Some(Token::Word(word)) => word != "and" && word != "or",
            Some(Token::LParen) | Some(Token::Bang) => true,
            _ => false,
        }
    }

    fn parse_or(&mut self) -> Result<Filter, LocatedParseError> {
        let mut branches = vec![self.parse_and()?];

        while self.keyword() == Some("or") {
            self.next();
            branches.push(self.parse_and()?);
        }

        Ok(single_or_fold(Filter::Any, branches))
    }

    fn parse_and(&mut self) -> Result<Filter, LocatedParseError> {
        let mut terms = vec![self.parse_unary()?];

        loop {
            if self.keyword() == Some("and") {
                self.next();
                terms.push(self.parse_unary()?);
            } else if self.starts_primary() {
                terms.push(self.parse_unary()?);
            } else {
                break;
            }
        }

        Ok(single_or_fold(Filter::All, terms))
    }

    fn parse_unary(&mut self) -> Result<Filter, LocatedParseError> {
        if self.keyword() == Some("not") {
            self.next();
            return Ok(Filter::Not(Box::new(self.parse_unary()?)));
        }

        if self.peek() == Some(&Token::LParen) {
            self.next();
            let inner = self.parse_or()?;
            if self.next().map(|spanned| spanned.token) != Some(Token::RParen) {
                // The parser is past the '(' when the ')' is missing, so the
                // caret lands at the end of the expression.
                let mut located = self.located(ParseError::UnclosedParen);
                if let Some(last) = self.tokens.last() {
                    located.position = last.end;
                    located.end = last.end;
                }
                return Err(located);
            }
            return Ok(inner);
        }

        self.parse_term()
    }

    fn parse_term(&mut self) -> Result<Filter, LocatedParseError> {
        let negated = self.peek() == Some(&Token::Bang);
        if negated {
            self.next();
        }

        let Some(field_word) = self.next() else {
            return Err(self.located(ParseError::UnexpectedToken("end of filter".to_owned())));
        };
        let Token::Word(word) = field_word.token else {
            return Err(self.located(ParseError::UnexpectedToken(render_token(&field_word.token))));
        };

        let operator = match self.peek() {
            Some(Token::Colon) => {
                self.next();
                match self.peek() {
                    Some(Token::Less) => {
                        self.next();
                        Some(Operator::Less)
                    }
                    Some(Token::Greater) => {
                        self.next();
                        Some(Operator::Greater)
                    }
                    _ => Some(Operator::Matches),
                }
            }
            Some(Token::Less) => {
                self.next();
                Some(Operator::Less)
            }
            Some(Token::Greater) => {
                self.next();
                Some(Operator::Greater)
            }
            _ => None,
        };

        let condition = match operator {
            None => Filter::Fuzzy(word),
            Some(operator) => {
                let Some(value_word) = self.next() else {
                    return Err(
                        self.located(ParseError::UnexpectedToken("missing value".to_owned()))
                    );
                };
                let Token::Word(raw) = value_word.token else {
                    return Err(
                        self.located(ParseError::UnexpectedToken(render_token(&value_word.token)))
                    );
                };

                let field = match Field::parse(&word) {
                    Some(field) => field,
                    None => {
                        return Err(LocatedParseError {
                            error: ParseError::UnknownField(word.clone()),
                            position: field_word.start,
                            end: field_word.end,
                        });
                    }
                };

                if !field.supports(operator) {
                    return Err(LocatedParseError {
                        error: ParseError::UnsupportedOperator {
                            field: field.label(),
                            operator: Field::operator_label(operator),
                        },
                        position: field_word.start,
                        end: value_word.end,
                    });
                }

                let value = match typed_value(field, &raw) {
                    Ok(value) => value,
                    Err(error) => {
                        return Err(LocatedParseError {
                            error,
                            position: value_word.start,
                            end: value_word.end,
                        });
                    }
                };

                Filter::Condition {
                    field,
                    operator,
                    value,
                }
            }
        };

        Ok(if negated {
            Filter::Not(Box::new(condition))
        } else {
            condition
        })
    }
}

fn single_or_fold(wrap: fn(Vec<Filter>) -> Filter, mut parts: Vec<Filter>) -> Filter {
    if parts.len() == 1 {
        parts.pop().expect("one part")
    } else {
        wrap(parts)
    }
}

fn typed_value(field: Field, raw: &str) -> Result<Value, ParseError> {
    let label = field.label();

    let bad_value = || ParseError::BadValue {
        field: label,
        value: raw.to_owned(),
    };

    match field {
        Field::Status => match raw {
            "online" | "on" => Ok(Value::Text("online".to_owned())),
            "offline" | "off" => Ok(Value::Text("offline".to_owned())),
            "uncertain" | "unc" | "maybe" => Ok(Value::Text("uncertain".to_owned())),
            _ => Err(bad_value()),
        },
        Field::Role => match raw {
            "sink" => Ok(Value::Text("sink".to_owned())),
            "head" | "headnode" => Ok(Value::Text("head".to_owned())),
            "sub" | "subnode" => Ok(Value::Text("sub".to_owned())),
            "unknown" | "?" => Ok(Value::Text("unknown".to_owned())),
            _ => Err(bad_value()),
        },
        Field::Approved => match raw {
            "true" | "yes" | "1" => Ok(Value::Text("true".to_owned())),
            "false" | "no" | "0" => Ok(Value::Text("false".to_owned())),
            _ => Err(bad_value()),
        },
        Field::Name => Ok(Value::Text(raw.to_owned())),
        Field::Voltage | Field::BootCount => {
            parse_number(raw).map(Value::Number).ok_or_else(bad_value)
        }
        Field::Network | Field::Address => {
            parse_number(raw).map(Value::Number).ok_or_else(bad_value)
        }
        Field::Firmware => parse_version(raw).ok_or_else(bad_value),
        Field::Age => parse_duration(raw).ok_or_else(bad_value),
    }
}

fn parse_number(raw: &str) -> Option<f64> {
    if let Some(hex) = raw.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16).ok().map(f64::from);
    }
    raw.parse::<f64>().ok()
}

fn parse_version(raw: &str) -> Option<Value> {
    let parts: Vec<u32> = raw
        .split('.')
        .map(|part| part.parse().ok())
        .collect::<Option<Vec<_>>>()?;

    let mut version = [0_u32; 4];
    version
        .iter_mut()
        .zip(parts)
        .for_each(|(slot, part)| *slot = part);
    Some(Value::Version(version))
}

fn parse_duration(raw: &str) -> Option<Value> {
    let split_at = raw.find(|character: char| character.is_ascii_alphabetic())?;
    let (digits, unit) = raw.split_at(split_at);
    let amount: u64 = digits.parse().ok()?;

    let seconds = match unit {
        "s" => amount,
        "m" => amount.checked_mul(60)?,
        "h" => amount.checked_mul(3600)?,
        "d" => amount.checked_mul(86400)?,
        "w" => amount.checked_mul(604_800)?,
        _ => return None,
    };
    Some(Value::Duration(seconds))
}

fn render_token(token: &Token) -> String {
    match token {
        Token::Word(word) => word.clone(),
        Token::LParen => "(".to_owned(),
        Token::RParen => ")".to_owned(),
        Token::Colon => ":".to_owned(),
        Token::Less => "<".to_owned(),
        Token::Greater => ">".to_owned(),
        Token::Bang => "!".to_owned(),
    }
}

impl Filter {
    pub fn matches(&self, node: &Node, now_ms: u64) -> bool {
        match self {
            Self::All(terms) => terms.iter().all(|term| term.matches(node, now_ms)),
            Self::Any(branches) => branches.iter().any(|branch| branch.matches(node, now_ms)),
            Self::Not(inner) => !inner.matches(node, now_ms),
            Self::Fuzzy(query) => fuzzy_matches(query, node),
            Self::Condition {
                field,
                operator,
                value,
            } => condition_matches(*field, *operator, value, node, now_ms),
        }
    }

    // True when the result can change as wall-clock time passes without
    // any store mutation (age predicates compare against `now`). Callers
    // that cache filter results need this to know when to invalidate.
    pub fn uses_time(&self) -> bool {
        match self {
            Self::All(terms) => terms.iter().any(Filter::uses_time),
            Self::Any(branches) => branches.iter().any(Filter::uses_time),
            Self::Not(inner) => inner.uses_time(),
            Self::Fuzzy(_) => false,
            Self::Condition { field, .. } => matches!(field, Field::Age),
        }
    }
}

fn condition_matches(
    field: Field,
    operator: Operator,
    value: &Value,
    node: &Node,
    now_ms: u64,
) -> bool {
    match field {
        Field::Status => {
            let expected = match value {
                Value::Text(text) => text.as_str(),
                _ => return false,
            };
            status_text(node.online) == expected
        }
        Field::Role => {
            let expected = match value {
                Value::Text(text) => text.as_str(),
                _ => return false,
            };
            role_text(node.role) == expected
        }
        Field::Approved => {
            let expected = matches!(value, Value::Text(text) if text == "true");
            node.approved.unwrap_or(false) == expected
        }
        Field::Name => {
            let Value::Text(pattern) = value else {
                return false;
            };
            node.name
                .as_deref()
                .is_some_and(|name| name.to_lowercase().contains(&pattern.to_lowercase()))
        }
        Field::Voltage => compare_optional(node.voltage.map(f64::from), operator, value),
        Field::BootCount => compare_optional(node.boot_count.map(f64::from), operator, value),
        Field::Network => matches_number(f64::from(node.key.network_id), operator, value),
        Field::Address => matches_number(f64::from(node.key.address), operator, value),
        Field::Firmware => {
            let Value::Version(wanted) = value else {
                return false;
            };
            match node.firmware {
                None => false,
                Some(version) => {
                    let current = [
                        version.major,
                        version.minor,
                        version.maintenance,
                        version.devel,
                    ];
                    match operator {
                        Operator::Matches => current == *wanted,
                        Operator::Less => current < *wanted,
                        Operator::Greater => current > *wanted,
                    }
                }
            }
        }
        Field::Age => {
            let Value::Duration(seconds) = value else {
                return false;
            };
            node.last_seen_ms
                .map(|seen| now_ms.saturating_sub(seen) / 1000)
                .is_some_and(|age| match operator {
                    Operator::Less => age < *seconds,
                    Operator::Greater => age > *seconds,
                    Operator::Matches => age == *seconds,
                })
        }
    }
}

fn status_text(status: OnlineStatus) -> &'static str {
    match status {
        OnlineStatus::Online => "online",
        OnlineStatus::Uncertain => "uncertain",
        OnlineStatus::Offline => "offline",
    }
}

fn role_text(role: Role) -> &'static str {
    match role {
        Role::Sink => "sink",
        Role::Headnode => "head",
        Role::Subnode => "sub",
        Role::Unknown => "unknown",
    }
}

fn compare_optional(actual: Option<f64>, operator: Operator, value: &Value) -> bool {
    actual.is_some_and(|number| matches_number(number, operator, value))
}

fn matches_number(actual: f64, operator: Operator, value: &Value) -> bool {
    let Value::Number(wanted) = value else {
        return false;
    };
    match operator {
        Operator::Matches => (actual - wanted).abs() < f64::EPSILON,
        Operator::Less => actual < *wanted,
        Operator::Greater => actual > *wanted,
    }
}

fn fuzzy_matches(query: &str, node: &Node) -> bool {
    if query.is_empty() {
        return true;
    }

    let name = node.name.as_deref().unwrap_or_default();
    let description = node.description.as_deref().unwrap_or_default();
    let address = format!("0x{:04x}", node.key.address);

    thread_local! {
        static MATCHER: fuzzy_matcher::skim::SkimMatcherV2 =
            fuzzy_matcher::skim::SkimMatcherV2::default().ignore_case();
    }

    MATCHER.with(|matcher| {
        matcher.fuzzy_match(name, query).is_some()
            || (!description.is_empty() && matcher.fuzzy_match(description, query).is_some())
    }) || address.contains(query)
        || node.key.address.to_string().contains(query)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FirmwareVersion, NodeKey};

    const NOW_MS: u64 = 1_700_000_300_000;

    fn node() -> Node {
        Node {
            key: NodeKey::new(42, 0x1a20),
            online: OnlineStatus::Online,
            name: Some("Line 3 sensor".to_owned()),
            description: Some("near the conveyor".to_owned()),
            approved: Some(true),
            role: Role::Subnode,
            voltage: Some(3.12),
            firmware: Some(FirmwareVersion {
                major: 5,
                minor: 2,
                maintenance: 1,
                devel: 0,
            }),
            boot_count: Some(4),
            last_seen_ms: Some(1_700_000_000_000),
            ..Node::default()
        }
    }

    fn matches(input: &str) -> bool {
        parse(input).expect("parses").matches(&node(), NOW_MS)
    }

    #[test]
    fn status_role_and_negation() {
        assert!(matches("status:online"));
        assert!(matches("status:on"));
        assert!(matches("!status:offline"));
        assert!(matches("role:sub"));
        assert!(matches("role:subnode"));
        assert!(!matches("role:sink"));
        assert!(matches("not role:sink"));
    }

    #[test]
    fn numeric_comparisons() {
        assert!(matches("voltage:<3.2"));
        assert!(matches("voltage:>3"));
        assert!(matches("v:<3.13"));
        assert!(!matches("voltage:<3"));
        assert!(matches("boots:4"));
        assert!(matches("boots>3"));
    }

    #[test]
    fn firmware_and_address_and_network() {
        assert!(matches("fw:5.2.1.0"));
        assert!(matches("fw:>5.1.9.9"));
        assert!(!matches("fw:>5.2.1.0"));
        assert!(matches("addr:0x1a20"));
        assert!(matches("addr:6688"));
        assert!(matches("net:42"));
    }

    #[test]
    fn address_ranges_compose_with_comparisons() {
        // The fixture node is 0x1a20 = 6688; comparisons work on both
        // notations and compose into ranges.
        assert!(!matches("addr:>0x2 and addr:<0x9"), "6688 is out of range");
        assert!(matches("addr:>1 and addr:<7000"));
        assert!(!matches("addr:>6688"));
        assert!(matches("addr:>6687 and addr:<6689"));
        assert!(!matches("addr:>7000"));
        assert!(matches("net:>10 and net:<100"));
    }

    #[test]
    fn age_uses_the_injected_clock() {
        assert!(matches("age>4m"));
        assert!(matches("age:5m"));
        assert!(!matches("age<5m"));
        assert!(matches("age<1h"));
    }

    #[test]
    fn boolean_combinations_with_parens() {
        assert!(matches("status:offline or (role:sub and voltage:>3)"));
        assert!(!matches("status:offline and role:sub"));
        assert!(matches(
            "(status:offline or status:online) and !approved:false"
        ));
    }

    #[test]
    fn free_text_is_fuzzy() {
        assert!(matches("line3"));
        assert!(matches("sensor"));
        assert!(matches("conveyor"));
        assert!(matches("LINE"));
        assert!(!matches("warehouse"));
        assert!(matches("0x1a20"));
    }

    #[test]
    fn approved_parses_booleans() {
        assert!(matches("approved:true"));
        assert!(!matches("approved:false"));
        assert!(matches("!approved:false"));
    }

    #[test]
    fn parse_errors_are_specific() {
        assert!(
            matches!(parse("voltge:<3"), Err(ParseError::UnknownField(field)) if field == "voltge")
        );
        assert!(
            matches!(parse("status:<online"), Err(ParseError::UnsupportedOperator { field, .. }) if field == "status")
        );
        assert!(
            matches!(parse("age:soon"), Err(ParseError::BadValue { field, value }) if field == "age" && value == "soon")
        );
        assert!(matches!(
            parse("voltage:<"),
            Err(ParseError::UnexpectedToken(_))
        ));
        assert!(matches!(
            parse("(status:online"),
            Err(ParseError::UnclosedParen)
        ));
        assert!(matches!(
            parse("status:online)"),
            Err(ParseError::MissingParen)
        ));
        assert!(matches!(
            parse("role:banana"),
            Err(ParseError::BadValue { .. })
        ));
    }

    #[test]
    fn empty_filter_matches_everything() {
        assert!(matches(""));
    }

    #[test]
    fn parse_errors_carry_their_token_position() {
        let located = parse_detailed("status:online voltge:<3").unwrap_err();
        assert!(matches!(located.error, ParseError::UnknownField(word) if word == "voltge"));
        assert_eq!(located.position, 14, "the caret points at 'voltge'");
        assert_eq!(located.end, 20);

        let located = parse_detailed("role:banana").unwrap_err();
        assert!(matches!(located.error, ParseError::BadValue { .. }));
        assert_eq!(located.position, 5, "the caret points at the value");
        assert_eq!(located.end, 11);

        let located = parse_detailed("status:<online").unwrap_err();
        assert!(matches!(
            located.error,
            ParseError::UnsupportedOperator { .. }
        ));
        assert_eq!(located.position, 0, "the whole condition is highlighted");

        let located = parse_detailed("voltage:<").unwrap_err();
        assert!(
            matches!(located.error, ParseError::UnexpectedToken(reason) if reason == "missing value")
        );
        assert_eq!(located.position, 9, "at end of input, past the '<'");
        assert_eq!(located.end, 9);
    }

    #[test]
    fn paren_errors_point_at_the_end_of_input() {
        let located = parse_detailed("(status:online").unwrap_err();
        assert!(matches!(located.error, ParseError::UnclosedParen));
        assert_eq!(located.position, 14, "caret at end of input");
        assert_eq!(located.end, 14);

        let located = parse_detailed("status:online)").unwrap_err();
        assert!(matches!(located.error, ParseError::MissingParen));
        assert_eq!(located.position, 13, "caret at the stray ')'");
        assert_eq!(located.end, 14);
    }

    #[test]
    fn the_reference_table_covers_every_field_and_stays_in_sync() {
        let reference = Field::reference();

        // Every field variant appears exactly once, and the canonical name
        // is one of its own accepted aliases.
        let mut fields: Vec<Field> = reference.iter().map(|entry| entry.field).collect();
        fields.sort_by_key(|field| format!("{field:?}"));
        let expected = [
            Field::Address,
            Field::Age,
            Field::Approved,
            Field::BootCount,
            Field::Firmware,
            Field::Name,
            Field::Network,
            Field::Role,
            Field::Status,
            Field::Voltage,
        ];
        assert_eq!(fields, expected);

        for entry in reference {
            assert!(
                entry.aliases.contains(&entry.name),
                "canonical name {} must be an alias of itself",
                entry.name
            );
            assert!(!entry.example.is_empty());
            // Every example parses: this is the drift guard between the
            // reference table and the grammar.
            parse(entry.example).unwrap_or_else(|error| {
                panic!(
                    "reference example '{}' does not parse: {error}",
                    entry.example
                )
            });

            for alias in entry.aliases {
                assert_eq!(Field::parse(alias), Some(entry.field), "alias {alias}");
            }
        }
    }
}
