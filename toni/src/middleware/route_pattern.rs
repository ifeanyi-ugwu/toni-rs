/// Represents a route pattern with optional HTTP method filtering
#[derive(Debug, Clone)]
pub struct RoutePattern {
    pub path: String,
    pub methods: Option<Vec<String>>,
}

impl RoutePattern {
    pub fn all_methods(path: &str) -> Self {
        Self {
            path: path.to_string(),
            methods: None,
        }
    }

    pub fn single_method(path: &str, method: &str) -> Self {
        Self {
            path: path.to_string(),
            methods: Some(vec![method.to_string()]),
        }
    }

    pub fn methods(path: &str, methods: Vec<&str>) -> Self {
        Self {
            path: path.to_string(),
            // Empty vec means all methods (allows mixing with specific methods in same Vec)
            methods: if methods.is_empty() {
                None
            } else {
                Some(methods.iter().map(|s| s.to_string()).collect())
            },
        }
    }

    pub fn matches(&self, path: &str, method: &str) -> bool {
        let path_matches = if self.path.ends_with('*') {
            let prefix = &self.path[..self.path.len() - 1];
            path.starts_with(prefix)
        } else {
            path == self.path
        };

        if !path_matches {
            return false;
        }

        match &self.methods {
            None => true,
            Some(methods) => methods.iter().any(|m| m.eq_ignore_ascii_case(method)),
        }
    }
}

/// Trait for types that can be converted into a RoutePattern
pub trait IntoRoutePattern {
    fn into_route_pattern(self) -> RoutePattern;
}

// Just a string path (all methods)
impl IntoRoutePattern for &str {
    fn into_route_pattern(self) -> RoutePattern {
        RoutePattern::all_methods(self)
    }
}

// Tuple (path, single method)
impl IntoRoutePattern for (&str, &str) {
    fn into_route_pattern(self) -> RoutePattern {
        RoutePattern::single_method(self.0, self.1)
    }
}

// Tuple (path, multiple methods as Vec)
impl IntoRoutePattern for (&str, Vec<&str>) {
    fn into_route_pattern(self) -> RoutePattern {
        RoutePattern::methods(self.0, self.1)
    }
}

// Tuple (path, multiple methods as slice)
impl IntoRoutePattern for (&str, &[&str]) {
    fn into_route_pattern(self) -> RoutePattern {
        RoutePattern::methods(self.0, self.1.to_vec())
    }
}

// Tuple (path, multiple methods as array) - common case
impl<const N: usize> IntoRoutePattern for (&str, [&str; N]) {
    fn into_route_pattern(self) -> RoutePattern {
        RoutePattern::methods(self.0, self.1.to_vec())
    }
}

// Tuple (path, reference to array) - allows &["GET", "POST"] syntax
impl<const N: usize> IntoRoutePattern for (&str, &[&str; N]) {
    fn into_route_pattern(self) -> RoutePattern {
        RoutePattern::methods(self.0, self.1.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_into_route_pattern_string() {
        let pattern: RoutePattern = "/users/*".into_route_pattern();
        assert_eq!(pattern.path, "/users/*");
        assert!(pattern.methods.is_none());
    }

    #[test]
    fn test_into_route_pattern_tuple_single() {
        let pattern: RoutePattern = ("/users/*", "POST").into_route_pattern();
        assert_eq!(pattern.path, "/users/*");
        assert_eq!(pattern.methods, Some(vec!["POST".to_string()]));
    }

    #[test]
    fn test_into_route_pattern_tuple_vec() {
        let pattern: RoutePattern = ("/users/*", vec!["POST", "PUT"]).into_route_pattern();
        assert_eq!(pattern.path, "/users/*");
        assert_eq!(
            pattern.methods,
            Some(vec!["POST".to_string(), "PUT".to_string()])
        );
    }

    #[test]
    fn test_into_route_pattern_tuple_array() {
        let pattern: RoutePattern = ("/users/*", ["POST", "PUT", "DELETE"]).into_route_pattern();
        assert_eq!(pattern.path, "/users/*");
        assert_eq!(
            pattern.methods,
            Some(vec![
                "POST".to_string(),
                "PUT".to_string(),
                "DELETE".to_string()
            ])
        );
    }
}
