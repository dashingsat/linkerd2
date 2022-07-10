#![deny(warnings, rust_2018_idioms)]
#![forbid(unsafe_code)]

use futures::prelude::*;
use linkerd2_proxy_api::inbound::{
    self as proto,
    inbound_server_policies_server::{InboundServerPolicies, InboundServerPoliciesServer},
};
use linkerd_policy_controller_core::{
    http_route::{HeaderMatch, Hostname, HttpRouteMatch, PathMatch, QueryParamMatch, Value},
    AuthorizationRef, ClientAuthentication, ClientAuthorization, DiscoverInboundServer, HttpRoute,
    IdentityMatch, InboundServer, InboundServerStream, IpNet, NetworkMatch, ProxyProtocol,
    ServerRef,
};
use maplit::*;
use std::sync::Arc;
use tracing::trace;

#[derive(Clone, Debug)]
pub struct Server<T> {
    discover: T,
    drain: drain::Watch,
    cluster_networks: Arc<[IpNet]>,
}

// === impl Server ===

impl<T> Server<T>
where
    T: DiscoverInboundServer<(String, String, u16)> + Send + Sync + 'static,
{
    pub fn new(discover: T, cluster_networks: Vec<IpNet>, drain: drain::Watch) -> Self {
        Self {
            discover,
            drain,
            cluster_networks: cluster_networks.into(),
        }
    }

    pub async fn serve(
        self,
        addr: std::net::SocketAddr,
        shutdown: impl std::future::Future<Output = ()>,
    ) -> hyper::Result<()> {
        let svc = InboundServerPoliciesServer::new(self);
        hyper::Server::bind(&addr)
            .http2_only(true)
            .tcp_nodelay(true)
            .serve(hyper::service::make_service_fn(move |_| {
                future::ok::<_, std::convert::Infallible>(svc.clone())
            }))
            .with_graceful_shutdown(shutdown)
            .await
    }

    fn check_target(
        &self,
        proto::PortSpec { workload, port }: proto::PortSpec,
    ) -> Result<(String, String, u16), tonic::Status> {
        // Parse a workload name in the form namespace:name.
        let (ns, name) = match workload.split_once(':') {
            None => {
                return Err(tonic::Status::invalid_argument(format!(
                    "Invalid workload: {}",
                    workload
                )));
            }
            Some((ns, pod)) if ns.is_empty() || pod.is_empty() => {
                return Err(tonic::Status::invalid_argument(format!(
                    "Invalid workload: {}",
                    workload
                )));
            }
            Some((ns, pod)) => (ns, pod),
        };

        // Ensure that the port is in the valid range.
        let port = {
            if port == 0 || port > std::u16::MAX as u32 {
                return Err(tonic::Status::invalid_argument(format!(
                    "Invalid port: {}",
                    port
                )));
            }
            port as u16
        };

        Ok((ns.to_string(), name.to_string(), port))
    }
}

#[async_trait::async_trait]
impl<T> InboundServerPolicies for Server<T>
where
    T: DiscoverInboundServer<(String, String, u16)> + Send + Sync + 'static,
{
    async fn get_port(
        &self,
        req: tonic::Request<proto::PortSpec>,
    ) -> Result<tonic::Response<proto::Server>, tonic::Status> {
        let target = self.check_target(req.into_inner())?;

        // Lookup the configuration for an inbound port. If the pod hasn't (yet)
        // been indexed, return a Not Found error.
        let s = self
            .discover
            .get_inbound_server(target)
            .await
            .map_err(|e| tonic::Status::internal(format!("lookup failed: {}", e)))?
            .ok_or_else(|| tonic::Status::not_found("unknown server"))?;

        Ok(tonic::Response::new(to_server(&s, &*self.cluster_networks)))
    }

    type WatchPortStream = BoxWatchStream;

    async fn watch_port(
        &self,
        req: tonic::Request<proto::PortSpec>,
    ) -> Result<tonic::Response<BoxWatchStream>, tonic::Status> {
        let target = self.check_target(req.into_inner())?;
        let drain = self.drain.clone();
        let rx = self
            .discover
            .watch_inbound_server(target)
            .await
            .map_err(|e| tonic::Status::internal(format!("lookup failed: {}", e)))?
            .ok_or_else(|| tonic::Status::not_found("unknown server"))?;
        Ok(tonic::Response::new(response_stream(
            drain,
            rx,
            self.cluster_networks.clone(),
        )))
    }
}

type BoxWatchStream =
    std::pin::Pin<Box<dyn Stream<Item = Result<proto::Server, tonic::Status>> + Send + Sync>>;

