// SPDX-License-Identifier: AGPL-3.0-or-later

use reqwest::RequestBuilder;

const MAX_BYTES: usize = 512;
const MAX_RANGES: usize = 16;

/// An explicit, validated Accept-Language preference. Empty means no header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AcceptLanguage(String);

impl AcceptLanguage {
    pub fn parse(value: &str) -> Result<Self, String> {
        if value.len() > MAX_BYTES {
            return Err(format!("accept_language must not exceed {MAX_BYTES} bytes"));
        }
        if !value.is_ascii()
            || value
                .bytes()
                .any(|byte| byte.is_ascii_control() && byte != b'\t')
        {
            return Err("accept_language must contain ASCII language ranges only".into());
        }
        let value = value.trim_matches([' ', '\t']);
        if value.is_empty() {
            return Ok(Self::default());
        }
        let mut normalized = String::with_capacity(value.len());
        for (index, item) in value.split(',').enumerate() {
            if index == MAX_RANGES {
                return Err(format!(
                    "accept_language must not exceed {MAX_RANGES} ranges"
                ));
            }
            let (range, weight) = item
                .split_once(';')
                .map_or((item, None), |(range, weight)| (range, Some(weight)));
            let range = range.trim_matches([' ', '\t']);
            if !valid_range(range) {
                return Err("accept_language contains an invalid language range".into());
            }
            if index != 0 {
                normalized.push(',');
            }
            normalized.push_str(&range.to_ascii_lowercase());
            if let Some(weight) = weight {
                let weight = weight.trim_matches([' ', '\t']);
                let value = weight
                    .strip_prefix("q=")
                    .or_else(|| weight.strip_prefix("Q="))
                    .ok_or("accept_language only supports q weights")?;
                let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
                if !matches!(whole, "0" | "1")
                    || fraction.len() > 3
                    || !fraction.bytes().all(|byte| byte.is_ascii_digit())
                    || (whole == "1" && fraction.bytes().any(|byte| byte != b'0'))
                {
                    return Err(
                        "accept_language q weights must be 0 to 1 with at most three decimals"
                            .into(),
                    );
                }
                if whole == "0" {
                    normalized.push_str(";q=0");
                    let fraction = fraction.trim_end_matches('0');
                    if !fraction.is_empty() {
                        normalized.push('.');
                        normalized.push_str(fraction);
                    }
                }
            }
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> Option<&str> {
        (!self.0.is_empty()).then_some(&self.0)
    }

    pub(super) fn apply(&self, request: RequestBuilder) -> RequestBuilder {
        match self.as_str() {
            Some(value) => request.header(reqwest::header::ACCEPT_LANGUAGE, value),
            None => request,
        }
    }
}

fn valid_range(range: &str) -> bool {
    if range == "*" {
        return true;
    }
    let mut subtags = range.split('-');
    let primary = subtags.next().unwrap_or_default();
    (1..=8).contains(&primary.len())
        && primary.bytes().all(|byte| byte.is_ascii_alphabetic())
        && subtags.all(|subtag| {
            (1..=8).contains(&subtag.len())
                && subtag.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_ranges_and_weights_without_reordering() {
        for (input, expected) in [
            (
                " KO-kr ; Q=1.000 , ko;q=0.900, en;q=0.8 ",
                "ko-kr,ko;q=0.9,en;q=0.8",
            ),
            (
                "zh-Hant-TW,en-US;q=0.001,*;q=0.000",
                "zh-hant-tw,en-us;q=0.001,*;q=0",
            ),
            (
                "es-419, x-private;q=0., i-klingon;q=1.",
                "es-419,x-private;q=0,i-klingon",
            ),
            ("en\t,\tfr", "en,fr"),
        ] {
            assert_eq!(
                AcceptLanguage::parse(input).unwrap().as_str(),
                Some(expected)
            );
        }
        assert_eq!(
            AcceptLanguage::parse(" \t").unwrap(),
            AcceptLanguage::default()
        );
    }

    #[test]
    fn rejects_injection_malformed_ranges_and_unbounded_values() {
        for input in [
            "en\r\nX-Test: injected",
            "en\0",
            "ko\n",
            "日本語",
            "en_US",
            "en-*",
            "en--US",
            "123",
            "abcdefghi",
            "en-abcdefghi",
            ",en",
            "en,",
            "en,,fr",
            "en;q=",
            "en;q=.5",
            "en;q=-1",
            "en;q=01",
            "en;q=1.001",
            "en;q=0.1234",
            "en;q=NaN",
            "en;q=1e0",
            "en;q=0.5;foo=bar",
            "en;foo=bar",
            "en;q =0.5",
        ] {
            assert!(AcceptLanguage::parse(input).is_err(), "{input:?}");
        }
        assert!(AcceptLanguage::parse(&["en"; MAX_RANGES + 1].join(",")).is_err());
        assert!(AcceptLanguage::parse(&" ".repeat(MAX_BYTES + 1)).is_err());
    }
}
