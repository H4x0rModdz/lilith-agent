//! Smoke test só de leitura: `cargo run -p lilith-win --example smoke`.
#[tokio::main]
async fn main() {
    use serde_json::json;
    for (action, params) in [
        ("audio.get_volume", json!({})),
        ("file.list", json!({ "path": "Downloads" })),
        ("web.search", json!({ "query": "vlc media player download", "limit": 2 })),
    ] {
        let out = lilith_win::dispatch(action, params).await;
        let text = format!("{out:?}");
        println!("{action}: {}", text.chars().take(300).collect::<String>());
    }
}
