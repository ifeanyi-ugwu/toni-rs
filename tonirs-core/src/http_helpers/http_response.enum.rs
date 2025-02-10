use super::Body;

#[derive(Debug)]
pub struct HttpResponseDefault {
    pub body: Option<Body>,
    pub status: Option<u16>
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub body: Option<Body>,
    pub status: u16,
    pub headers: Vec<(String, String)>,
}
impl HttpResponse {
    pub fn new() -> Self {
        Self { 
            body: None,
            status: 200,
            headers: vec![],
        }
    }
}
