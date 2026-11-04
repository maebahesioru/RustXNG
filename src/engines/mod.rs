//! Engine implementations, one module per upstream `searx/engines/*.py`.
//!
//! | module                | upstream                    |
//! |-----------------------|-----------------------------|
//! | `arxiv`               | `arxiv.py`                   |
//! | `bing`                | `bing.py`                    |
//! | `brave`               | `brave.py`                   |
//! | `crates`              | `crates.py`                  |
//! | `crossref`            | `crossref.py`                |
//! | `cinii`               | — (CiNii OpenSearch API)     |
//! | `qiita`               | — (Qiita API v2)             |
//! | `docker_hub`          | `docker_hub.py`              |
//! | `duckduckgo`          | `duckduckgo.py`              |
//! | `github`              | `github.py`                  |
//! | `hackernews`          | `hackernews.py`              |
//! | `mojeek`              | `mojeek.py`                  |
//! | `mwmbl`               | `mwmbl.py`                   |
//! | `npm`                 | `npm.py`                     |
//! | `openlibrary`         | `openlibrary.py`             |
//! | `openstreetmap`       | `openstreetmap.py`           |
//! | `photon`              | `photon.py`                  |
//! | `pypi`                | `pypi.py`                    |
//! | `semantic_scholar`    | `semantic_scholar.py`        |
//! | `stackexchange`       | `stackexchange.py`           |
//! | `wikidata`            | `wikidata.py`                |
//! | `wikipedia`           | `wikipedia.py`               |

