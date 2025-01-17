use linkerd_policy_controller_k8s_api::{self as api, policy::httproute::*};
use linkerd_policy_test::admission;

#[tokio::test(flavor = "current_thread")]
async fn accepts_valid() {
    admission::accepts(|ns| HttpRoute {
        metadata: api::ObjectMeta {
            namespace: Some(ns.clone()),
            name: Some("test".to_string()),
            ..Default::default()
        },
        spec: HttpRouteSpec {
            inner: CommonRouteSpec {
                parent_refs: Some(vec![server_parent_ref(ns)]),
            },
            hostnames: None,
            rules: Some(rules()),
        },
        status: None,
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn rejects_non_server_parent_ref() {
    admission::rejects(|ns| HttpRoute {
        metadata: api::ObjectMeta {
            namespace: Some(ns.clone()),
            name: Some("test".to_string()),
            ..Default::default()
        },
        spec: HttpRouteSpec {
            inner: CommonRouteSpec {
                parent_refs: Some(vec![non_server_parent_ref(ns)]),
            },
            hostnames: None,
            rules: Some(rules()),
        },
        status: None,
    })
    .await;
}

/// Tests that an `HTTPRoute` is rejected if it contains *any* parent refs that
/// target non-`Server` resources, even if it also targets a `Server` resource.
#[tokio::test(flavor = "current_thread")]
async fn rejects_mixed_parent_ref() {
    admission::rejects(|ns| HttpRoute {
        metadata: api::ObjectMeta {
            namespace: Some(ns.clone()),
            name: Some("test".to_string()),
            ..Default::default()
        },
        spec: HttpRouteSpec {
            inner: CommonRouteSpec {
                parent_refs: Some(vec![
                    server_parent_ref(ns.clone()),
                    non_server_parent_ref(ns),
                ]),
            },
            hostnames: None,
            rules: Some(rules()),
        },
        status: None,
    })
    .await;
}

fn server_parent_ref(ns: String) -> ParentReference {
    ParentReference {
        group: Some("policy.linkerd.io".to_string()),
        kind: Some("Server".to_string()),
        namespace: Some(ns),
        name: "my-server".to_string(),
        section_name: None,
        port: None,
    }
}

fn non_server_parent_ref(ns: String) -> ParentReference {
    ParentReference {
        group: Some("foo.bar.bas".to_string()),
        kind: Some("Gateway".to_string()),
        namespace: Some(ns),
        name: "my-gateway".to_string(),
        section_name: None,
        port: None,
    }
}

fn rules() -> Vec<HttpRouteRule> {
    vec![HttpRouteRule {
        matches: Some(vec![HttpRouteMatch {
            path: Some(HttpPathMatch::Exact {
                value: "/foo".to_string(),
            }),
            ..HttpRouteMatch::default()
        }]),
        filters: None,
    }]
}
