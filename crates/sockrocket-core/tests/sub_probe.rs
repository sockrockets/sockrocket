//! Live probe: fetch + parse a clash subscription URL from `.local/live.env`.
//! Run: cargo test -p sockrocket-core --test sub_probe -- --ignored --nocapture

mod common;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "needs SOCKROCKET_SUB_URL in .local/live.env"]
async fn sub_probe() {
    let env = common::load_env_file("live.env");
    let url = common::require(&env, "SOCKROCKET_SUB_URL");
    let content = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap()
        .get(&url)
        .send()
        .await
        .expect("fetch sub")
        .bytes()
        .await
        .expect("read body");
    eprintln!("fetched {} bytes", content.len());

    let text = String::from_utf8_lossy(&content).to_string();
    let format = sockrocket_core::detect_format(&text);
    eprintln!("detected format: {:?}", format);
    let nodes = sockrocket_core::parse_subscription_content(&text, "clash").expect("parse sub");
    eprintln!("parsed {} nodes", nodes.len());
}