pub mod acfun;
pub mod ahmia;
pub mod alpinelinux;
pub mod andisearch;
pub mod annas_archive;
pub mod ansa;
pub mod apkmirror;
pub mod apple_app_store;
pub mod apple_maps;
pub mod archlinux;
pub mod artic;
pub mod artstation;
pub mod arxiv;
pub mod astrophysics_data_system;
pub mod azure;
pub mod baidu;
pub mod bandcamp;
pub mod bilibili;
pub mod bing;
pub mod bing_images;
pub mod bing_news;
pub mod bing_videos;
pub mod bitchute;
pub mod boardreader;
pub mod bpb;
pub mod brave;
pub mod braveapi;
pub mod bt4g;
pub mod btdigg;
pub mod cachy_os;
pub mod ccc_media;
pub mod chatnoir;
pub mod chefkoch;
pub mod chinaso;
pub mod cinii;
pub mod cloudflareai;
pub mod core;
pub mod crates;
pub mod crossref;
pub mod currency_convert;
pub mod dailymotion;
pub mod deepl;
pub mod deepseek;
pub mod deezer;
pub mod destatis;
pub mod deviantart;
pub mod devicons;
pub mod dictzone;
pub mod discourse;
pub mod docker_hub;
pub mod dogpile;
pub mod duckduckgo;
pub mod duckduckgo_definitions;
pub mod duckduckgo_extra;
pub mod duckduckgo_weather;
pub mod duckduckgo_web;
pub mod duden;
pub mod ebay;
pub mod elasticsearch;
pub mod emojipedia;
pub mod europepmc;
pub mod exaapi;
pub mod fdroid;
pub mod findborg;
pub mod findfiles;
pub mod findthatmeme;
pub mod fireball;
pub mod five_hundred_px;
pub mod flaticon;
pub mod flickr;
pub mod flickr_noapi;
pub mod freesound;
pub mod frinkiac;
pub mod fyyd;
pub mod geizhals;
pub mod genius;
pub mod giphy;
pub mod gitea;
pub mod github;
pub mod github_code;
pub mod gitlab;
pub mod gmx;
pub mod goodreads;
pub mod grokipedia;
pub mod hackernews;
pub mod hex;
pub mod huggingface;
pub mod iconify;
pub mod il_post;
pub mod imdb;
pub mod imgur;
pub mod ina;
pub mod ipernity;
pub mod iqiyi;
pub mod iseek;
pub mod jina;
pub mod jisho;
pub mod json_engine;
pub mod kagi;
pub mod keenable;
pub mod kickass;
pub mod lemmy;
pub mod lib_rs;
pub mod libretranslate;
pub mod lucide;
pub mod luxxle;
pub mod magnific;
pub mod marginalia;
#[cfg(feature = "db-engines")]
pub mod mariadb_server;
pub mod mastodon;
pub mod mediathekviewweb;
pub mod mediawiki;
pub mod meilisearch;
pub mod metacpan;
pub mod microsoft_learn;
pub mod mixcloud;
pub mod mojeek;
#[cfg(feature = "db-engines")]
pub mod mongodb;
pub mod moviepilot;
pub mod mwmbl;
#[cfg(feature = "db-engines")]
pub mod mysql_server;
pub mod naver;
pub mod neocities;
pub mod neosearch;
pub mod niconico;
pub mod nine_gag;
pub mod npm;
pub mod nvd;
pub mod nyaa;
pub mod odysee;
pub mod ollama;
pub mod open_meteo;
pub mod openalex;
pub mod openclipart;
pub mod openlibrary;
pub mod opensemantic;
pub mod openstreetmap;
pub mod openverse;
pub mod pdbe;
pub mod peertube;
pub mod pexels;
pub mod photon;
pub mod picjumbo;
pub mod pinterest;
pub mod piratebay;
pub mod pixabay;
pub mod pixiv;
pub mod pkg_go_dev;
pub mod podchaser;
#[cfg(feature = "db-engines")]
pub mod postgresql;
pub mod privacywall;
pub mod public_domain_image_archive;
pub mod pubmed;
pub mod pypi;
pub mod qiita;
pub mod quark;
pub mod qwant;
pub mod r1337x;
pub mod r360search;
pub mod r360search_videos;
pub mod radio_browser;
pub mod recoll;
pub mod repology;
pub mod resulthunter;
pub mod reuters;
pub mod rottentomatoes;
pub mod rumble;
pub mod s1search_rampjs;
pub mod scanr_structures;
pub mod searchzee;
pub mod seekninja;
pub mod selfhst;
pub mod semantic_scholar;
pub mod senscritique;
pub mod sepiasearch;
pub mod seznam;
pub mod shopify_stock;
pub mod sogou;
pub mod sogou_images;
pub mod sogou_videos;
pub mod sogou_wechat;
pub mod solidtorrents;
pub mod solr;
pub mod soundcloud;
pub mod sourcehut;
pub mod spotify;
pub mod springer;
#[cfg(feature = "db-engines")]
pub mod sqlite;
pub mod stackexchange;
pub mod steam;
pub mod stocksnap;
pub mod swisscows;
pub mod swisscows_news;
pub mod tagesschau;
pub mod tiger;
pub mod tineye;
pub mod tokyotoshokan;
pub mod tootfinder;
pub mod torznab;
pub mod translated;
pub mod tusksearch;
pub mod unsplash;
pub mod uxwing;
#[cfg(feature = "db-engines")]
pub mod valkey_server;
pub mod vimeo;
pub mod voidlinux;
pub mod vuhuv;
pub mod wallhaven;
pub mod wikicommons;
pub mod wikidata;
pub mod wikipedia;
pub mod wolframalpha_api;
pub mod wolframalpha_noapi;
pub mod wordnik;
pub mod wttr;
pub mod www1x;
pub mod xpath;
pub mod xprivo;
pub mod yacy;
pub mod yahoo;
pub mod yahoo_japan;
pub mod yahoo_news;
pub mod yandex;
pub mod yandex_api;
pub mod yandex_music;
pub mod yep;
pub mod zlibrary;

use crate::engine::EngineHandle;
use crate::settings::EngineConfig;

