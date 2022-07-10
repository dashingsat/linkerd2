pub use http::Method;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HttpRoute {
    pub hostnames: Vec<Hostname>,
    pub matches: Vec<HttpRouteMatch>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Hostname {
    Exact(String),
    Suffix { reverse_labels: Vec<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HttpRouteMatch {
    pub path: Option<PathMatch>,
    pub headers: Vec<HeaderMatch>,
    pub query_params: Vec<QueryParamMatch>,
    pub method: Option<Method>,
}

#[derive(Clone, Debug)]
pub enum PathMatch {
    Exact(String),
    Prefix(String),
    Regex(regex::Regex),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HeaderMatch {
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QueryParamMatch {
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Debug)]
pub enum Value {
    Exact(String),
    Regex(regex::Regex),
}

// === impl PathMatch ===

impl PathMatch {
    #[inline]
    pub fn try_regex(v: &str) -> Result<Self, regex::Error> {
        regex::Regex::new(v).map(Self::Regex)
    }
}

impl PartialEq for PathMatch {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Prefix(a), Self::Prefix(b)) => a == b,
            (Self::Regex(a), Self::Regex(b)) => a.as_str() == b.as_str(),
            _ => false,
        }
    }
}

impl Eq for PathMatch {}

impl std::hash::Hash for PathMatch {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Exact(s) => s.hash(state),
            Self::Prefix(s) => s.hash(state),
            Self::Regex(r) => r.as_str().hash(state),
        }
    }
}

// === impl Value ===

impl Value {
    #[inline]
    pub fn try_regex(v: &str) -> Result<Self, regex::Error> {
        regex::Regex::new(v).map(Self::Regex)
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Exact(a), Self::Exact(b)) => a == b,
            (Self::Regex(a), Self::Regex(b)) => a.as_str() == b.as_str(),
            _ => false,
        }
    }
}

impl Eq for Value {}

impl std::hash::Hash for Value {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            Self::Exact(s) => s.hash(state),
            Self::Regex(r) => r.as_str().hash(state),
        }
    }
}
