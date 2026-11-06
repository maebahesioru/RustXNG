//! A small XPath subset that is enough for the declarative engines of
//! SearXNG's `settings.yml` (`engine: xpath`).
//!
//! Supported syntax:
//!
//! * `//tag`, `.//tag`, `./tag`, `/tag`, wildcard `*`
//! * predicates `[@attr]`, `[@attr="v"]`, `[contains(@attr, "v")]`,
//!   `[1]`, `[last()]` (also on a parenthesised path: `(./a/@href)[last()]`)
//! * terminals `/@attr` and `/text()`, otherwise the element text is used
//! * value functions `concat(...)`, `substring-before(expr, "s")`,
//!   `substring-after(expr, "s")`
//!
//! Anything else is rejected with an error, never silently mis-parsed.

use scraper::{ElementRef, Html, Selector};

use crate::engine::xml::{XmlDocument, XmlElement};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Predicate {
    Exists(String),
    Equals(String, String),
    Contains(String, String),
    /// starts-with(@attr, "prefix") — used by ayo's suggestion filter.
    StartsWith(String, String),
    /// A bare element test such as [h5]: the node must have a child element
    /// with that tag. Used by anaconda's content_xpath (./td[h5]/text()).
    Child(String),
    Position(usize),
    Last,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Step {
    descendant: bool,
    tag: String,
    predicates: Vec<Predicate>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Terminal {
    Element,
    Attribute(String),
    Text,
}

/// A parsed XPath expression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XPath {
    steps: Vec<Step>,
    terminal: Terminal,
}

/// A value selected by an XPath expression.
#[derive(Debug, Clone)]
pub enum Node<'a> {
    Element(ElementRef<'a>),
    Text(String),
}

impl<'a> Node<'a> {
    /// The text of the node (element text is collapsed like upstream's
    /// `extract_text`).
    pub fn text(&self) -> String {
        match self {
            Self::Element(element) => crate::engine::util::text(*element),
            Self::Text(text) => crate::engine::util::clean(text),
        }
    }
}

impl XPath {
    /// Parse an expression, e.g. `//div[@class="result"]/a/@href`.
    pub fn parse(expression: &str) -> Result<Self, String> {
        let trimmed = expression.trim();
        if trimmed.is_empty() {
            return Err("empty XPath expression".to_string());
        }
        // (path)[last()] / (path)[1]: apply the position to the deepest step.
        if trimmed.starts_with('(') {
            let closing = trimmed
                .rfind(")[")
                .ok_or_else(|| format!("unsupported parenthesised XPath {expression:?}"))?;
            let inner = &trimmed[1..closing];
            let suffix = &trimmed[closing + 1..];
            let mut path = XPath::parse(inner)?;
            let predicate = parse_predicate(suffix)?;
            match path.steps.last_mut() {
                Some(step) => step.predicates.push(predicate),
                None => return Err(format!("nothing to index in {expression:?}")),
            }
            return Ok(path);
        }
        let mut steps = Vec::new();
        let mut terminal = Terminal::Element;
        let mut rest = trimmed;
        // A leading '.' only means "relative to the context node".
        if let Some(stripped) = rest.strip_prefix('.') {
            rest = stripped;
        }
        while !rest.is_empty() {
            let descendant = if let Some(stripped) = rest.strip_prefix("//") {
                rest = stripped;
                true
            } else if let Some(stripped) = rest.strip_prefix('/') {
                rest = stripped;
                false
            } else if steps.is_empty() {
                // "div/a" without a leading slash: descendants of the context.
                true
            } else {
                return Err(format!("unexpected {rest:?} in {expression:?}"));
            };
            if let Some(stripped) = rest.strip_prefix('@') {
                let end = stripped
                    .find(|character: char| {
                        !(character.is_alphanumeric() || "-_:".contains(character))
                    })
                    .unwrap_or(stripped.len());
                terminal = Terminal::Attribute(stripped[..end].to_string());
                break;
            }
            if rest.starts_with("text()") {
                terminal = Terminal::Text;
                break;
            }
            let end = rest
                .find(|character: char| {
                    !(character.is_alphanumeric() || "-_:*".contains(character))
                })
                .unwrap_or(rest.len());
            if end == 0 {
                return Err(format!("missing element name in {expression:?}"));
            }
            let tag = rest[..end].to_string();
            rest = &rest[end..];
            let mut predicates = Vec::new();
            while let Some(after_open) = rest.strip_prefix('[') {
                let closing = find_closing_bracket(after_open)
                    .ok_or_else(|| format!("unclosed predicate in {expression:?}"))?;
                predicates.push(parse_predicate(&format!("[{}]", &after_open[..closing]))?);
                rest = &after_open[closing + 1..];
            }
            steps.push(Step {
                descendant,
                tag,
                predicates,
            });
        }
        if steps.is_empty() && matches!(terminal, Terminal::Element) {
            return Err(format!("no element steps in {expression:?}"));
        }
        Ok(XPath { steps, terminal })
    }

    /// Select the matching values from a document.
    pub fn select<'a>(&self, root: &'a Html) -> Vec<Node<'a>> {
        let mut nodes: Vec<ElementRef<'a>> = vec![root.root_element()];
        for step in &self.steps {
            let mut matched: Vec<ElementRef<'a>> = Vec::new();
            for element in &nodes {
                let candidates: Vec<ElementRef<'a>> = if step.descendant {
                    element.descendants().filter_map(ElementRef::wrap).collect()
                } else {
                    element.children().filter_map(ElementRef::wrap).collect()
                };
                matched.extend(
                    candidates
                        .into_iter()
                        .filter(|candidate| step.matches(*candidate)),
                );
            }
            nodes = step.apply_positions(matched);
        }
        self.terminal_map(nodes)
    }

    /// Select the matching values from a subtree (used for per-result fields).
    pub fn select_in<'a>(&self, context: ElementRef<'a>) -> Vec<Node<'a>> {
        let mut nodes: Vec<ElementRef<'a>> = vec![context];
        for step in &self.steps {
            let mut matched: Vec<ElementRef<'a>> = Vec::new();
            for element in &nodes {
                let candidates: Vec<ElementRef<'a>> = if step.descendant {
                    element.descendants().filter_map(ElementRef::wrap).collect()
                } else {
                    element.children().filter_map(ElementRef::wrap).collect()
                };
                matched.extend(
                    candidates
                        .into_iter()
                        .filter(|candidate| step.matches(*candidate)),
                );
            }
            nodes = step.apply_positions(matched);
        }
        self.terminal_map(nodes)
    }

    fn terminal_map<'a>(&self, nodes: Vec<ElementRef<'a>>) -> Vec<Node<'a>> {
        match &self.terminal {
            Terminal::Element => nodes.into_iter().map(Node::Element).collect(),
            Terminal::Text => nodes
                .into_iter()
                .map(|node| Node::Text(crate::engine::util::text(node)))
                .collect(),
            Terminal::Attribute(name) => nodes
                .into_iter()
                .filter_map(|element| {
                    element
                        .value()
                        .attr(name)
                        .map(|value| Node::Text(value.trim().to_string()))
                })
                .collect(),
        }
    }
    /// Select matching values from an XML document (see crate::engine::xml).
    ///
    /// Mirrors XPath::select step for step, but walks the XML tree instead of an
    /// HTML tree. Kept as a parallel implementation rather than a generic one
    /// because the HTML path must not change behaviour at all: every CSS-scraped
    /// engine depends on it.
    pub fn select_xml<'a>(&self, document: &'a XmlDocument) -> Vec<XmlNode<'a>> {
        let mut nodes: Vec<&XmlElement> = document.roots.iter().collect();
        for (index, step) in self.steps.iter().enumerate() {
            let mut matched: Vec<&XmlElement> = Vec::new();
            for element in &nodes {
                // The document root is itself a candidate for the first step.
                // XPath's //RDF matches the document element, but the loop below
                // only ever looks at children/descendants, so without this the
                // root would be unreachable and //RDF//item could never match.
                if index == 0 && step.matches_xml(element) {
                    matched.push(element);
                }
                let candidates: Vec<&XmlElement> = if step.descendant {
                    element.descendants()
                } else {
                    element.children.iter().collect()
                };
                matched.extend(candidates.into_iter().filter(|c| step.matches_xml(c)));
            }
            nodes = step.apply_positions_xml(matched);
        }
        self.terminal_map_xml(nodes)
    }

    /// Select matching values from an XML subtree (per-result fields).
    pub fn select_xml_in<'a>(&self, context: &'a XmlElement) -> Vec<XmlNode<'a>> {
        let mut nodes: Vec<&XmlElement> = vec![context];
        for step in &self.steps {
            let mut matched: Vec<&XmlElement> = Vec::new();
            for element in &nodes {
                let candidates: Vec<&XmlElement> = if step.descendant {
                    element.descendants()
                } else {
                    element.children.iter().collect()
                };
                matched.extend(candidates.into_iter().filter(|c| step.matches_xml(c)));
            }
            nodes = step.apply_positions_xml(matched);
        }
        self.terminal_map_xml(nodes)
    }

    fn terminal_map_xml<'a>(&self, nodes: Vec<&'a XmlElement>) -> Vec<XmlNode<'a>> {
        match &self.terminal {
            Terminal::Element => nodes.into_iter().map(XmlNode::Element).collect(),
            Terminal::Text => nodes
                .into_iter()
                .map(|element| XmlNode::Text(crate::engine::util::clean(&xml_text(element))))
                .collect(),
            Terminal::Attribute(name) => nodes
                .into_iter()
                .filter_map(|element| {
                    element
                        .attr(name)
                        .map(|v| XmlNode::Text(v.trim().to_string()))
                })
                .collect(),
        }
    }
}

