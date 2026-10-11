//! Busca e leitura de páginas. Portado de
//! `jarvis-os/system/action-bus/src/handlers/web.rs`.

use crate::{req_str, WinError};
use scraper::{Html, Selector};
use serde_json::{json, Value};
use std::net::IpAddr;

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
    let html = public_get(url)
        .await?
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

const MAX_REDIRECTS: usize = 5;

/// GET que só alcança a internet pública. `web.fetch` é scope seguro (sem
/// aprovação) e a URL vem do modelo — que pode ter lido uma página com
/// prompt injection. Sem isso, ele alcançaria localhost (API do Ollama,
/// serviços de dev) e a rede local (roteador, NAS). Redirects são seguidos à
/// mão para validar cada salto.
///
/// Limitação conhecida: DNS rebinding entre a checagem e a conexão.
pub(crate) async fn public_get(url: &str) -> Result<reqwest::Response, WinError> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| WinError::Failed(format!("cliente http: {e}")))?;
    let mut current = reqwest::Url::parse(url).map_err(|e| WinError::InvalidParams(format!("url inválida: {e}")))?;
    for _ in 0..=MAX_REDIRECTS {
        ensure_public(&current).await?;
        let resp = client
            .get(current.clone())
            .send()
            .await
            .map_err(|e| WinError::Unavailable(format!("fetch {current}: {e}")))?;
        if !resp.status().is_redirection() {
            return resp
                .error_for_status()
                .map_err(|e| WinError::Failed(format!("fetch {current}: {e}")));
        }
        let location = resp
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| WinError::Failed("redirect sem Location".into()))?;
        current = current
            .join(location)
            .map_err(|e| WinError::Failed(format!("redirect inválido: {e}")))?;
    }
    Err(WinError::Failed(format!("redirects demais (> {MAX_REDIRECTS})")))
}

async fn ensure_public(url: &reqwest::Url) -> Result<(), WinError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(WinError::InvalidParams("url precisa ser http(s)".into()));
    }
    let host = url.host_str().ok_or_else(|| WinError::InvalidParams("url sem host".into()))?;
    let port = url.port_or_known_default().unwrap_or(80);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let addrs: Vec<IpAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| WinError::Unavailable(format!("dns {host}: {e}")))?
        .map(|a| a.ip())
        .collect();
    if addrs.is_empty() || addrs.iter().any(|ip| !is_public(*ip)) {
        return Err(WinError::InvalidParams(format!(
            "'{host}' aponta para a rede local/loopback — bloqueado para web.fetch"
        )));
    }
    Ok(())
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || a == 0
                || (a == 100 && (64..128).contains(&b)) // CGNAT / Tailscale
                || a >= 224) // multicast e reservado
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (first & 0xfe00) == 0xfc00 // unique local
                || (first & 0xffc0) == 0xfe80) // link-local
        }
    }
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
    fn private_and_loopback_are_not_public() {
        for ip in [
            "127.0.0.1", "10.0.0.5", "192.168.1.1", "172.16.0.1", "169.254.169.254", "0.0.0.0",
            "100.100.1.1", "::1", "fd00::1", "fe80::1", "::ffff:127.0.0.1",
        ] {
            assert!(!is_public(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["1.1.1.1", "142.250.0.1", "2606:4700::1111"] {
            assert!(is_public(ip.parse().unwrap()), "{ip}");
        }
    }

    #[tokio::test]
    async fn fetch_refuses_local_targets() {
        for url in ["http://127.0.0.1:11434/api/tags", "http://localhost/", "http://[::1]/", "http://192.168.0.1/"] {
            let err = fetch(json!({ "url": url })).await.unwrap_err();
            assert!(matches!(err, WinError::InvalidParams(_)), "{url}: {err}");
        }
    }

    #[test]
    fn ddg_lite_parse_and_unwrap() {
        let html = r#"<table><tr><td><a class="result-link" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.videolan.org%2F&rut=x">VLC</a></td></tr>
                      <tr><td class="result-snippet">  Free   player </td></tr></table>"#;
        let r = parse_ddg_lite(html, 5);
        assert_eq!(r[0]["url"], "https://www.videolan.org/");
        assert_eq!(r[0]["snippet"], "Free player");
    }
}
