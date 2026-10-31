use fuzzy_matcher::FuzzyMatcher;
use wntui_core::filter::{Field, FieldReference};

// Tab completion for the filter line. Pure functions over the current
// input: no state, no side effects, so they test table-driven.

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Completion {
    // Replace the current word with this text.
    Word(String),
    // Insert this text right where the word ends (operators: ":", "<", ">").
    Operator(&'static str),
}

fn current_word(input: &str) -> &str {
    let start = input
        .rfind(|character: char| character.is_whitespace() || character == '(' || character == ')')
        .map(|index| index + 1)
        .unwrap_or(0);
    &input[start..]
}

fn word_start(input: &str) -> usize {
    input
        .rfind(|character: char| character.is_whitespace() || character == '(' || character == ')')
        .map(|index| index + 1)
        .unwrap_or(0)
}

// What the cursor is on: a field name or a field value.
enum WordContext {
    Field,
    Value(Field),
}

fn word_context(input: &str) -> WordContext {
    let start = word_start(input);
    let before = &input[..start];
    let word = current_word(input);

    // The word being typed follows a field + operator, e.g. "status:on".
    // Operators may have a space after them ("status: on") — the tokenizer
    // treats both forms the same.
    let trimmed = before.trim_end();
    if let Some((_, operator_offset)) = find_last_operator(trimmed) {
        let field_word = find_field_word(&trimmed[..operator_offset]);
        if let Some(field) = field_word.and_then(Field::parse) {
            // Only a value context if no whitespace separates the operator
            // from the current word (or the word is empty and the operator
            // is the last thing typed).
            let after_operator = &trimmed[operator_offset..];
            let joined = after_operator.chars().all(|c| !c.is_whitespace());
            if joined || word.is_empty() {
                return WordContext::Value(field);
            }
        }
    }

    if let Some(offset) = word.find([':', '<', '>']) {
        let field_word = &word[..offset];
        return match Field::parse(field_word) {
            Some(field) => WordContext::Value(field),
            None => WordContext::Field,
        };
    }

    WordContext::Field
}

// Returns (operator, byte offset) of the last ':' / '<' / '>' before the
// current word. Only the segment after the last boolean keyword counts: in
// "status:online and role:", the operator belongs to role, not status.
fn find_last_operator(before: &str) -> Option<(&'static str, usize)> {
    let base = before
        .rfind(" and ")
        .or_else(|| before.rfind(" or "))
        .or_else(|| before.rfind(" not "))
        .map(|index| index + 1)
        .unwrap_or(0);

    let segment = &before[base..];
    let offset_in_segment = segment.rfind([':', '<', '>'])?;
    let offset = base + offset_in_segment;
    let operator = match &before[offset..offset + 1] {
        ":" => ":",
        "<" => "<",
        _ => ">",
    };
    Some((operator, offset))
}

// The word immediately before an operator, skipping trailing whitespace.
fn find_field_word(before_operator: &str) -> Option<&str> {
    let trimmed = before_operator.trim_end();
    let start = trimmed
        .rfind(|character: char| character.is_whitespace() || character == '(' || character == ')')
        .map(|index| index + 1)
        .unwrap_or(0);
    Some(&trimmed[start..])
}

fn field_completions(prefix: &str) -> Vec<String> {
    if prefix.is_empty() {
        return Vec::new();
    }

    let mut entries: Vec<(String, i64, bool)> = Vec::new();

    for entry in Field::reference() {
        for alias in entry.aliases {
            // A fully typed alias completes to itself: not a candidate.
            if *alias == prefix {
                continue;
            }
            if fuzzy_prefix_rank(alias, prefix) || alias.starts_with(prefix) {
                // Ranking: prefix hits first, then canonical names, then
                // shorter aliases, then fuzzy-only matches.
                entries.push((
                    (*alias).to_owned(),
                    if alias.starts_with(prefix) { 1_000 } else { 0 },
                    *alias == entry.name,
                ));
            }
        }
    }

    entries.sort_by(|left, right| {
        right
            .1
            .cmp(&left.1)
            .then(right.2.cmp(&left.2))
            .then(left.0.len().cmp(&right.0.len()))
    });
    entries.dedup_by(|seen, entry| seen.0 == entry.0);
    entries.into_iter().map(|(alias, _, _)| alias).collect()
}

fn fuzzy_prefix_rank(candidate: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        // Empty input is the entry point, not a completion context: fields
        // are listed in `:fields`, not spam-completed on a bare Tab.
        return false;
    }

    thread_local! {
        static MATCHER: fuzzy_matcher::skim::SkimMatcherV2 =
            fuzzy_matcher::skim::SkimMatcherV2::default().ignore_case();
    }

    MATCHER.with(|matcher| matcher.fuzzy_match(candidate, prefix).is_some())
}

fn value_completions(field: Field) -> Vec<String> {
    let entry = Field::reference()
        .iter()
        .find(|entry: &&FieldReference| entry.field == field);
    match entry {
        Some(entry) => entry
            .enum_values
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        None => Vec::new(),
    }
}