/// A value selected from an XML document.
#[derive(Debug, Clone)]
pub enum XmlNode<'a> {
    Element(&'a XmlElement),
    Text(String),
}

impl<'a> XmlNode<'a> {
    /// The text of the node, collapsed like Node::text.
    pub fn text(&self) -> String {
        match self {
            Self::Element(element) => crate::engine::util::clean(&xml_text(element)),
            Self::Text(text) => crate::engine::util::clean(text),
        }
    }
}

/// The text of an element and its descendants, joined with spaces.
///
/// Mirrors scraper's ElementRef::text so an XML result and an HTML result
/// produce the same string for the same markup.
fn xml_text(element: &XmlElement) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !element.text.trim().is_empty() {
        parts.push(element.text.clone());
    }
    for child in &element.children {
        let text = xml_text(child);
        if !text.trim().is_empty() {
            parts.push(text);
        }
    }
    parts.join(" ")
}

impl Step {
    /// Positional predicates over an XML node set (see apply_positions).
    fn apply_positions_xml<'a>(&self, matched: Vec<&'a XmlElement>) -> Vec<&'a XmlElement> {
        let mut current = matched;
        for predicate in &self.predicates {
            match predicate {
                Predicate::Position(index) => {
                    current = if *index > 0 && *index <= current.len() {
                        vec![current[*index - 1]]
                    } else {
                        Vec::new()
                    };
                }
                Predicate::Last => {
                    current = current.last().copied().map(|e| vec![e]).unwrap_or_default();
                }
                _ => {}
            }
        }
        current
    }

    /// Whether an XML element matches this step.
    ///
    /// Tag matching is namespace tolerant (see XmlElement::matches_tag), so a
    /// feed that puts its items in a default namespace still answers to //item.
    fn matches_xml(&self, element: &XmlElement) -> bool {
        if !element.matches_tag(&self.tag) {
            return false;
        }
        self.predicates.iter().all(|predicate| match predicate {
            Predicate::Exists(attribute) => element.attr(attribute).is_some(),
            Predicate::Equals(attribute, expected) => {
                element.attr(attribute) == Some(expected.as_str())
            }
            Predicate::Contains(attribute, needle) => element
                .attr(attribute)
                .map(|value| value.contains(needle.as_str()))
                .unwrap_or(false),
            Predicate::StartsWith(attribute, prefix) => element
                .attr(attribute)
                .map(|value| value.starts_with(prefix.as_str()))
                .unwrap_or(false),
            Predicate::Child(tag) => element
                .children
                .iter()
                .any(|child| tag == "*" || child.matches_tag(tag)),
            Predicate::Position(_) | Predicate::Last => true,
        })
    }
}

