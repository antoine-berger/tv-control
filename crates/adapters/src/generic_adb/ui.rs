//! Strict bounded parser for UI Automator output; no remote text is executable.
use super::{MAX_UI_ELEMENTS, MAX_UI_NODES};
use std::{
    collections::{hash_map::DefaultHasher, BTreeMap},
    hash::{Hash, Hasher},
};
use tv_domain::{
    generic::{UiElement, UiSnapshot},
    Failure, PortResult,
};
pub(super) fn parse_ui(device: &str, session: u128, raw: &str) -> PortResult<UiSnapshot> {
    let invalid = || Failure::before("ui_unavailable");
    if raw.len() > 512 * 1024 || raw.contains("<!") {
        return Err(invalid());
    }
    let start = raw.find("<hierarchy").ok_or_else(invalid)?;
    let end = raw.rfind("</hierarchy>").ok_or_else(invalid)? + "</hierarchy>".len();
    let xml = raw.get(start..end).ok_or_else(invalid)?;
    let mut hash = DefaultHasher::new();
    device.hash(&mut hash);
    session.hash(&mut hash);
    xml.hash(&mut hash);
    let revision = format!("{:016x}", hash.finish());
    let mut elements = Vec::new();
    let mut rest = xml;
    let mut stack: Vec<&str> = Vec::new();
    let mut nodes = 0usize;
    while !rest.is_empty() {
        if !rest.starts_with('<') {
            return Err(invalid());
        }
        let close = tag_end(rest).ok_or_else(invalid)?;
        let tag = &rest[1..close];
        rest = rest[close + 1..].trim_start();
        if let Some(name) = tag.strip_prefix('/') {
            if stack.pop() != Some(name.trim()) {
                return Err(invalid());
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let tag = tag.strip_suffix('/').unwrap_or(tag).trim_end();
        let split = tag.find(char::is_whitespace).unwrap_or(tag.len());
        let name = &tag[..split];
        if !matches!(name, "hierarchy" | "node")
            || (name == "hierarchy" && (!stack.is_empty() || !elements.is_empty()))
        {
            return Err(invalid());
        }
        if name == "node" && stack.is_empty() {
            return Err(invalid());
        }
        let attributes = attributes(&tag[split..]).ok_or_else(invalid)?;
        if !self_closing {
            stack.push(name);
        }
        if stack.len() > 64 {
            return Err(Failure::before("ui_output_limit"));
        }
        if name != "node" {
            continue;
        }
        nodes += 1;
        if nodes > MAX_UI_NODES {
            return Err(Failure::before("ui_output_limit"));
        }
        let flag = |key: &str| attributes.get(key).is_some_and(|v| v == "true");
        if !flag("enabled")
            || flag("password")
            || !(flag("clickable") || flag("focusable") || flag("focused"))
        {
            continue;
        }
        let bounds =
            parse_bounds(attributes.get("bounds").ok_or_else(invalid)?).ok_or_else(invalid)?;
        // Off-screen or clipped nodes can remain enabled/focusable in Android.
        // They cannot be safely tapped, but must not invalidate visible siblings.
        if bounds[0] >= bounds[2] || bounds[1] >= bounds[3] {
            continue;
        }
        let text = attributes
            .get("content-desc")
            .filter(|s| !s.is_empty())
            .or_else(|| attributes.get("text"))
            .cloned()
            .unwrap_or_default();
        let text: String = text.chars().filter(|c| !c.is_control()).take(256).collect();
        let role = attributes
            .get("class")
            .and_then(|s| s.rsplit('.').next())
            .unwrap_or("View")
            .chars()
            .take(64)
            .collect();
        if elements.len() == MAX_UI_ELEMENTS {
            return Err(Failure::before("ui_output_limit"));
        }
        elements.push(UiElement {
            reference: format!("{revision}:{}", elements.len()),
            text,
            role,
            focused: flag("focused"),
            bounds,
        });
    }
    if !stack.is_empty() {
        return Err(invalid());
    }
    Ok(UiSnapshot { revision, elements })
}
fn tag_end(s: &str) -> Option<usize> {
    let mut quote = None;
    for (index, ch) in s.char_indices().skip(1) {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if ch == '>' {
            return Some(index);
        } else if ch == '<' {
            return None;
        }
    }
    None
}
fn attributes(mut s: &str) -> Option<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    while !s.trim().is_empty() {
        s = s.trim_start();
        let equal = s.find('=')?;
        let key = s[..equal].trim();
        if key.is_empty()
            || !key
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"_-:".contains(&c))
        {
            return None;
        }
        s = s[equal + 1..].trim_start();
        let quote = s.chars().next()?;
        if !matches!(quote, '\'' | '"') {
            return None;
        }
        s = &s[1..];
        let end = s.find(quote)?;
        if s[..end].contains('<') {
            return None;
        }
        let value = unescape(&s[..end])?;
        if result.insert(key.to_owned(), value).is_some() {
            return None;
        }
        s = &s[end + 1..];
        if !s.is_empty() && !s.starts_with(char::is_whitespace) {
            return None;
        }
    }
    Some(result)
}
fn unescape(mut s: &str) -> Option<String> {
    let mut result = String::new();
    while let Some(start) = s.find('&') {
        result.push_str(&s[..start]);
        s = &s[start + 1..];
        let end = s.find(';')?;
        let value = &s[..end];
        let ch = match value {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            _ => {
                let number = if let Some(value) = value.strip_prefix("#x") {
                    u32::from_str_radix(value, 16).ok()?
                } else {
                    value.strip_prefix('#')?.parse().ok()?
                };
                char::from_u32(number)?
            }
        };
        result.push(ch);
        s = &s[end + 1..];
    }
    result.push_str(s);
    Some(result)
}
fn parse_bounds(raw: &str) -> Option<[u32; 4]> {
    let (first, second) = raw.strip_prefix('[')?.strip_suffix(']')?.split_once("][")?;
    let (left, top) = first.split_once(',')?;
    let (right, bottom) = second.split_once(',')?;
    let rect = [
        left.parse().ok()?,
        top.parse().ok()?,
        right.parse().ok()?,
        bottom.parse().ok()?,
    ];
    rect.iter().all(|v| *v <= 65535).then_some(rect)
}
