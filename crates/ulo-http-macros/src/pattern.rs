//! The route-pattern grammar, checked at compile time on the attribute's literal. The same
//! grammar as `ulo_http::router::pattern`, which parses it again at `prepare`; the two are kept in
//! step by hand, since this crate cannot depend on the crate that re-exports it, and give the same
//! reasons.

/// Why `text` is not a route pattern, or `Ok`: a leading `/`; segments of static text, `{name}`,
/// or a final `{*name}`; no brace elsewhere; no empty segment; no name twice. A name is
/// non-empty, starts with `_` or a letter, and continues with `_`, letters and digits. One
/// trailing slash is insignificant, so `/files/{*rest}/` ends in its rest segment.
pub(crate) fn check(text: &str) -> Result<(), &'static str> {
    if !text.starts_with('/') {
        return Err("a pattern starts with `/`");
    }
    let normalized = match text.strip_suffix('/') {
        Some(stripped) if !stripped.is_empty() => stripped,
        _ => text,
    };
    if normalized == "/" {
        return Ok(());
    }
    let pieces: Vec<&str> = normalized[1..].split('/').collect();
    let last = pieces.len() - 1;
    let mut names: Vec<&str> = Vec::new();
    for (index, piece) in pieces.into_iter().enumerate() {
        if piece.is_empty() {
            return Err("a segment between two `/` is empty");
        }
        if let Some(inner) = piece.strip_prefix('{') {
            let Some(inner) = inner.strip_suffix('}') else {
                return Err("a `{` opens a segment that does not end with `}`");
            };
            let name = match inner.strip_prefix('*') {
                Some(name) => {
                    check_name(name)?;
                    if index != last {
                        return Err("`{*name}` is the last segment only");
                    }
                    name
                }
                None => {
                    check_name(inner)?;
                    inner
                }
            };
            if names.contains(&name) {
                return Err("a parameter name appears twice");
            }
            names.push(name);
        } else if piece.contains(['{', '}']) {
            return Err("a brace appears outside a whole `{name}` segment");
        }
    }
    Ok(())
}

fn check_name(name: &str) -> Result<(), &'static str> {
    let mut chars = name.chars();
    match chars.next() {
        None => Err("a parameter name is empty"),
        Some(first) if first != '_' && !first.is_alphabetic() => Err("a parameter name starts with `_` or a letter"),
        Some(_) if chars.any(|c| c != '_' && !c.is_alphanumeric()) => {
            Err("a parameter name holds only `_`, letters and digits")
        }
        Some(_) => Ok(()),
    }
}