impl Step {
    /// Positional predicates apply to the combined result of the step, exactly
    /// like an XPath node set.
    fn apply_positions<'a>(&self, matched: Vec<ElementRef<'a>>) -> Vec<ElementRef<'a>> {
        let mut current = matched;
        for predicate in &self.predicates {
            match predicate {
                Predicate::Position(index) => {
                    current = if *index > 0 && *index <= current.len() {
                        vec![current[*index - 1]]
                    } else {
                        Vec::new()
                    };
                }
                Predicate::Last => {
                    current = current
                        .last()
                        .copied()
                        .map(|element| vec![element])
                        .unwrap_or_default();
                }
                _ => {}
            }
        }
        current
    }

    fn matches(&self, element: ElementRef<'_>) -> bool {
        if self.tag != "*" && element.value().name() != self.tag {
            return false;
        }
        self.predicates.iter().all(|predicate| match predicate {
            Predicate::Exists(attribute) => element.value().attr(attribute).is_some(),
            Predicate::Equals(attribute, expected) => {
                element.value().attr(attribute) == Some(expected.as_str())
            }
            Predicate::Contains(attribute, needle) => element
                .value()
                .attr(attribute)
                .map(|value| value.contains(needle.as_str()))
                .unwrap_or(false),
            Predicate::StartsWith(attribute, prefix) => element
                .value()
                .attr(attribute)
                .map(|value| value.starts_with(prefix.as_str()))
                .unwrap_or(false),
            Predicate::Child(tag) => element
                .children()
                .filter_map(ElementRef::wrap)
                .any(|child| tag == "*" || child.value().name() == tag),
            Predicate::Position(_) | Predicate::Last => true,
        })
    }
}