/// Build the engine implementation selected by `config.engine`.
pub fn build(config: &EngineConfig) -> Result<EngineHandle, String> {
    let handle: EngineHandle = match config.engine.as_str() {
        "arxiv" => arxiv::Arxiv::from_config(config)?,
        "bing" => bing::Bing::from_config(config)?,
        "bing_news" => bing_news::BingNews::from_config(config)?,
        "brave" => brave::Brave::from_config(config)?,
        "crates" => crates::Crates::from_config(config)?,
        "crossref" => crossref::Crossref::from_config(config)?,
        "dailymotion" => dailymotion::Dailymotion::from_config(config)?,
        "deezer" => deezer::Deezer::from_config(config)?,
        "docker_hub" => docker_hub::DockerHub::from_config(config)?,
        "duckduckgo" => duckduckgo::DuckDuckGo::from_config(config)?,
        "github" => github::GitHub::from_config(config)?,
        "hackernews" => hackernews::HackerNews::from_config(config)?,
        "mojeek" => mojeek::Mojeek::from_config(config)?,
        "mwmbl" => mwmbl::Mwmbl::from_config(config)?,
        "npm" => npm::Npm::from_config(config)?,
        "openlibrary" => openlibrary::OpenLibrary::from_config(config)?,
        "openstreetmap" => openstreetmap::Openstreetmap::from_config(config)?,
        "openverse" => openverse::Openverse::from_config(config)?,
        "photon" => photon::Photon::from_config(config)?,
        "pypi" => pypi::PyPi::from_config(config)?,
        "semantic_scholar" => semantic_scholar::SemanticScholar::from_config(config)?,
        "stackexchange" => stackexchange::StackExchange::from_config(config)?,
        "wikicommons" => wikicommons::Wikicommons::from_config(config)?,
        "wikidata" => wikidata::Wikidata::from_config(config)?,
        "wikipedia" => wikipedia::Wikipedia::from_config(config)?,
        "json_engine" => json_engine::JsonEngine::from_config(config)?,
        "mediawiki" => mediawiki::MediaWiki::from_config(config)?,
        "xpath" => xpath::XPathEngine::from_config(config)?,
        "andisearch" => andisearch::Andisearch::from_config(config)?,
        "apkmirror" => apkmirror::ApkMirror::from_config(config)?,
        "apple_app_store" => apple_app_store::AppleAppStore::from_config(config)?,
        "baidu" => baidu::Baidu::from_config(config)?,
        "dogpile" => dogpile::Dogpile::from_config(config)?,
        "findborg" => findborg::Findborg::from_config(config)?,
        "findfiles" => findfiles::FindFiles::from_config(config)?,
        "fireball" => fireball::Fireball::from_config(config)?,
        "500px" => five_hundred_px::FiveHundredPx::from_config(config)?,
        "lemmy" => lemmy::Lemmy::from_config(config)?,
        "luxxle" => luxxle::Luxxle::from_config(config)?,
        "mastodon" => mastodon::Mastodon::from_config(config)?,
        "naver" => naver::Naver::from_config(config)?,
        "9gag" => nine_gag::NineGag::from_config(config)?,
        "openclipart" => openclipart::OpenClipart::from_config(config)?,
        "peertube" => peertube::Peertube::from_config(config)?,
        "pexels" => pexels::Pexels::from_config(config)?,
        "pixabay" => pixabay::Pixabay::from_config(config)?,
        "qwant" => qwant::Qwant::from_config(config)?,
        "sepiasearch" => sepiasearch::Sepiasearch::from_config(config)?,
        "tootfinder" => tootfinder::Tootfinder::from_config(config)?,
        "tusksearch" => tusksearch::Tusksearch::from_config(config)?,
        "unsplash" => unsplash::Unsplash::from_config(config)?,
        "bandcamp" => bandcamp::Bandcamp::from_config(config)?,
        "genius" => genius::Genius::from_config(config)?,
        "goodreads" => goodreads::Goodreads::from_config(config)?,
        "imdb" => imdb::Imdb::from_config(config)?,
        "mixcloud" => mixcloud::Mixcloud::from_config(config)?,
        "soundcloud" => soundcloud::Soundcloud::from_config(config)?,
        "wallhaven" => wallhaven::Wallhaven::from_config(config)?,
        "artstation" => artstation::Artstation::from_config(config)?,
        "deviantart" => deviantart::Deviantart::from_config(config)?,
        "flickr_noapi" => flickr_noapi::FlickrNoapi::from_config(config)?,
        "giphy" => giphy::Giphy::from_config(config)?,
        "imgur" => imgur::Imgur::from_config(config)?,
        "alpinelinux" => alpinelinux::AlpineLinux::from_config(config)?,
        "annas_archive" => annas_archive::AnnasArchive::from_config(config)?,
        "discourse" => discourse::Discourse::from_config(config)?,
        "duckduckgo_extra" => duckduckgo_extra::DuckDuckGoExtra::from_config(config)?,
        "duckduckgo_web" => duckduckgo_web::DuckDuckGoWeb::from_config(config)?,
        "1337x" => r1337x::_1337x::from_config(config)?,
        "360search" => r360search::Search360::from_config(config)?,
        "artic" => artic::Artic::from_config(config)?,
        "bing_images" => bing_images::BingImages::from_config(config)?,
        "bing_videos" => bing_videos::BingVideos::from_config(config)?,
        "bitchute" => bitchute::Bitchute::from_config(config)?,
        "bpb" => bpb::Bpb::from_config(config)?,
        "braveapi" => braveapi::BraveApi::from_config(config)?,
        "cachy_os" => cachy_os::CachyOs::from_config(config)?,
        "ccc_media" => ccc_media::CccMedia::from_config(config)?,
        "chinaso" => chinaso::ChinaSo::from_config(config)?,
        "cinii" => cinii::Cinii::from_config(config)?,
        "cloudflareai" => cloudflareai::CloudflareAi::from_config(config)?,
        "core" => core::Core::from_config(config)?,
        "360search_videos" => r360search_videos::Videoscarch360::from_config(config)?,
        "iqiyi" => iqiyi::Iqiyi::from_config(config)?,
        "niconico" => niconico::Niconico::from_config(config)?,
        "odysee" => odysee::Odysee::from_config(config)?,
        "rumble" => rumble::Rumble::from_config(config)?,
        "vimeo" => vimeo::Vimeo::from_config(config)?,
        "github_code" => github_code::GitHubCode::from_config(config)?,
        "gitlab" => gitlab::GitLab::from_config(config)?,
        "marginalia" => marginalia::Marginalia::from_config(config)?,
        "seznam" => seznam::Seznam::from_config(config)?,
        "yahoo" => yahoo::Yahoo::from_config(config)?,
        "yahoo_japan" => yahoo_japan::YahooJapan::from_config(config)?,
        "yandex" => yandex::Yandex::from_config(config)?,
        "ebay" => ebay::Ebay::from_config(config)?,
        "europepmc" => europepmc::EuropePmc::from_config(config)?,
        "geizhals" => geizhals::Geizhals::from_config(config)?,
        "gitea" => gitea::Gitea::from_config(config)?,
        "hex" => hex::Hex::from_config(config)?,
        "neosearch" => neosearch::Neosearch::from_config(config)?,
        "openalex" => openalex::OpenAlex::from_config(config)?,
        "pkg_go_dev" => pkg_go_dev::PkgGoDev::from_config(config)?,
        "sogou" => sogou::Sogou::from_config(config)?,
        "sogou_images" => sogou_images::SogouImages::from_config(config)?,
        "sogou_videos" => sogou_videos::SogouVideos::from_config(config)?,
        "sourcehut" => sourcehut::SourceHut::from_config(config)?,
        "springer" => springer::Springer::from_config(config)?,
        "steam" => steam::Steam::from_config(config)?,
        "swisscows" => swisscows::Swisscows::from_config(config)?,
        "www1x" => www1x::Www1x::from_config(config)?,
        "yep" => yep::Yep::from_config(config)?,
        "duden" => duden::Duden::from_config(config)?,
        "fdroid" => fdroid::Fdroid::from_config(config)?,
        "flaticon" => flaticon::Flaticon::from_config(config)?,
        "flickr" => flickr::Flickr::from_config(config)?,
        "iconify" => iconify::Iconify::from_config(config)?,
        "jisho" => jisho::Jisho::from_config(config)?,
        "pinterest" => pinterest::Pinterest::from_config(config)?,
        "pixiv" => pixiv::Pixiv::from_config(config)?,
        "pubmed" => pubmed::PubMed::from_config(config)?,
        "qiita" => qiita::Qiita::from_config(config)?,
        "quark" => quark::Quark::from_config(config)?,
        "repology" => repology::Repology::from_config(config)?,
        "shopify_stock" => shopify_stock::ShopifyStock::from_config(config)?,
        "wordnik" => wordnik::Wordnik::from_config(config)?,
        "emojipedia" => emojipedia::Emojipedia::from_config(config)?,
        "wolframalpha_api" => wolframalpha_api::WolframAlphaApi::from_config(config)?,
        "currency_convert" => currency_convert::CurrencyConvert::from_config(config)?,
        "dictzone" => dictzone::Dictzone::from_config(config)?,
        "kickass" => kickass::Kickass::from_config(config)?,
        "nyaa" => nyaa::Nyaa::from_config(config)?,
        "open_meteo" => open_meteo::OpenMeteo::from_config(config)?,
        "piratebay" => piratebay::Piratebay::from_config(config)?,
        "solidtorrents" => solidtorrents::SolidTorrents::from_config(config)?,
        "tokyotoshokan" => tokyotoshokan::TokyoToshokan::from_config(config)?,
        "wolframalpha_noapi" => wolframalpha_noapi::WolframAlphaNoapi::from_config(config)?,
        "acfun" => acfun::Acfun::from_config(config)?,
        "ahmia" => ahmia::Ahmia::from_config(config)?,
        "bilibili" => bilibili::Bilibili::from_config(config)?,
        "bt4g" => bt4g::Bt4g::from_config(config)?,
        "btdigg" => btdigg::Btdigg::from_config(config)?,
        "chefkoch" => chefkoch::Chefkoch::from_config(config)?,
        "destatis" => destatis::Destatis::from_config(config)?,
        "huggingface" => huggingface::HuggingFace::from_config(config)?,
        "jina" => jina::Jina::from_config(config)?,
        "lib_rs" => lib_rs::LibRs::from_config(config)?,
        "metacpan" => metacpan::Metacpan::from_config(config)?,
        "nvd" => nvd::Nvd::from_config(config)?,
        "pdbe" => pdbe::Pdbe::from_config(config)?,
        "torznab" => torznab::Torznab::from_config(config)?,
        "ansa" => ansa::Ansa::from_config(config)?,
        "apple_maps" => apple_maps::AppleMaps::from_config(config)?,
        "archlinux" => archlinux::Archlinux::from_config(config)?,
        "boardreader" => boardreader::Boardreader::from_config(config)?,
        "devicons" => devicons::Devicons::from_config(config)?,
        "duckduckgo_definitions" => {
            duckduckgo_definitions::DuckDuckGoDefinitions::from_config(config)?
        }
        "grokipedia" => grokipedia::Grokipedia::from_config(config)?,
        "il_post" => il_post::IlPost::from_config(config)?,
        "ipernity" => ipernity::Ipernity::from_config(config)?,
        "magnific" => magnific::Magnific::from_config(config)?,
        "mediathekviewweb" => mediathekviewweb::Mediathekviewweb::from_config(config)?,
        "microsoft_learn" => microsoft_learn::MicrosoftLearn::from_config(config)?,
        "moviepilot" => moviepilot::Moviepilot::from_config(config)?,
        "neocities" => neocities::Neocities::from_config(config)?,
        "picjumbo" => picjumbo::Picjumbo::from_config(config)?,
        "podchaser" => podchaser::Podchaser::from_config(config)?,
        "reuters" => reuters::Reuters::from_config(config)?,
        "rottentomatoes" => rottentomatoes::RottenTomatoes::from_config(config)?,
        "s1search_rampjs" => s1search_rampjs::S1SearchRampjs::from_config(config)?,
        "scanr_structures" => scanr_structures::ScanrStructures::from_config(config)?,
        "iseek" => iseek::Iseek::from_config(config)?,
        "keenable" => keenable::Keenable::from_config(config)?,
        "libretranslate" => libretranslate::LibreTranslate::from_config(config)?,
        "privacywall" => privacywall::Privacywall::from_config(config)?,
        "public_domain_image_archive" => {
            public_domain_image_archive::PublicDomainImageArchive::from_config(config)?
        }
        "radio_browser" => radio_browser::RadioBrowser::from_config(config)?,
        "astrophysics_data_system" => {
            astrophysics_data_system::AstrophysicsDataSystem::from_config(config)?
        }
        "chatnoir" => chatnoir::Chatnoir::from_config(config)?,
        "findthatmeme" => findthatmeme::FindThatMeme::from_config(config)?,
        "frinkiac" => frinkiac::Frinkiac::from_config(config)?,
        "fyyd" => fyyd::Fyyd::from_config(config)?,
        "gmx" => gmx::Gmx::from_config(config)?,
        "ina" => ina::Ina::from_config(config)?,
        "lucide" => lucide::Lucide::from_config(config)?,
        "tiger" => tiger::Tiger::from_config(config)?,
        "tineye" => tineye::Tineye::from_config(config)?,
        "uxwing" => uxwing::Uxwing::from_config(config)?,
        "deepl" => deepl::Deepl::from_config(config)?,
        "resulthunter" => resulthunter::Resulthunter::from_config(config)?,
        "searchzee" => searchzee::SearchZee::from_config(config)?,
        "seekninja" => seekninja::SeekNinja::from_config(config)?,
        "selfhst" => selfhst::Selfhst::from_config(config)?,
        "senscritique" => senscritique::Senscritique::from_config(config)?,
        "sogou_wechat" => sogou_wechat::SogouWechat::from_config(config)?,
        "spotify" => spotify::Spotify::from_config(config)?,
        "stocksnap" => stocksnap::Stocksnap::from_config(config)?,
        "swisscows_news" => swisscows_news::SwisscowsNews::from_config(config)?,
        "tagesschau" => tagesschau::Tagesschau::from_config(config)?,
        "voidlinux" => voidlinux::VoidLinux::from_config(config)?,
        "vuhuv" => vuhuv::Vuhuv::from_config(config)?,
        "xprivo" => xprivo::Xprivo::from_config(config)?,
        "exaapi" => exaapi::ExaApi::from_config(config)?,
        "deepseek" => deepseek::DeepSeek::from_config(config)?,
        "freesound" => freesound::Freesound::from_config(config)?,
        "kagi" => kagi::Kagi::from_config(config)?,
        "ollama" => ollama::Ollama::from_config(config)?,
        "translated" => translated::Translated::from_config(config)?,
        "wttr" => wttr::Wttr::from_config(config)?,
        "yacy" => yacy::Yacy::from_config(config)?,
        "yahoo_news" => yahoo_news::YahooNews::from_config(config)?,
        "azure" => azure::Azure::from_config(config)?,
        "meilisearch" => meilisearch::Meilisearch::from_config(config)?,
        "solr" => solr::Solr::from_config(config)?,
        "elasticsearch" => elasticsearch::Elasticsearch::from_config(config)?,
        "yandex_api" => yandex_api::YandexApi::from_config(config)?,
        "duckduckgo_weather" => duckduckgo_weather::DuckDuckGoWeather::from_config(config)?,
        "yandex_music" => yandex_music::YandexMusic::from_config(config)?,
        "zlibrary" => zlibrary::Zlibrary::from_config(config)?,
        #[cfg(feature = "db-engines")]
        "mariadb_server" => mariadb_server::MariadbServer::from_config(config)?,
        #[cfg(feature = "db-engines")]
        "mongodb" => mongodb::Mongodb::from_config(config)?,
        #[cfg(feature = "db-engines")]
        "mysql_server" => mysql_server::MysqlServer::from_config(config)?,
        "opensemantic" => opensemantic::OpenSemantic::from_config(config)?,
        #[cfg(feature = "db-engines")]
        "postgresql" => postgresql::Postgresql::from_config(config)?,
        "recoll" => recoll::Recoll::from_config(config)?,
        #[cfg(feature = "db-engines")]
        "sqlite" => sqlite::Sqlite::from_config(config)?,
        #[cfg(feature = "db-engines")]
        "valkey_server" => valkey_server::ValkeyServer::from_config(config)?,
        other => {
            return Err(format!(
                "engine {:?} uses the unknown engine type {other:?}",
                config.name
            ));
        }
    };
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    #[test]
    fn every_default_engine_builds() {
        let settings = Settings::defaults();
        for config in settings.enabled_engines() {
            build(config).unwrap_or_else(|error| {
                panic!("engine {:?} could not be built: {error}", config.name)
            });
        }
    }

    #[test]
    fn unknown_engine_types_are_rejected() {
        let config = EngineConfig::new("nope", "not_an_engine", "np", &["general"]);
        assert!(build(&config).is_err());
    }
}
