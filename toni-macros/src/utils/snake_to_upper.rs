use proc_macro2::{Ident, Span};
use syn::{Error, Result};

pub fn snake_to_upper(snake: &Ident) -> Result<String> {
    let mut snake_str = snake.to_string();
    let mut result = String::new();
    if snake_str.starts_with('_') {
        snake_str = snake_str
            .strip_prefix('_')
            .ok_or_else(|| Error::new(snake.span(), "Failed to strip prefix"))?
            .to_owned();
    }
    for segment in snake_str.split('_') {
        if segment.is_empty() {
            return Err(Error::new(
                snake.span(),
                "Empty segment found in snake case identifier",
            ));
        }
        let mut chars = segment.chars();
        let first_char = chars
            .next()
            .ok_or_else(|| Error::new(snake.span(), "Segment has no characters"))?;
        let upper_first: String = first_char.to_uppercase().collect();
        result.push_str(&upper_first);
        result.push_str(chars.as_str());
    }
    Ok(result)
}

pub fn upper_to_snake(upper: &str) -> Result<String> {
    let mut result = String::new();
    for c in upper.chars() {
        if c.is_uppercase() {
            result.push('_');
            let lower = c.to_lowercase().next().ok_or_else(|| {
                Error::new(
                    Span::call_site(),
                    "Failed to convert uppercase to lowercase",
                )
            })?;
            result.push(lower);
        } else {
            result.push(c);
        }
    }
    Ok(result)
}
