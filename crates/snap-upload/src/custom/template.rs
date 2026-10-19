//! ShareX custom-uploader syntax: `{filename}`, `{input}`, `{random:a|b}`,
//! `{base64}` on the request side and `{json:a.b[0].c}`, `{xml:/a/b}`,
//! `{regex:pattern|group}`, `{response}`, `{responseurl}`, `{header:name}` on
//! the response side. Unknown `{...}` runs (such as JSON braces) stay literal.

use std::cell::OnceCell;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand::Rng;
use regex::Regex;
use serde_json::Value;
use snap_core::naming::{self, NameContext};

use crate::util::{pct, split_ext};

/// Values available when building the request.
pub(crate) struct Vars<'a> {
    pub file_name: &'a str,
    pub input: String,
    pub mime: &'a str,
    pub title: &'a str,
    pub secret: &'a str,
    pub file: &'a [u8],
}

/// The server's answer, for URL / error-message templates.
pub(crate) struct ResponseView<'a> {
    pub text: &'a str,
    pub url: &'a str,
    pub headers: &'a [(String, String)],
    json: OnceCell<Option<Value>>,
}

impl<'a> ResponseView<'a> {
    pub fn new(text: &'a str, url: &'a str, headers: &'a [(String, String)]) -> Self {
        Self { text, url, headers, json: OnceCell::new() }
    }

    fn json(&self) -> Option<&Value> {
        self.json.get_or_init(|| serde_json::from_str(self.text.trim()).ok()).as_ref()
    }
}

/// How substituted values are escaped for the surrounding body format.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Escape {
    None,
    Json,
    Xml,
}

impl Escape {
    fn apply(self, s: String) -> String {
        match self {
            Escape::None => s,
            Escape::Json => {
                let quoted = Value::String(s).to_string();
                quoted[1..quoted.len() - 1].to_string()
            }
            Escape::Xml => crate::util::html_escape(&s),
        }
    }
}

pub(crate) fn expand(template: &str, vars: &Vars, resp: Option<&ResponseView>, escape: Escape) -> String {
    let mut out = String::with_capacity(template.len());
    let mut i = 0;
    while i < template.len() {
        let rest = &template[i..];
        let Some(open) = rest.find('{') else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..open]);
        let start = i + open;
        if let Some(close) = matching_brace(template, start)
            && let Some(value) = eval(&template[start + 1..close], vars, resp)
        {
            out.push_str(&escape.apply(value));
            i = close + 1;
            continue;
        }
        out.push('{');
        i = start + 1;
    }
    out
}

/// Byte index of the `}` matching the `{` at `open`, honouring nesting.
fn matching_brace(s: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (off, c) in s[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + off);
                }
            }
            _ => {}
        }
    }
    None
}

fn eval(inner: &str, vars: &Vars, resp: Option<&ResponseView>) -> Option<String> {
    let (name, arg) = match inner.split_once(':') {
        Some((n, a)) => (n.trim(), Some(a)),
        None => (inner.trim(), None),
    };
    let lower = name.to_ascii_lowercase();
    let text = |s: &str| Some(s.to_string());
    match lower.as_str() {
        "filename" => text(vars.file_name),
        "filenamenoext" => text(split_ext(vars.file_name).0),
        "ext" => text(split_ext(vars.file_name).1.trim_start_matches('.')),
        "input" => text(&vars.input),
        "mime" => text(vars.mime),
        "title" => text(vars.title),
        "secret" => text(vars.secret),
        "guid" => Some(guid()),
        "random" => Some(match arg {
            Some(a) => {
                let options: Vec<&str> = a.split('|').collect();
                options[rand::rng().random_range(0..options.len())].to_string()
            }
            None => naming::random_code(8),
        }),
        "base64" => Some(match arg {
            Some(a) => STANDARD.encode(a.as_bytes()),
            None => STANDARD.encode(vars.file),
        }),
        "urlencode" => arg.map(pct),
        "response" => resp.map(|r| r.text.to_string()),
        "responseurl" => resp.map(|r| r.url.to_string()),
        "header" => {
            let r = resp?;
            let wanted = arg?.trim();
            Some(r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(wanted)).map(|(_, v)| v.clone()).unwrap_or_default())
        }
        "json" => {
            let r = resp?;
            Some(r.json().and_then(|j| json_select(j, arg?)).unwrap_or_default())
        }
        "xml" => {
            let r = resp?;
            Some(xml_select(r.text, arg?).unwrap_or_default())
        }
        "regex" => {
            let r = resp?;
            Some(regex_select(r.text, arg?).unwrap_or_default())
        }
        _ if lower.chars().all(|c| c.is_ascii_alphanumeric()) => {
            // Date and counter tokens shared with file naming.
            let token = format!("{{{inner}}}");
            let expanded = naming::expand(&token, &NameContext::now());
            (expanded != token).then_some(expanded)
        }
        _ => None,
    }
}

