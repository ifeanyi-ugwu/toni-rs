use proc_macro2::Ident;

pub fn snake_to_upper(snake: &Ident) -> String {
    snake.to_string().split('_').map(|s| s.chars().next().unwrap().to_uppercase().to_string() + &s[1..]).collect::<Vec<String>>().join("")
}

pub fn upper_to_snake(upper: &str) -> String {
    let mut result = String::new();
    for c in upper.to_string().chars() {
        if c.is_uppercase() {
            result.push('_');
            result.push(c.to_lowercase().next().unwrap());
        } else {
            result.push(c);
        }
    }
    result
}