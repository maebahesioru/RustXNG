//! Material Icons engine, ported from searx/engines/material_icons.py.
//!
//! Google serves the icon metadata as JSON behind an XSSI prefix, so the
//! prefix is stripped before parsing. The "filled" modifier switches the
//! rendered variant and the filters of the result URL.
// engine-id: material_icons

use std::sync::Arc;

use crate::engine::util::*;
use crate::engine::{
    Engine, EngineDefaults, EngineError, EngineItem, EngineMeta, EngineRequest, EngineResponse,
};
use crate::query::Query;
use crate::results::Template;
use crate::settings::EngineConfig;

const DEFAULTS: EngineDefaults = EngineDefaults {
    categories: &["images", "icons"],
    shortcut: "mi",
    paging: false,
    time_range_support: false,
    safesearch: false,
    language_support: false,
    weight: 1.0,
    timeout: 4.0,
};

const SEARCH_URL: &str =
    "https://fonts.google.com/metadata/icons?key=material_symbols&incomplete=true";
const ICON_URL: &str = "https://fonts.google.com/icons?icon.query={query}&selected=Material+Symbols+Outlined:{icon_name}:FILL@0{fill};wght@400;GRAD@0;opsz@24";
const IMG_SRC_URL: &str = "https://fonts.gstatic.com/s/i/short-term/release/materialsymbolsoutlined/{icon_name}/{svg_type}/24px.svg";
const XSSI_PREFIX: &str = ")]}'";

/// Material Icons, Google's Material Symbols set.
pub struct MaterialIcons {
    meta: EngineMeta,
}

impl MaterialIcons {
    pub fn from_config(config: &EngineConfig) -> Result<Arc<Self>, String> {
        Ok(Arc::new(Self {
            meta: EngineMeta::from_config(config, DEFAULTS),
        }))
    }
}

impl Engine for MaterialIcons {
    fn meta(&self) -> &EngineMeta {
        &self.meta
    }

    fn build_request(&self, _query: &Query, _pageno: u32) -> Result<EngineRequest, EngineError> {
        // The whole catalogue is downloaded, the query is applied locally.
        Ok(EngineRequest::get(SEARCH_URL).header("Accept", "application/json"))
    }

    fn parse(
        &self,
        response: &EngineResponse,
        query: &Query,
    ) -> Result<Vec<EngineItem>, EngineError> {
        let body = response.text();
        let body = body.strip_prefix(XSSI_PREFIX).unwrap_or(&body);
        let json: serde_json::Value = serde_json::from_str(body.trim())
            .map_err(|error| EngineError::Parse(format!("invalid JSON response: {error}")))?;
        let Some(icons) = json_path(&json, "icons").and_then(|value| value.as_array()) else {
            return Ok(Vec::new());
        };
        let lowered = query.query_text().to_lowercase();
        let outlined = !lowered.contains("fill");
        let search = remove_filled(&lowered);
        let parts: Vec<String> = search.split(' ').map(str::to_string).collect();
        let svg_type = if outlined { "default" } else { "fill1" };
        let fill = if outlined { "0" } else { "1" };
        let mut results = Vec::new();
        for icon in icons {
            if !matches(icon, &parts) {
                continue;
            }
            let name = json_str(icon, "name");
            if name.is_empty() {
                continue;
            }
            let url = ICON_URL
                .replace("{query}", &name)
                .replace("{icon_name}", &name)
                .replace("{fill}", fill);
            let img_src = IMG_SRC_URL
                .replace("{icon_name}", &name)
                .replace("{svg_type}", svg_type);
            let tags = title_case_list(string_list(icon, "tags"));
            let categories = title_case_list(string_list(icon, "categories"));
            results.push(
                ResultBuilder::new(url, title_case(&name.replace('_', "")))
                    .content(format!("{tags} / {categories}"))
                    .image(img_src)
                    .template(Template::Images)
                    .build(),
            );
        }
        Ok(results)
    }
}

/// Upstream tests a query word against the name, the tags and the categories.
fn matches(icon: &serde_json::Value, parts: &[String]) -> bool {
    let name = json_str(icon, "name");
    let tags = string_list(icon, "tags");
    let categories = string_list(icon, "categories");
    for part in parts {
        if name.contains(part.as_str()) || tags.contains(part) || categories.contains(part) {
            return true;
        }
    }
    false
}

fn string_list(value: &serde_json::Value, path: &str) -> Vec<String> {
    let items = json_path(value, path).and_then(serde_json::Value::as_array);
    items
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The regex (fill)(ed)? removal of upstream, longest alternative first.
fn remove_filled(value: &str) -> String {
    let stripped = value.replace("filled", "").replace("fill", "");
    stripped.trim().to_string()
}

/// Python str.title(): a letter is uppercased when it follows a non-letter.
fn title_case(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut at_start = true;
    for character in value.chars() {
        if at_start && character.is_alphabetic() {
            output.extend(character.to_uppercase());
        } else if character.is_alphabetic() {
            output.extend(character.to_lowercase());
        } else {
            output.push(character);
        }
        at_start = !character.is_alphabetic();
    }
    output
}

fn title_case_list(values: Vec<String>) -> String {
    let cased: Vec<String> = values.iter().map(|value| title_case(value)).collect();
    cased.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r##")]}'
{"icons": [
  {"name": "home", "tags": ["house", "building"], "categories": ["action"]},
  {"name": "home_work", "tags": ["work"], "categories": ["action"]},
  {"name": "search", "tags": ["find"], "categories": ["action"]}
]}"##;

    fn engine() -> Arc<MaterialIcons> {
        let config = EngineConfig::new("material icons", "material_icons", "mi", &["icons"]);
        MaterialIcons::from_config(&config).unwrap()
    }

    fn icon_at(items: &[EngineItem], index: usize) -> &crate::results::SearchResult {
        match &items[index] {
            EngineItem::Result(found) => found,
            _ => panic!("expected an icon result"),
        }
    }

    fn parse(query_text: &str) -> Vec<EngineItem> {
        let response = EngineResponse::new(200, SEARCH_URL, FIXTURE.as_bytes().to_vec());
        let query = Query::parse(query_text, &crate::engine::EngineRegistry::default(), &[]);
        engine().parse(&response, &query).unwrap()
    }

    #[test]
    fn parses_the_outlined_variant() {
        let items = parse("home");
        assert_eq!(items.len(), 2);
        let first = icon_at(&items, 0);
        assert_eq!(first.title, "Home");
        assert_eq!(first.content, "House, Building / Action");
        assert_eq!(first.template, Template::Images);
        let url = first.url.clone();
        assert!(url.contains("Material+Symbols+Outlined:home:FILL@00;"));
        let image = first.img_src.clone().unwrap_or_default();
        assert!(image.ends_with("default/24px.svg"));
        assert_eq!(icon_at(&items, 1).title, "Homework");
    }

    #[test]
    fn the_filled_modifier_switches_the_variant() {
        let items = parse("filled home");
        assert_eq!(items.len(), 2);
        let first = icon_at(&items, 0);
        assert!(first.url.contains("FILL@01;"));
        let image = first.img_src.clone().unwrap_or_default();
        assert!(image.ends_with("fill1/24px.svg"));
    }

    #[test]
    fn other_words_select_another_icon() {
        let items = parse("search");
        assert_eq!(items.len(), 1);
        assert_eq!(icon_at(&items, 0).title, "Search");
        assert!(parse("nosuchicon").is_empty());
    }
}
