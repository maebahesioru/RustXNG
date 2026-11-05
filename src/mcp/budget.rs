//! Bounds for the serialized structured JSON, before JSON-RPC/SDK wrapping.
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

pub(crate) const MAX_PAYLOAD_TOKENS: usize = 64_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum PayloadTokenizer {
    #[serde(rename = "cl100k_base")]
    Cl100kBase,
    #[serde(rename = "o200k_base")]
    O200kBase,
}

impl PayloadTokenizer {
    fn name(self) -> &'static str {
        match self {
            Self::Cl100kBase => "cl100k_base",
            Self::O200kBase => "o200k_base",
        }
    }

    pub fn count(self, text: &str) -> usize {
        // get_encoding uses bundled ranks and caches the tokenizer. Treat web
        // text that resembles special tokens as ordinary text, never controls.
        tiktoken::get_encoding(self.name())
            .expect("supported bundled encoding")
            .count(text)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenBudgetInfo {
    pub tokenizer: PayloadTokenizer,
    pub max_tokens: usize,
}

#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct TokenBudget(Option<TokenBudgetInfo>);

impl TokenBudget {
    pub(crate) fn parse(name: &str, max_tokens: usize) -> Result<Self, String> {
        let name = name.trim();
        if name.is_empty() && max_tokens == 0 {
            return Ok(Self::default());
        }
        let tokenizer = match name {
            "cl100k_base" => PayloadTokenizer::Cl100kBase,
            "o200k_base" => PayloadTokenizer::O200kBase,
            "" => return Err("max_payload_tokens requires an explicit tokenizer".into()),
            _ => return Err("tokenizer must be cl100k_base or o200k_base".into()),
        };
        if !(1..=MAX_PAYLOAD_TOKENS).contains(&max_tokens) {
            return Err(format!(
                "max_payload_tokens must be between 1 and {MAX_PAYLOAD_TOKENS} when tokenizer is selected"
            ));
        }
        Ok(Self(Some(TokenBudgetInfo {
            tokenizer,
            max_tokens,
        })))
    }

    pub(crate) fn info(self) -> Option<TokenBudgetInfo> {
        self.0
    }

    pub(crate) fn fits<T: Serialize>(
        self,
        value: &T,
        max_chars: usize,
        max_bytes: usize,
    ) -> Result<bool, String> {
        // rmcp builds structuredContent via serde_json::Value. Use the same
        // representation/key order for token counting, including budget metadata.
        let json = if self.0.is_some() {
            serde_json::to_string(&serde_json::to_value(value).map_err(|e| e.to_string())?)
        } else {
            serde_json::to_string(value)
        }
        .map_err(|e| e.to_string())?;
        if json.len() > max_bytes || json.chars().count() > max_chars {
            return Ok(false);
        }
        Ok(self
            .0
            .is_none_or(|budget| budget.tokenizer.count(&json) <= budget.max_tokens))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_require_an_explicit_encoding_and_positive_bound() {
        assert!(TokenBudget::parse("", 0).unwrap().info().is_none());
        assert!(TokenBudget::parse("", 10).is_err());
        assert!(TokenBudget::parse("guess-from-model", 10).is_err());
        assert!(TokenBudget::parse("o200k_base", 0).is_err());
        assert!(TokenBudget::parse("cl100k_base", MAX_PAYLOAD_TOKENS + 1).is_err());
    }

    #[test]
    fn known_counts_and_special_looking_text_are_ordinary() {
        assert_eq!(PayloadTokenizer::Cl100kBase.count("hello world"), 2);
        assert_eq!(PayloadTokenizer::O200kBase.count("hello world"), 2);
        let text = "日本語<|endoftext|>😀";
        for name in ["cl100k_base", "o200k_base"] {
            let encoding = tiktoken::get_encoding(name).unwrap();
            assert_eq!(encoding.count(text), encoding.encode(text).len());
            assert!(encoding.count(text) > encoding.count_with_special_tokens(text));
        }
    }

    #[test]
    fn count_matches_the_structured_value_including_json_escapes() {
        #[derive(Serialize)]
        struct Reordered<'a> {
            z: &'a str,
            a: &'a str,
        }
        let value = Reordered {
            z: "日本語\n\"😀",
            a: "source",
        };
        let json = serde_json::to_value(&value).unwrap().to_string();
        let count = PayloadTokenizer::Cl100kBase.count(&json);
        assert!(
            TokenBudget::parse("cl100k_base", count)
                .unwrap()
                .fits(&value, 1000, 1000)
                .unwrap()
        );
        assert!(
            !TokenBudget::parse("cl100k_base", count - 1)
                .unwrap()
                .fits(&value, 1000, 1000)
                .unwrap()
        );
    }
}