fn find_closing_bracket(text: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (index, character) in text.char_indices() {
        match quote {
            Some(active) if character == active => {
                quote = None;
            }
            Some(_) => {}
            None => match character {
                '\'' | '"' => quote = Some(character),
                '[' => depth += 1,
                ']' if depth == 0 => return Some(index),
                ']' => depth -= 1,
                _ => {}
            },
        }
    }
    None
}

fn parse_predicate(text: &str) -> Result<Predicate, String> {
    let inner = text
        .trim()
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(text)
        .trim();
    if inner == "last()" {
        return Ok(Predicate::Last);
    }
    if let Ok(index) = inner.parse::<usize>() {
        return Ok(Predicate::Position(index));
    }
    if let Some(rest) = inner.strip_prefix("contains(") {
        let rest = rest.strip_suffix(')').unwrap_or(rest);
        let (attribute, needle) = split_call_arguments(rest)?;
        let attribute = attribute
            .trim()
            .strip_prefix('@')
            .ok_or_else(|| format!("contains() needs an attribute: {text:?}"))?
            .to_string();
        return Ok(Predicate::Contains(attribute, unquote(&needle)));
    }
    if let Some(rest) = inner.strip_prefix("starts-with(") {
        let rest = rest.strip_suffix(')').unwrap_or(rest);
        let (attribute, needle) = split_call_arguments(rest)?;
        let attribute = attribute
            .trim()
            .strip_prefix('@')
            .ok_or_else(|| format!("starts-with() needs an attribute: {text:?}"))?
            .to_string();
        return Ok(Predicate::StartsWith(attribute, unquote(&needle)));
    }
    if let Some(attribute) = inner.strip_prefix('@') {
        if let Some((name, value)) = attribute.split_once('=') {
            return Ok(Predicate::Equals(name.trim().to_string(), unquote(value)));
        }
        return Ok(Predicate::Exists(attribute.trim().to_string()));
    }
    // A bare element name is the XPath child test [tag].
    if !inner.is_empty()
        && inner
            .chars()
            .all(|character| character.is_alphanumeric() || "-_:*".contains(character))
    {
        return Ok(Predicate::Child(inner.to_string()));
    }
    Err(format!("unsupported predicate {text:?}"))
}

