//! Page fetching shared by MCP `read_page` and HTTP raw content.
//!
//! [`direct`] owns the direct fetcher and the generic destination checks.
//! [`deepseek`] is an optional external backend selected by `[fetch]`, so
//! provider-specific code stays out of the MCP and search layers.

pub mod deepseek;
mod direct;
mod pdf;
mod render;

pub(crate) use direct::public_client;
#[cfg(test)]
pub(crate) use direct::test_public_client;
pub use direct::{MAX_BODY_BYTES, PageContent, PageReader, is_fetchable, is_global};

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::settings::{FetchProvider, FetchSettings, Outgoing};

/// A page returned by an external fetching backend.
#[derive(Clone, Debug)]
pub struct FetchedPage {
    pub text: Arc<str>,
    pub source_truncated: bool,
}

/// An object-safe page source. The direct fetcher lives in [`direct`]; this
/// trait is for opt-in providers such as the DeepSeek Fetch API.
pub trait PageSource: Send + Sync {
    fn fetch<'a>(
        &'a self,
        url: &'a reqwest::Url,
        max_chars: usize,
    ) -> BoxFuture<'a, Result<FetchedPage, String>>;
}

/// Build the external backend requested by `[fetch]`. `None` keeps the direct
/// fetcher.
pub fn from_settings(
    settings: &FetchSettings,
    outgoing: &Outgoing,
) -> Result<Option<Arc<dyn PageSource>>, String> {
    match settings.provider {
        FetchProvider::Direct => Ok(None),
        FetchProvider::DeepSeek => {
            let source = deepseek::DeepSeekSource::from_settings(
                &settings.deepseek,
                settings.timeout,
                outgoing,
            )?;
            Ok(Some(Arc::new(source) as Arc<dyn PageSource>))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_is_the_default_backend() {
        let settings = FetchSettings::default();
        assert!(
            from_settings(&settings, &Outgoing::default())
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn deepseek_without_a_key_is_an_error() {
        if std::env::var("DEEPSEEK_API_KEY").is_ok() {
            return;
        }
        let settings = FetchSettings {
            provider: FetchProvider::DeepSeek,
            ..FetchSettings::default()
        };
        assert!(from_settings(&settings, &Outgoing::default()).is_err());
    }
}
