use std::collections::HashMap;

use super::{Body, Extensions};

#[derive(Clone, Debug)]
pub struct HttpRequest {
    pub body: Body,
    pub headers: Vec<(String, String)>,
    pub method: String,
    pub uri: String,
    pub query_params: HashMap<String, String>,
    pub path_params: HashMap<String, String>,
    pub extensions: Extensions,
}
