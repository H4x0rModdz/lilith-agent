//! Busca e leitura de páginas. Portado de
//! `jarvis-os/system/action-bus/src/handlers/web.rs`.

use crate::{req_str, WinError};
use scraper::{Html, Selector};
use serde_json::{json, Value};

const DEFAULT_SEARCH_URL: &str = "https://lite.duckduckgo.com/lite/";
// O DDG (e a maioria dos buscadores) recusa UA vazio — apresenta-se como navegador.
const USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:140.0) Gecko/20100101 Firefox/140.0";

/// Pesquisa na web; até `limit` resultados `[{title, url, snippet}]`.
/// Padrão: DuckDuckGo Lite (sem chave). `LILITH_WEB_SEARCH_URL` troca o
/// endpoint (ex.: uma instância SearXNG).
pub async fn search(params: Value) -> Result<Value, WinError> {
    let query = req_str(&params, "query")?;
    let limit = params["limit"].as_u64().unwrap_or(5).clamp(1, 10) as usize;
    let base = std::env::var("LILITH_WEB_SEARCH_URL").unwrap_or_else(|_| DEFAULT_SEARCH_URL.into());
    let html = http_client()?
        .get(&base)
        .query(&[("q", query)])
        .send()
        .await
        .map_err(|e| WinError::Unavailable(format!("busca web: {e}")))?
        .text()
        .await
        .map_err(|e| WinError::Failed(format!("leitura da busca: {e}")))?;
    Ok(json!({ "query": query, "results": parse_ddg_lite(&html, limit) }))
}

/// Texto legível de uma página (sem scripts/markup), truncado.
pub async fn fetch(params: Value) -> Result<Value, WinError> {
    let url = req_str(&params, "url")?;
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err(WinError::InvalidParams("url precisa ser http(s)".into()));
    }
    let max = params["max_chars"].as_u64().unwrap_or(4000).clamp(200, 20000) as usize;
    let html = http_client()?
        .get(url)
        .send()
        .await
        .map_err(|e| WinError::Unavailable(format!("fetch {url}: {e}")))?
        .text()
        .await
        .map_err(|e| WinError::Failed(format!("leitura da página: {e}")))?;
    let text: String = html_to_text(&html).chars().take(max).collect();
    Ok(json!({ "url": url, "text": text }))
}

/// Cliente HTTP compartilhado (UA de navegador + timeout). Usado também por
/// downloads e wallpaper.
pub(crate) fn http_client() -> Result<reqwest::Client, WinError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(20))
        .build()
        .map_err(|e| WinError::Failed(format!("cliente http: {e}")))
}

/// Resultados do DDG Lite: `a.result-link` pareado por posição com `.result-snippet`.
fn parse_ddg_lite(html: &str, limit: usize) -> Vec<Value> {
    let doc = Html::parse_document(html);
    let link_sel = Selector::parse("a.result-link").unwrap();
    let snip_sel = Selector::parse(".result-snippet").unwrap();
    let snippets: Vec<String> = doc
        .select(&snip_sel)
        .map(|s| collapse_ws(&s.text().collect::<String>()))
        .collect();

    let mut out = Vec::new();
    for (i, a) in doc.select(&link_sel).enumerate() {
        if out.len() >= limit {
            break;
        }
        let url = clean_ddg_href(a.value().attr("href").unwrap_or(""));
        if url.is_empty() {
            continue;
        }
        out.push(json!({
            "title": collapse_ws(&a.text().collect::<String>()),
            "url": url,
            "snippet": snippets.get(i).cloned().unwrap_or_default(),
        }));
    }
    out
}

fn html_to_text(html: &str) -> String {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("p, h1, h2, h3, h4, li, article, blockquote").unwrap();
    doc.select(&sel)
        .map(|el| collapse_ws(&el.text().collect::<String>()))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// O DDG Lite embrulha os links em `/l/?uddg=<url codificada>`.
fn clean_ddg_href(href: &str) -> String {
    if let Some(idx) = href.find("uddg=") {
        let enc = &href[idx + 5..];
        return percent_decode(enc.split('&').next().unwrap_or(enc));
    }
    if let Some(rest) = href.strip_prefix("//") {
        return format!("https://{rest}");
    }
    href.to_string()
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(if bytes[i] == b'+' { b' ' } else { bytes[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddg_lite_parse_and_unwrap() {
        let html = r#"<table><tr><td><a class="result-link" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.videolan.org%2F&rut=x">VLC</a></td></tr>
                      <tr><td class="result-snippet">  Free   player </td></tr></table>"#;
        let r = parse_ddg_lite(html, 5);
        assert_eq!(r[0]["url"], "https://www.videolan.org/");
        assert_eq!(r[0]["snippet"], "Free player");
    }
}