fn split_call_arguments(text: &str) -> Result<(String, String), String> {
    let mut quote: Option<char> = None;
    // Arguments may nest calls of their own, e.g. substring-after(.//a[contains(@class, "x")]/@href, "://");
    // a comma inside those parentheses does not separate the outer arguments.
    let mut depth = 0usize;
    for (index, character) in text.char_indices() {
        match quote {
            Some(active) if character == active => quote = None,
            Some(_) => {}
            None => match character {
                '\'' | '"' => quote = Some(character),
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    return Ok((text[..index].to_string(), text[index + 1..].to_string()));
                }
                _ => {}
            },
        }
    }
    Err(format!("expected two arguments in {text:?}"))
}

fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(trimmed)
        .to_string()
}

/// Evaluate one of the value functions upstream supports on fields:
/// `concat(...)`, `substring-before(path, "s")`, `substring-after(path, "s")`
/// or a plain path. Returns the first value, like upstream's `query()[0]`.
pub fn evaluate_first(root: &Html, expression: &str) -> Option<String> {
    evaluate_value(root, expression.trim())
}

/// Like `evaluate_first`, but relative to a result element.
pub fn evaluate_in(context: ElementRef<'_>, expression: &str) -> Option<String> {
    let expression = expression.trim();
    if let Some(rest) = expression.strip_prefix("concat(") {
        let rest = rest.strip_suffix(')')?;
        let mut output = String::new();
        let mut remainder = rest;
        loop {
            let (argument, tail) = split_first_argument(remainder)?;
            output.push_str(&evaluate_in(context, &argument).unwrap_or_default());
            if tail.trim().is_empty() {
                break;
            }
            remainder = tail;
        }
        return Some(output);
    }
    if let Some(rest) = expression
        .strip_prefix("substring-after(")
        .or_else(|| expression.strip_prefix("substring-before("))
    {
        let after = expression.starts_with("substring-after(");
        let rest = rest.strip_suffix(')')?;
        let (path, needle) = split_call_arguments(rest).ok()?;
        let value = evaluate_in(context, &path)?;
        let needle = unquote(&needle);
        if after {
            return value.split_once(&needle).map(|(_, tail)| tail.to_string());
        }
        return value.split_once(&needle).map(|(head, _)| head.to_string());
    }
    if expression.len() >= 2 {
        let bytes = expression.as_bytes();
        if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
        {
            return Some(expression[1..expression.len() - 1].to_string());
        }
    }
    let xpath = XPath::parse(expression).ok()?;
    xpath
        .select_in(context)
        .into_iter()
        .map(|node| node.text())
        .find(|value| !value.is_empty())
}

fn evaluate_value(root: &Html, expression: &str) -> Option<String> {
    let expression = expression.trim();
    // A quoted string literal is a value of its own (used inside concat()).
    if expression.len() >= 2 {
        let bytes = expression.as_bytes();
        if (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
        {
            return Some(expression[1..expression.len() - 1].to_string());
        }
    }
    if let Some(rest) = expression.strip_prefix("concat(") {
        let rest = rest.strip_suffix(')')?;
        let mut arguments = Vec::new();
        let mut remainder = rest;
        loop {
            let (argument, tail) = split_first_argument(remainder)?;
            arguments.push(argument);
            if tail.trim().is_empty() {
                break;
            }
            remainder = tail;
        }
        let mut output = String::new();
        for argument in arguments {
            output.push_str(&evaluate_value(root, &argument).unwrap_or_default());
        }
        return Some(output);
    }
    if let Some(rest) = expression
        .strip_prefix("substring-after(")
        .or_else(|| expression.strip_prefix("substring-before("))
    {
        let after = expression.starts_with("substring-after(");
        let rest = rest.strip_suffix(')')?;
        let (path, needle) = split_call_arguments(rest).ok()?;
        let value = evaluate_value(root, &path)?;
        let needle = unquote(&needle);
        if after {
            return value.split_once(&needle).map(|(_, tail)| tail.to_string());
        }
        return value.split_once(&needle).map(|(head, _)| head.to_string());
    }
    let xpath = XPath::parse(expression).ok()?;
    xpath
        .select(root)
        .into_iter()
        .map(|node| node.text())
        .find(|value| !value.is_empty())
}

fn split_first_argument(text: &str) -> Option<(String, &str)> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for (index, character) in text.char_indices() {
        match quote {
            Some(active) if character == active => quote = None,
            Some(_) => {}
            None => match character {
                '\'' | '"' => quote = Some(character),
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                ',' if depth == 0 => {
                    return Some((text[..index].to_string(), &text[index + 1..]));
                }
                _ => {}
            },
        }
    }
    Some((text.to_string(), ""))
}

