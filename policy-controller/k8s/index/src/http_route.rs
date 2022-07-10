use anyhow::Result;
use k8s_gateway_api as api;
use linkerd_policy_controller_core::http_route::{
    HeaderMatch, Hostname, HttpRoute, HttpRouteMatch, Method, PathMatch, QueryParamMatch, Value,
};

#[derive(Clone, Debug, PartialEq)]
pub struct RouteBinding {
    pub route: HttpRoute,
    pub parent_refs: Vec<api::ParentReference>,
}

impl RouteBinding {
    pub fn from_resource(route: api::HttpRoute) -> Result<Self> {
        let hostnames = route
            .spec
            .hostnames
            .iter()
            .flatten()
            .map(|hostname| {
                if hostname.starts_with("*.") {
                    let mut reverse_labels = hostname
                        .split('.')
                        .skip(1)
                        .map(ToString::to_string)
                        .collect::<Vec<_>>();
                    reverse_labels.reverse();
                    Hostname::Suffix { reverse_labels }
                } else {
                    Hostname::Exact(hostname.to_owned())
                }
            })
            .collect();

        let matches = route
            .spec
            .rules
            .into_iter()
            .flatten()
            .flat_map(|rule| rule.matches.into_iter().flatten())
            .map(
                |api::HttpRouteMatch {
                     path,
                     headers,
                     query_params,
                     method,
                 }| {
                    let path = path
                        .map(|path_match| match path_match {
                            api::HttpPathMatch::Exact { value } => Ok(PathMatch::Exact(value)),
                            api::HttpPathMatch::PathPrefix { value } => {
                                Ok(PathMatch::Prefix(value))
                            }
                            api::HttpPathMatch::RegularExpression { value } => {
                                PathMatch::try_regex(&*value)
                            }
                        })
                        .transpose()?;

                    let headers = headers
                        .into_iter()
                        .flatten()
                        .map(|header_match| match header_match {
                            api::HttpHeaderMatch::Exact { name, value } => Ok(HeaderMatch {
                                name,
                                value: Value::Exact(value),
                            }),
                            api::HttpHeaderMatch::RegularExpression { name, value } => {
                                Ok(HeaderMatch {
                                    name,
                                    value: Value::try_regex(&*value)?,
                                })
                            }
                        })
                        .collect::<Result<Vec<_>>>()?;

                    let query_params = query_params
                        .into_iter()
                        .flatten()
                        .map(|query_param| match query_param {
                            k8s_gateway_api::HttpQueryParamMatch::Exact { name, value } => {
                                Ok(QueryParamMatch {
                                    name,
                                    value: Value::Exact(value),
                                })
                            }
                            k8s_gateway_api::HttpQueryParamMatch::RegularExpression {
                                name,
                                value,
                            } => Ok(QueryParamMatch {
                                name,
                                value: Value::try_regex(&*value)?,
                            }),
                        })
                        .collect::<Result<Vec<_>>>()?;

                    let method = method.map(|m| m.parse::<Method>()).transpose()?;

                    Ok(HttpRouteMatch {
                        path,
                        headers,
                        query_params,
                        method,
                    })
                },
            )
            .collect::<Result<Vec<_>>>()?;

        Ok(RouteBinding {
            route: HttpRoute { hostnames, matches },
            parent_refs: route.spec.inner.parent_refs.unwrap_or_default(),
        })
    }

    pub fn selects_server(&self, name: &str) -> bool {
        for parent_ref in self.parent_refs.iter() {
            if parent_ref.group.as_deref() == Some("policy.linkerd.io")
                && parent_ref.kind.as_deref() == Some("Server")
                && parent_ref.name == name
            {
                return true;
            }
        }
        false
    }
}
