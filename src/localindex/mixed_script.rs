use std::sync::{Arc, LazyLock};

use tantivy::query::{AutomatonWeight, EnableScoring, Query, Weight};
use tantivy::schema::Field;
use tantivy_fst::{Automaton, Regex};

const CJK_PATTERN: &str = "[\u{3040}-\u{30ff}\u{3400}-\u{4dbf}\u{4e00}-\u{9fff}]";
static CJK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(CJK_PATTERN).expect("valid CJK character class"));
static SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("{CJK_PATTERN}.*")).expect("valid mixed-script suffix"));

#[derive(Clone, Debug)]
pub(super) struct MixedScriptQuery {
    field: Field,
    matcher: Arc<Matcher>,
}

impl MixedScriptQuery {
    pub(super) fn new(word: &str, field: Field) -> Self {
        debug_assert!(!word.is_empty() && !super::has_cjk(word));
        Self {
            field,
            matcher: Arc::new(Matcher { word: word.into() }),
        }
    }

    pub(super) fn for_field(&self, field: Field) -> Self {
        Self {
            field,
            matcher: Arc::clone(&self.matcher),
        }
    }
}

impl Query for MixedScriptQuery {
    fn weight(&self, _scoring: EnableScoring<'_>) -> tantivy::Result<Box<dyn Weight>> {
        Ok(Box::new(AutomatonWeight::<Matcher>::new(
            self.field,
            Arc::clone(&self.matcher),
        )))
    }
}

#[derive(Debug)]
struct Matcher {
    word: Box<str>,
}

#[derive(Clone)]
struct State {
    prefix: Option<usize>,
    has_prefix: bool,
    literal: Option<(usize, bool)>,
    suffix: Option<usize>,
}

// Match CJK+ word (CJK .*)? or word CJK .* without compiling a DFA per word.
impl Automaton for Matcher {
    type State = State;

    fn start(&self) -> State {
        State {
            prefix: CJK.start(),
            has_prefix: false,
            literal: None,
            suffix: None,
        }
    }

    fn is_match(&self, state: &State) -> bool {
        state
            .literal
            .is_some_and(|(position, prefixed)| prefixed && position == self.word.len())
            || SUFFIX.is_match(&state.suffix)
    }

    fn can_match(&self, state: &State) -> bool {
        state.prefix.is_some() || state.literal.is_some() || state.suffix.is_some()
    }

    fn accept(&self, state: &State, byte: u8) -> State {
        let cjk = &*CJK;
        let suffix = &*SUFFIX;
        let word = self.word.as_bytes();
        let suffix_start = if state
            .literal
            .is_some_and(|(position, _)| position == word.len())
        {
            suffix.start()
        } else {
            state.suffix
        };
        let mut literal = state.literal.and_then(|(position, prefixed)| {
            (word.get(position) == Some(&byte)).then_some((position + 1, prefixed))
        });
        // Both paths stay live when a non-CJK literal shares a UTF-8 lead byte.
        if state.prefix == cjk.start() && word.first() == Some(&byte) {
            literal = Some((1, state.has_prefix));
        }
        let mut prefix = cjk.accept(&state.prefix, byte);
        let mut has_prefix = state.has_prefix;
        if cjk.is_match(&prefix) {
            prefix = cjk.start();
            has_prefix = true;
        }
        State {
            prefix,
            has_prefix,
            literal,
            suffix: suffix.accept(&suffix_start, byte),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepts(automaton: &impl Automaton, text: &[u8]) -> bool {
        let mut state = automaton.start();
        for &byte in text {
            if !automaton.can_match(&state) {
                return false;
            }
            state = automaton.accept(&state, byte);
        }
        automaton.is_match(&state)
    }

    #[test]
    fn matches_the_previous_regex_for_ascii_and_unicode_words() {
        let words = [
            "rust",
            "py",
            "éclair",
            "λ",
            "ㄅ",
            "〆",
            "한글",
            "𐐨",
            "ｒｕｓｔ",
        ];
        let prefixes = ["", "の", "所有権", "x", "xの", "のx", "ㄅ", "\n"];
        let suffixes = [
            "",
            "の",
            "のpy",
            "所有権",
            "ing",
            "rust",
            "\n",
            "の\n",
            "のa\nb",
            "の\u{2028}",
            "の\0",
        ];
        for word in words {
            let escaped = regex::escape(word);
            let reference = Regex::new(&format!(
                "({CJK_PATTERN}+{escaped}({CJK_PATTERN}.*)?|{escaped}{CJK_PATTERN}.*)"
            ))
            .unwrap();
            let matcher = Matcher { word: word.into() };
            for prefix in prefixes {
                for suffix in suffixes {
                    let token = format!("{prefix}{word}{suffix}");
                    assert_eq!(
                        accepts(&matcher, token.as_bytes()),
                        accepts(&reference, token.as_bytes()),
                        "word={word:?}, token={token:?}"
                    );
                }
            }
            let mut invalid = format!("{word}の").into_bytes();
            invalid.push(0xff);
            assert!(!accepts(&matcher, &invalid));
            let token = format!("の{word}のpy");
            for end in 0..token.len() {
                assert_eq!(
                    accepts(&matcher, &token.as_bytes()[..end]),
                    accepts(&reference, &token.as_bytes()[..end]),
                    "word={word:?}, truncated token={:?}",
                    &token.as_bytes()[..end]
                );
            }
        }
    }

    #[test]
    fn requires_a_script_boundary_and_does_not_boost_bare_words_twice() {
        let matcher = Matcher {
            word: "rust".into(),
        };
        for token in ["rustの所有権", "rustのpy", "所有権rust", "所有権rustの説明"] {
            assert!(accepts(&matcher, token.as_bytes()), "{token}");
        }
        for token in [
            "rust",
            "trustの所有権",
            "rustaceanの所有権",
            "rustpy",
            "のrustpy",
        ] {
            assert!(!accepts(&matcher, token.as_bytes()), "{token}");
        }
        let first = MixedScriptQuery::new("rust", Field::from_field_id(0));
        let second = first.for_field(Field::from_field_id(1));
        assert!(Arc::ptr_eq(&first.matcher, &second.matcher));
    }
}
