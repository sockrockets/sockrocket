use super::*;
use crate::config::model::{TransportConfig, TransportType};
use std::sync::atomic::AtomicUsize;
struct Count(Arc<AtomicUsize>);
impl Drop for Count {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
async fn server(status: Option<u16>) -> (u16, Arc<AtomicUsize>, Task) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let alive = Arc::new(AtomicUsize::new(0));
    let counts = alive.clone();
    let task = Task(tokio::spawn(async move {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
             accepted=listener.accept()=>{let Ok((mut stream,_))=accepted else{break;};let alive=counts.clone();alive.fetch_add(1,Ordering::SeqCst);tasks.spawn(async move {let _count=Count(alive);let mut header=Vec::new();let mut b=[0];while !header.ends_with(b"\r\n\r\n")&&header.len()<65536 {if stream.read_exact(&mut b).await.is_err(){return;}header.push(b[0]);}if let Some(status)=status {let reply=format!("HTTP/1.1 {status} Test\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");let _=stream.write_all(reply.as_bytes()).await;}else{let _=tokio::io::copy(&mut stream,&mut tokio::io::sink()).await;}});},
             Some(_)=tasks.join_next(),if !tasks.is_empty()=>{}
            }
        }
    }));
    (port, alive, task)
}
fn factory(port: u16, options: serde_json::Value) -> Arc<Factory> {
    let raw = Arc::new(TlsConnFactory::new("127.0.0.1", port, None, false, None));
    let c = XhttpConfig {
        mode: "packet-up".into(),
        options: std::collections::BTreeMap::from([("extra".into(), options)]),
        ..Default::default()
    };
    Factory::new(
        raw,
        "127.0.0.1",
        port,
        &TransportConfig {
            grpc: None,
            transport_type: TransportType::Xhttp,
            tls: None,
            ws: None,
            reality: None,
            xhttp: Some(c),
        },
    )
    .unwrap()
}
async fn idle(count: &AtomicUsize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while count.load(Ordering::SeqCst) != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn request_failure_reaches_reader_and_does_not_leak_connections() {
    let (port, alive, _server) = server(Some(403)).await;
    let factory = factory(port, serde_json::json!({"xmux":{"maxConnections":1}}));
    let mut stream = factory.open().await.unwrap();
    stream.write_all(b"synthetic request").await.unwrap();
    let mut response = [0; 16];
    let error = tokio::time::timeout(Duration::from_secs(2), stream.read(&mut response))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    drop(stream);
    drop(factory);
    idle(&alive).await;
}
#[tokio::test]
async fn cancelling_pending_headers_closes_all_request_tasks_and_pool_connections() {
    let (port, alive, _server) = server(None).await;
    let factory = factory(port, serde_json::json!({"xmux":{"maxConnections":1}}));
    let mut stream = factory.open().await.unwrap();
    stream.write_all(b"synthetic request").await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while alive.load(Ordering::SeqCst) < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    drop(stream);
    drop(factory);
    idle(&alive).await;
}
#[tokio::test]
async fn request_limit_retires_busy_slots_without_deadlocking_existing_sessions() {
    let (port, alive, _server) = server(Some(200)).await;
    let factory = factory(
        port,
        serde_json::json!({"xmux":{"maxConnections":1,"hMaxRequestTimes":2,"cMaxReuseTimes":1}}),
    );
    let mut lease = Arc::new(factory.acquire().await.unwrap());
    let first = lease.slot.clone();
    for _ in 0..8 {
        let request = ::http::Request::builder()
            .uri("http://example.invalid/synthetic")
            .body(())
            .unwrap();
        let mut exchange = factory.request(request, Body::Empty, lease).await.unwrap();
        lease = exchange._lease.clone();
        tokio::io::copy(&mut exchange.response.body, &mut tokio::io::sink())
            .await
            .unwrap();
    }
    assert!(!Arc::ptr_eq(&first, &lease.slot));
    drop(first);
    drop(lease);
    drop(factory);
    idle(&alive).await;
}
#[test]
fn import_options_cover_all_wire_modes_and_reject_unknown_fields() {
    for mode in ["auto", "stream-one", "stream-up", "packet-up"] {
        let c = XhttpConfig {
            mode: mode.into(),
            ..Default::default()
        };
        let s = Settings::parse(&c).unwrap();
        assert_eq!(
            s.mode(false),
            if mode == "auto" { "packet-up" } else { mode }
        );
    }
    let mut c = XhttpConfig::default();
    c.options.insert("extra".into(),serde_json::json!({"xmux":{"maxConnections":1,"maxConcurrency":0},"sessionIDTable":"Base62","sessionIDLength":"16-24"}));
    assert!(Settings::parse(&c).is_ok());
    c.options
        .get_mut("extra")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert("unknownOption".into(), true.into());
    assert!(Settings::parse(&c).is_err());
}

#[test]
fn extra_replaces_direct_advanced_fields_and_keeps_outer_identity() {
    let mut config = XhttpConfig {
        path: Some("/outer".into()),
        host: Some("outer.invalid".into()),
        mode: "packet-up".into(),
        ..Default::default()
    };
    config
        .options
        .insert("headers".into(), serde_json::json!({"x-ignored":"direct"}));
    config.options.insert("extra".into(),serde_json::json!({"path":"/ignored","host":"ignored.invalid","mode":"invalid","headers":{"x-active":"extra"}}));
    let settings = Settings::parse(&config).unwrap();
    assert!(!settings.options.headers.contains_key("x-ignored"));
    assert_eq!(settings.config.path.as_deref(), Some("/outer"));
    let (request, _) = settings
        .request(
            "fallback.invalid",
            true,
            "session",
            Some(0),
            Some(bytes::Bytes::from_static(b"test")),
            false,
        )
        .unwrap();
    assert_eq!(request.uri().host(), Some("outer.invalid"));
    assert!(request.uri().path().starts_with("/outer/"));
    config.options.get_mut("extra").unwrap()["headers"] = serde_json::json!({"Host":"bad.invalid"});
    assert!(Settings::parse(&config).is_err());
}
