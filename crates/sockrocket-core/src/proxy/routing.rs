use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result;

use super::connector::{BoxProxyStream, Outbound, SharedOutbound};
use crate::router::{RouteAction, Router};

/// Shared immutable rule snapshots for long-lived listeners. Never hold the
/// lock across an await; accepted streams retain their own snapshot.
#[derive(Clone)]
pub struct RoutingState(Arc<std::sync::RwLock<Option<Arc<Router>>>>);

impl RoutingState {
    pub fn new(router: Option<Arc<Router>>) -> Self {
        Self(Arc::new(std::sync::RwLock::new(router)))
    }

    pub fn snapshot(&self) -> Option<Arc<Router>> {
        self.0.read().unwrap().clone()
    }

    pub fn replace(&self, router: Option<Arc<Router>>) {
        *self.0.write().unwrap() = router;
    }
}

/// An outbound that routes connections through a Router, dispatching to
/// either a proxy outbound, direct outbound, or rejecting the connection.
pub struct RoutingOutbound {
    router: Arc<Router>,
    proxy_outbound: SharedOutbound,
    direct_outbound: SharedOutbound,
}

impl RoutingOutbound {
    pub fn new(router: Arc<Router>, proxy_outbound: SharedOutbound) -> Self {
        Self {
            router,
            proxy_outbound,
            direct_outbound: SharedOutbound::direct(),
        }
    }
}

impl Outbound for RoutingOutbound {
    fn connect(
        &self,
        host: &str,
        port: u16,
    ) -> Pin<Box<dyn Future<Output = Result<BoxProxyStream>> + Send + '_>> {
        // Own the host: the returned future's lifetime is tied to `&self`,
        // so it cannot borrow the caller's `host` string.
        let host = host.to_string();
        Box::pin(async move {
            let action = self.router.route_resolving(&host, port).await;
            tracing::debug!("Route: {}:{} -> {:?}", host, port, action);

            match action {
                RouteAction::Proxy => self.proxy_outbound.0.connect(&host, port).await,
                RouteAction::Direct => self.direct_outbound.0.connect(&host, port).await,
                RouteAction::Reject => {
                    anyhow::bail!("Connection rejected by routing rule: {}:{}", host, port)
                }
            }
        })
    }

    fn connect_datagram(
        &self,
        host: &str,
        port: u16,
    ) -> super::datagram::DatagramFuture<'_, super::datagram::BoxDatagram> {
        let host = host.to_owned();
        Box::pin(async move {
            match self.router.route_resolving(&host, port).await {
                RouteAction::Direct => self.direct_outbound.0.connect_datagram(&host, port).await,
                RouteAction::Proxy => self.proxy_outbound.0.connect_datagram(&host, port).await,
                RouteAction::Reject => anyhow::bail!("UDP rejected by routing rule"),
            }
        })
    }

    fn routing_policy(&self) -> Option<Arc<Router>> {
        Some(self.router.clone())
    }
    fn is_direct(&self) -> bool {
        self.proxy_outbound.0.is_direct()
    }

    fn name(&self) -> &str {
        "routing"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::router::{MatchRule, RuleSet};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    #[test]
    fn routing_updates_are_shared_but_old_snapshots_remain_valid() {
        let first = Arc::new(Router::new(RuleSet::new()));
        let state = RoutingState::new(Some(first.clone()));
        let reader = state.clone();
        let snapshot = reader.snapshot().unwrap();
        state.replace(None);
        assert!(reader.snapshot().is_none());
        assert!(Arc::ptr_eq(&snapshot, &first));
        let second = Arc::new(Router::new(RuleSet::new()));
        state.replace(Some(second.clone()));
        assert!(Arc::ptr_eq(&reader.snapshot().unwrap(), &second));
    }

    #[tokio::test]
    async fn test_routing_outbound_direct() {
        // Echo server
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _echo = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 5];
            s.read_exact(&mut buf).await.unwrap();
            s.write_all(&buf).await.unwrap();
        });

        // Route everything direct
        let mut rules = RuleSet::new();
        rules.set_default(RouteAction::Direct);
        let router = Arc::new(Router::new(rules));

        let outbound = RoutingOutbound::new(router, SharedOutbound::direct());
        let mut stream = outbound
            .connect(&addr.ip().to_string(), addr.port())
            .await
            .unwrap();

        stream.write_all(b"hello").await.unwrap();
        let mut buf = [0u8; 5];
        stream.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[tokio::test]
    async fn test_routing_outbound_reject() {
        let mut rules = RuleSet::new();
        rules.add_rule(MatchRule::DomainKeyword("ads".into()), RouteAction::Reject);
        let router = Arc::new(Router::new(rules));

        let outbound = RoutingOutbound::new(router, SharedOutbound::direct());
        let result = outbound.connect("ads.example.com", 80).await;
        assert!(result.is_err());
        let err = result.err().unwrap();
        assert!(err.to_string().contains("rejected"));
    }
}