fn response_stream(
    drain: drain::Watch,
    mut rx: InboundServerStream,
    cluster_networks: Arc<[IpNet]>,
) -> BoxWatchStream {
    Box::pin(async_stream::try_stream! {
        tokio::pin! {
            let shutdown = drain.signaled();
        }

        loop {
            tokio::select! {
                // When the port is updated with a new server, update the server watch.
                res = rx.next() => match res {
                    Some(s) => {
                        yield to_server(&s, &*cluster_networks);
                    }
                    None => return,
                },

                // If the server starts shutting down, close the stream so that it doesn't hold the
                // server open.
                _ = (&mut shutdown) => {
                    return;
                }
            }
        }
    })
}

fn to_server(srv: &InboundServer, cluster_networks: &[IpNet]) -> proto::Server {
    // Convert the protocol object into a protobuf response.
    let protocol = proto::ProxyProtocol {
        kind: match srv.protocol {
            ProxyProtocol::Detect { timeout } => Some(proto::proxy_protocol::Kind::Detect(
                proto::proxy_protocol::Detect {
                    timeout: Some(timeout.into()),
                    http_routes: srv
                        .http_routes
                        .iter()
                        .map(|(name, route)| to_http_route(name, route, srv, cluster_networks))
                        .collect(),
                },
            )),
            ProxyProtocol::Http1 => Some(proto::proxy_protocol::Kind::Http1(
                proto::proxy_protocol::Http1 {
                    routes: srv
                        .http_routes
                        .iter()
                        .map(|(name, route)| to_http_route(name, route, srv, cluster_networks))
                        .collect(),
                },
            )),
            ProxyProtocol::Http2 => Some(proto::proxy_protocol::Kind::Http2(
                proto::proxy_protocol::Http2 {
                    routes: srv
                        .http_routes
                        .iter()
                        .map(|(name, route)| to_http_route(name, route, srv, cluster_networks))
                        .collect(),
                },
            )),
            ProxyProtocol::Grpc => Some(proto::proxy_protocol::Kind::Grpc(
                proto::proxy_protocol::Grpc::default(),
            )),
            ProxyProtocol::Opaque => Some(proto::proxy_protocol::Kind::Opaque(
                proto::proxy_protocol::Opaque {},
            )),
            ProxyProtocol::Tls => Some(proto::proxy_protocol::Kind::Tls(
                proto::proxy_protocol::Tls {},
            )),
        },
    };
    trace!(?protocol);

    let authorizations = srv
        .authorizations
        .iter()
        .map(|(n, c)| to_authz(n, c, cluster_networks))
        .collect();
    trace!(?authorizations);

    let labels = match &srv.reference {
        ServerRef::Default(name) => convert_args!(hashmap!(
            "group" => "",
            "kind" => "default",
            "name" => name,
        )),
        ServerRef::Server(name) => convert_args!(hashmap!(
            "group" => "policy.linkerd.io",
            "kind" => "server",
            "name" => name,
        )),
    };
    trace!(?labels);

    proto::Server {
        protocol: Some(protocol),
        authorizations,
        labels,
        ..Default::default()
    }
}

fn to_authz(
    reference: &AuthorizationRef,
    ClientAuthorization {
        networks,
        authentication,
    }: &ClientAuthorization,
    cluster_networks: &[IpNet],
) -> proto::Authz {
    let networks = if networks.is_empty() {
        cluster_networks
            .iter()
            .map(|n| proto::Network {
                net: Some((*n).into()),
                except: vec![],
            })
            .collect::<Vec<_>>()
    } else {
        networks
            .iter()
            .map(|NetworkMatch { net, except }| proto::Network {
                net: Some((*net).into()),
                except: except.iter().cloned().map(Into::into).collect(),
            })
            .collect()
    };

    let labels = match reference {
        AuthorizationRef::Default(name) => convert_args!(hashmap!(
            "group" => "",
            "kind" => "default",
            "name" => name,
        )),
        AuthorizationRef::ServerAuthorization(name) => convert_args!(hashmap!(
            "group" => "policy.linkerd.io",
            "kind" => "serverauthorization",
            "name" => name,
        )),
        AuthorizationRef::AuthorizationPolicy(name) => convert_args!(hashmap!(
            "group" => "policy.linkerd.io",
            "kind" => "authorizationpolicy",
            "name" => name,
        )),
    };

    let authn = match authentication {
        ClientAuthentication::Unauthenticated => proto::Authn {
            permit: Some(proto::authn::Permit::Unauthenticated(
                proto::authn::PermitUnauthenticated {},
            )),
        },

        ClientAuthentication::TlsUnauthenticated => proto::Authn {
            permit: Some(proto::authn::Permit::MeshTls(proto::authn::PermitMeshTls {
                clients: Some(proto::authn::permit_mesh_tls::Clients::Unauthenticated(
                    proto::authn::PermitUnauthenticated {},
                )),
            })),
        },

        // Authenticated connections must have TLS and apply to all
        // networks.
        ClientAuthentication::TlsAuthenticated(identities) => {
            let suffixes = identities
                .iter()
                .filter_map(|i| match i {
                    IdentityMatch::Suffix(s) => Some(proto::IdentitySuffix { parts: s.to_vec() }),
                    _ => None,
                })
                .collect();

            let identities = identities
                .iter()
                .filter_map(|i| match i {
                    IdentityMatch::Exact(n) => Some(proto::Identity {
                        name: n.to_string(),
                    }),
                    _ => None,
                })
                .collect();

            proto::Authn {
                permit: Some(proto::authn::Permit::MeshTls(proto::authn::PermitMeshTls {
                    clients: Some(proto::authn::permit_mesh_tls::Clients::Identities(
                        proto::authn::permit_mesh_tls::PermitClientIdentities {
                            identities,
                            suffixes,
                        },
                    )),
                })),
            }
        }
    };

    proto::Authz {
        networks,
        labels,
        authentication: Some(authn),
        metadata: None, // TODO fill
    }
}