fn guid() -> String {
    let mut b: [u8; 16] = rand::rng().random();
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

// -- JSON path ---------------------------------------------------------------

enum Seg {
    Key(String),
    Index(usize),
}

fn parse_json_path(path: &str) -> Vec<Seg> {
    let path = path.trim().trim_start_matches('$');
    let mut segs = Vec::new();
    let chars: Vec<char> = path.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '.' => i += 1,
            '[' => {
                let end = chars[i..].iter().position(|c| *c == ']').map_or(chars.len(), |p| i + p);
                let inner: String = chars[i + 1..end].iter().collect();
                let inner = inner.trim();
                if let Ok(n) = inner.parse::<usize>() {
                    segs.push(Seg::Index(n));
                } else if inner == "*" {
                    segs.push(Seg::Index(0));
                } else {
                    segs.push(Seg::Key(inner.trim_matches(['\'', '"']).to_string()));
                }
                i = end + 1;
            }
            _ => {
                let end = chars[i..].iter().position(|c| *c == '.' || *c == '[').map_or(chars.len(), |p| i + p);
                segs.push(Seg::Key(chars[i..end].iter().collect()));
                i = end;
            }
        }
    }
    segs
}

/// Evaluate a dotted / bracketed JSON path (`a.b[0].c`, `$.a['b']`) to text.
pub(crate) fn json_select(root: &Value, path: &str) -> Option<String> {
    let mut cur = root;
    for seg in parse_json_path(path) {
        cur = match (seg, cur) {
            (Seg::Key(k), Value::Object(o)) => o.get(&k)?,
            (Seg::Key(k), Value::Array(a)) => a.get(k.parse::<usize>().ok()?)?,
            (Seg::Index(n), Value::Array(a)) => a.get(n)?,
            _ => return None,
        };
    }
    Some(match cur {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

// -- XPath subset ------------------------------------------------------------

struct Step {
    descendant: bool,
    test: Test,
    preds: Vec<Pred>,
}

enum Test {
    Name(String),
    Any,
    Text,
    Attr(String),
}

enum Pred {
    Index(usize),
    AttrExists(String),
    AttrEq(String, String),
    ChildEq(String, String),
}

fn parse_xpath(path: &str) -> Vec<Step> {
    let mut steps = Vec::new();
    let mut descendant = false;
    for part in split_top_level(path.trim(), '/') {
        if part.is_empty() {
            // An empty segment from `//` marks the descendant axis for the next step.
            descendant = true;
            continue;
        }
        let (head, preds) = match part.find('[') {
            Some(p) => (&part[..p], parse_preds(&part[p..])),
            None => (part, Vec::new()),
        };
        let test = if head == "*" {
            Test::Any
        } else if head == "text()" {
            Test::Text
        } else if let Some(a) = head.strip_prefix('@') {
            Test::Attr(a.to_string())
        } else {
            Test::Name(head.to_string())
        };
        steps.push(Step { descendant, test, preds });
        descendant = false;
    }
    steps
}

/// Split on `sep` outside of `[...]` and quotes.
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut quote, mut start) = (0i32, None::<char>, 0);
    for (i, c) in s.char_indices() {
        match (c, quote) {
            ('\'' | '"', None) => quote = Some(c),
            (q, Some(open)) if q == open => quote = None,
            ('[', None) => depth += 1,
            (']', None) => depth -= 1,
            (c, None) if c == sep && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    // A leading `/` yields an empty first part that only means "absolute".
    if parts.first() == Some(&"") && parts.get(1).is_some_and(|p| !p.is_empty()) {
        parts.remove(0);
    }
    parts
}

fn parse_preds(s: &str) -> Vec<Pred> {
    let mut preds = Vec::new();
    for raw in s.split(']').filter_map(|p| p.strip_prefix('[')) {
        let raw = raw.trim();
        if let Ok(n) = raw.parse::<usize>() {
            preds.push(Pred::Index(n));
        } else if let Some((lhs, rhs)) = raw.split_once('=') {
            let value = rhs.trim().trim_matches(['\'', '"']).to_string();
            match lhs.trim().strip_prefix('@') {
                Some(attr) => preds.push(Pred::AttrEq(attr.to_string(), value)),
                None => preds.push(Pred::ChildEq(lhs.trim().to_string(), value)),
            }
        } else if let Some(attr) = raw.strip_prefix('@') {
            preds.push(Pred::AttrExists(attr.to_string()));
        }
    }
    preds
}

fn node_text(node: roxmltree::Node) -> String {
    node.descendants().filter(|n| n.is_text()).filter_map(|n| n.text()).collect()
}

fn matches_pred(node: roxmltree::Node, pred: &Pred) -> bool {
    match pred {
        Pred::Index(_) => true,
        Pred::AttrExists(a) => node.attribute(a.as_str()).is_some(),
        Pred::AttrEq(a, v) => node.attribute(a.as_str()) == Some(v.as_str()),
        Pred::ChildEq(name, v) => node.children().any(|c| c.is_element() && c.tag_name().name() == name && node_text(c).trim() == v),
    }
}

fn eval_xpath<'a, 'i>(context: roxmltree::Node<'a, 'i>, steps: &[Step]) -> Vec<roxmltree::Node<'a, 'i>> {
    let mut current = vec![context];
    for step in steps {
        let mut next = Vec::new();
        for ctx in &current {
            let mut candidates: Vec<roxmltree::Node> = match (&step.test, step.descendant) {
                (Test::Attr(_), _) | (Test::Text, _) => vec![*ctx],
                (_, true) => ctx.descendants().skip(1).filter(|n| n.is_element()).collect(),
                (_, false) => ctx.children().filter(|n| n.is_element()).collect(),
            };
            if let Test::Name(name) = &step.test {
                candidates.retain(|n| n.tag_name().name() == name);
            }
            candidates.retain(|n| step.preds.iter().all(|p| matches_pred(*n, p)));
            for p in &step.preds {
                if let Pred::Index(i) = p {
                    candidates = candidates.get(i.saturating_sub(1)).copied().into_iter().collect();
                }
            }
            next.extend(candidates);
        }
        current = next;
    }
    current
}

/// Evaluate an XPath subset (`/a/b`, `//c`, `a[2]`, `x[@id='1']`, `@attr`, `text()`).
pub(crate) fn xml_select(xml: &str, path: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml.trim()).ok()?;
    let steps = parse_xpath(path);
    let last = steps.last()?;
    let value = |nodes: Vec<roxmltree::Node>| -> Option<String> {
        let first = nodes.first()?;
        Some(match &last.test {
            Test::Attr(a) => first.attribute(a.as_str())?.to_string(),
            _ => node_text(*first),
        })
    };
    // Absolute from the document, else relative to the root element.
    value(eval_xpath(doc.root(), &steps)).or_else(|| value(eval_xpath(doc.root_element(), &steps)))
}

// -- Regex ---------------------------------------------------------------------

/// `pattern|group` where `group` is a number or a name; defaults to group 1
/// (or the whole match when the pattern has no groups).
pub(crate) fn regex_select(text: &str, spec: &str) -> Option<String> {
    let (pattern, group) = match spec.rsplit_once('|') {
        Some((p, g)) if !p.is_empty() && !g.is_empty() && g.chars().all(|c| c.is_alphanumeric() || c == '_') && Regex::new(p).is_ok() => {
            (p, Some(g))
        }
        _ => (spec, None),
    };
    let re = Regex::new(pattern).ok()?;
    let caps = re.captures(text)?;
    let m = match group {
        Some(g) => match g.parse::<usize>() {
            Ok(n) => caps.get(n),
            Err(_) => caps.name(g),
        },
        None if re.captures_len() > 1 => caps.get(1),
        None => caps.get(0),
    }?;
    Some(m.as_str().to_string())
}