// The candidate list for the input as typed, in display order.
pub fn candidates(input: &str) -> Vec<Completion> {
    let word = current_word(input);

    match word_context(input) {
        WordContext::Field => {
            let prefix = word;
            let mut completions: Vec<Completion> = field_completions(prefix)
                .into_iter()
                .map(Completion::Word)
                .collect();

            // With a known field fully typed, Tab offers the operator. The
            // field may sit in the previous word when completion left a
            // trailing space ("voltage " → operators).
            let field_word = if prefix.is_empty() {
                previous_word(input)
            } else {
                prefix
            };

            if let Some(field) = Field::parse(field_word) {
                let entry = Field::reference().iter().find(|entry| entry.field == field);
                if let Some(entry) = entry {
                    for operator in entry.comparators.split_whitespace() {
                        completions.push(Completion::Operator(match operator {
                            ":" => ":",
                            "<" => "<",
                            _ => ">",
                        }));
                    }
                }
            }

            completions
        }
        WordContext::Value(field) => {
            // Prefix hits rank first; the full value list stays visible
            // like Wireshark's completion popup.
            let value_prefix = value_prefix(input);
            let mut values = value_completions(field);
            values.sort_by(|left, right| {
                let left_hit = left.starts_with(value_prefix) && left != value_prefix;
                let right_hit = right.starts_with(value_prefix) && right != value_prefix;
                right_hit.cmp(&left_hit)
            });
            values.into_iter().map(Completion::Word).collect()
        }
    }
}

// The word before the current one, or None when there is none.
fn previous_word(input: &str) -> &str {
    let start = word_start(input);
    let before = input[..start].trim_end();
    if before.is_empty() {
        return "";
    }

    let word_begin = before
        .rfind(|character: char| character.is_whitespace() || character == '(' || character == ')')
        .map(|index| index + 1)
        .unwrap_or(0);
    &before[word_begin..]
}

// The already-typed value text for the word under the cursor.
fn value_prefix(input: &str) -> &str {
    let word = current_word(input);
    match word.find([':', '<', '>']) {
        Some(offset) => &word[offset + 1..],
        None => word,
    }
}

// Applies the first candidate to the input. Returns None when there is
// nothing to complete so the caller can leave the input untouched.
pub fn complete(input: &str) -> Option<(String, Option<String>)> {
    let list = candidates(input);
    let first = list.first()?;

    match first {
        Completion::Word(replacement) => {
            let start = replacement_start(input);
            let applied = format!("{}{} ", &input[..start], replacement);
            let suggestion = list
                .iter()
                .filter_map(|candidate| match candidate {
                    Completion::Word(word) => Some(word.clone()),
                    Completion::Operator(_) => None,
                })
                .collect::<Vec<_>>()
                .join(" ");
            Some((applied, Some(suggestion)))
        }
        Completion::Operator(operator) => {
            // Operators attach directly to the field: eat any trailing
            // whitespace a previous completion left behind.
            let trimmed = input.trim_end();
            Some((format!("{trimmed}{operator}"), None))
        }
    }
}

// Where a word replacement begins. Value completions replace only the
// value part of "field<operator>value"; field completions replace the
// current word from its start.
fn replacement_start(input: &str) -> usize {
    let start = word_start(input);
    let word = current_word(input);

    // Value inside the current word: "role:si" → replace "si", keep "role:".
    if let Some(offset) = word.find([':', '<', '>']) {
        let field_word = &word[..offset];
        if Field::parse(field_word).is_some() {
            return start + offset + 1;
        }
    }

    // Value in a separate word: "status: on" → replace from after the
    // operator so the applied text has no gap.
    let before = input[..start].trim_end();
    if let Some((_, operator_offset)) = find_last_operator(before) {
        let field_word = find_field_word(&before[..operator_offset]);
        if field_word.and_then(Field::parse).is_some() {
            return before.len();
        }
    }

    start
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(input: &str) -> Vec<String> {
        candidates(input)
            .into_iter()
            .filter_map(|candidate| match candidate {
                Completion::Word(word) => Some(word),
                Completion::Operator(_) => None,
            })
            .collect()
    }

    fn operators(input: &str) -> Vec<&'static str> {
        candidates(input)
            .into_iter()
            .filter_map(|candidate| match candidate {
                Completion::Operator(operator) => Some(operator),
                Completion::Word(_) => None,
            })
            .collect()
    }

    #[test]
    fn completes_field_prefixes_fuzzy_ranked() {
        let list = words("vol");
        assert_eq!(list.first().map(String::as_str), Some("voltage"));
        assert!(list.iter().any(|word| word == "volt"));

        // Canonical aliases rank before their long forms.
        let list = words("st");
        assert_eq!(list.first().map(String::as_str), Some("status"));
    }

    #[test]
    fn full_field_offers_its_operators() {
        assert_eq!(operators("voltage"), vec![":", "<", ">"]);
        assert_eq!(operators("status"), vec![":"]);
    }

    #[test]
    fn value_position_offers_enum_values() {
        assert_eq!(words("status:on"), vec!["online", "offline", "uncertain"]);
        assert_eq!(words("role:"), vec!["sink", "head", "sub", "unknown"]);
    }

    #[test]
    fn applying_a_completion_replaces_the_current_word() {
        let (applied, _) = complete("vol").unwrap();
        assert_eq!(applied, "voltage ");

        let (applied, _) = complete("status: on").unwrap();
        assert_eq!(applied, "status:online ");

        let (applied, _) = complete("(role:si").unwrap();
        assert_eq!(applied, "(role:sink ");
    }

    #[test]
    fn operator_completion_appends_without_a_space() {
        let (applied, _) = complete("voltage").unwrap();
        assert_eq!(applied, "voltage:");
    }

    #[test]
    fn numbers_and_free_text_offer_nothing() {
        assert!(complete("addr:0x2").is_none());
        assert!(complete("warehouse").is_none());
        assert!(complete("").is_none());
    }

    #[test]
    fn multi_word_expressions_complete_the_last_word() {
        let (applied, _) = complete("status:online and role:he").unwrap();
        assert_eq!(applied, "status:online and role:head ");
    }
}