/// Compile a CSS selector for the fast paths (results containers).
pub fn selector(css: &str) -> Result<Selector, String> {
    Selector::parse(css).map_err(|error| format!("bad selector {css:?}: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> Html {
        Html::parse_document(
            r#"<html><body>
              <div class="result"><h2><a href="/one">First <b>result</b></a></h2><p class="s">Snippet one</p></div>
              <div class="result"><h2><a href="/two">Second</a></h2><p class="s">Snippet two</p></div>
              <div class="other"><a href="https://third.example">Third</a></div>
            </body></html>"#,
        )
    }

    #[test]
    fn selects_elements_and_attributes() {
        let html = document();
        let results = XPath::parse("//div[@class=\"result\"]").unwrap();
        assert_eq!(results.select(&html).len(), 2);

        let links = XPath::parse("//div[@class=\"result\"]/h2/a/@href").unwrap();
        let values: Vec<String> = links.select(&html).iter().map(Node::text).collect();
        assert_eq!(values, vec!["/one", "/two"]);
    }

    #[test]
    fn supports_contains_and_last_and_indexes() {
        let html = document();
        let contains = XPath::parse("//div[contains(@class, \"res\")]/h2/a").unwrap();
        assert_eq!(contains.select(&html).len(), 2);

        let last = XPath::parse("(//div[@class=\"result\"]/h2/a/@href)[last()]").unwrap();
        assert_eq!(
            last.select(&html)
                .iter()
                .map(Node::text)
                .collect::<Vec<_>>(),
            vec!["/two"]
        );

        let first = XPath::parse("//div[@class=\"result\"][1]/h2/a/@href").unwrap();
        assert_eq!(
            first
                .select(&html)
                .iter()
                .map(Node::text)
                .collect::<Vec<_>>(),
            vec!["/one"]
        );
    }

    #[test]
    fn text_terminals_and_relative_paths() {
        let html = document();
        let title = XPath::parse("//div[@class=\"result\"]/h2/a").unwrap();
        let values: Vec<String> = title.select(&html).iter().map(Node::text).collect();
        assert_eq!(values, vec!["First result", "Second"]);

        let results = XPath::parse("//div[@class=\"result\"]").unwrap();
        let Some(Node::Element(first)) = results.select(&html).into_iter().next() else {
            panic!("no element");
        };
        let snippet = XPath::parse("./p[@class=\"s\"]").unwrap();
        assert_eq!(snippet.select_in(first).len(), 1);
    }

    #[test]
    fn value_functions_are_supported() {
        let html = document();
        assert_eq!(
            evaluate_first(
                &html,
                "concat('https://', substring-after(//div[@class=\"other\"]/a/@href, '://'))"
            ),
            Some("https://third.example".to_string())
        );
    }

    #[test]
    fn broken_expressions_are_rejected() {
        assert!(XPath::parse("").is_err());
        assert!(XPath::parse("//div[").is_err());
        assert!(XPath::parse("//div[@class=\"x\"]/").is_err());
    }
}