fn to_http_route(
    name: impl ToString,
    route: &HttpRoute,
    srv: &InboundServer,
    cluster_networks: &[IpNet],
) -> linkerd2_proxy_api::inbound::HttpRoute {
    use linkerd2_proxy_api::{
        inbound::{http_route::Rule, HttpRoute},
        meta,
    };

    let metadata = meta::Metadata {
        kind: Some(meta::metadata::Kind::Resource(meta::Resource {
            group: "gateway.networking.k8s.io".to_string(),
            kind: "HTTPRoute".to_string(),
            name: name.to_string(),
        })),
    };

    let hosts = route
        .hostnames
        .iter()
        .cloned()
        .map(into_host_match)
        .collect();

    let authorizations = srv
        .authorizations
        .iter()
        .map(|(n, c)| to_authz(n, c, cluster_networks))
        .collect();
    trace!(?authorizations);

    let matches = route
        .matches
        .iter()
        .cloned()
        .map(into_route_match)
        .collect();

    // XXX why is this okay? Does this mean that the proxy API should change?
    // Presumably we're not hanlding filters yet?
    let rules = vec![Rule {
        matches,
        filters: Vec::new(),
    }];

    HttpRoute {
        metadata: Some(metadata),
        hosts,
        authorizations,
        rules,
    }
}

fn into_route_match(
    HttpRouteMatch {
        path,
        headers,
        query_params,
        method,
    }: HttpRouteMatch,
) -> linkerd2_proxy_api::http_route::HttpRouteMatch {
    linkerd2_proxy_api::http_route::HttpRouteMatch {
        path: path.map(into_path_match),
        headers: headers.into_iter().map(into_header_match).collect(),
        query_params: query_params
            .into_iter()
            .map(into_query_param_match)
            .collect(),
        method: method.map(Into::into),
    }
}

fn into_host_match(hostname: Hostname) -> linkerd2_proxy_api::http_route::HostMatch {
    use linkerd2_proxy_api::http_route::{host_match, HostMatch};
    match hostname {
        Hostname::Exact(host) => HostMatch {
            r#match: Some(host_match::Match::Exact(host)),
        },
        Hostname::Suffix { reverse_labels } => HostMatch {
            r#match: Some(host_match::Match::Suffix(host_match::Suffix {
                reverse_labels,
            })),
        },
    }
}

fn into_header_match(
    HeaderMatch { name, value }: HeaderMatch,
) -> linkerd2_proxy_api::http_route::HeaderMatch {
    use linkerd2_proxy_api::http_route::{self as api, header_match};
    api::HeaderMatch {
        name,
        value: match value {
            Value::Exact(v) => Some(header_match::Value::Exact(v)),
            Value::Regex(re) => Some(header_match::Value::Regex(re.to_string())),
        },
    }
}

fn into_path_match(pm: PathMatch) -> linkerd2_proxy_api::http_route::PathMatch {
    use linkerd2_proxy_api::http_route::{self as api, path_match};
    match pm {
        PathMatch::Exact(path) => api::PathMatch {
            kind: Some(path_match::Kind::Exact(path)),
        },
        PathMatch::Prefix(pfx) => api::PathMatch {
            kind: Some(path_match::Kind::Prefix(pfx)),
        },
        PathMatch::Regex(re) => api::PathMatch {
            kind: Some(path_match::Kind::Regex(re.to_string())),
        },
    }
}

fn into_query_param_match(
    QueryParamMatch { name, value }: QueryParamMatch,
) -> linkerd2_proxy_api::http_route::QueryParamMatch {
    use linkerd2_proxy_api::http_route::{self as api, query_param_match};
    api::QueryParamMatch {
        name,
        value: match value {
            Value::Exact(v) => Some(query_param_match::Value::Exact(v)),
            Value::Regex(re) => Some(query_param_match::Value::Regex(re.to_string())),
        },
    }
}
