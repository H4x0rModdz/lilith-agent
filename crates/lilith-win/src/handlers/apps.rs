use super::{blocking, hidden_command};
use crate::paths::{fold, resolve};
use crate::{req_str, WinError};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Mutex;
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Abre um app pelo nome. Ordem: apelidos pt-BR → entradas do Menu Iniciar
/// (`Get-StartApps`, cobre apps Win32 e da Store) → ShellExecute direto
/// (executáveis no PATH / App Paths, ex.: "chrome", "code").
pub async fn open(params: Value) -> Result<Value, WinError> {
    let app = req_str(&params, "app")?.to_string();

    // Já aberto: traz para frente em vez de abrir outra instância (pedir
    // "abre o waterfox" com ele aberto não deve criar mais janelas).
    if !params["new_window"].as_bool().unwrap_or(false) {
        let exe_hint = alias(&app).map(|a| a.trim_end_matches(".exe").to_string()).unwrap_or(app.clone());
        if let Some(w) = blocking(move || Ok(crate::window::find_app(&exe_hint))).await? {
            let id = w.id;
            blocking(move || crate::window::focus(id)).await?;
            return Ok(json!({ "focused": w.title, "already_open": true }));
        }
    }

    if let Some(target) = alias(&app) {
        shell_open(target.to_string()).await?;
        return Ok(json!({ "opened": app, "via": "alias" }));
    }

    if let Some(entry) = find_start_app(&app).await {
        shell_open(format!("shell:AppsFolder\\{}", entry.app_id)).await?;
        return Ok(json!({ "opened": entry.name, "via": "start_menu" }));
    }

    // Fallback só para nomes simples ("chrome", "code.exe"), resolvidos pelo
    // PATH / App Paths. Caminhos, esquemas e argumentos não passam: app.open
    // é scope seguro, e aceitar `C:\x\algo.exe` executaria qualquer binário
    // sem aprovação — para isso existe file.run.
    if !is_plain_app_name(&app) {
        return Err(WinError::InvalidParams(format!(
            "'{app}' não é nome de app; para executar um arquivo use file.run (pede aprovação)"
        )));
    }
    shell_open(app.clone())
        .await
        .map_err(|_| WinError::NotFound(format!("app '{app}' (não está no Menu Iniciar nem no PATH)")))?;
    Ok(json!({ "opened": app, "via": "shell" }))
}

fn is_plain_app_name(name: &str) -> bool {
    let n = name.trim();
    let stem_ok = !n.is_empty()
        && n.chars().all(|c| c.is_alphanumeric() || matches!(c, ' ' | '.' | '-' | '_'))
        && !n.starts_with('.');
    let ext = std::path::Path::new(n).extension().and_then(|e| e.to_str()).map(str::to_lowercase);
    stem_ok && matches!(ext.as_deref(), None | Some("exe"))
}

/// Fecha processos cujo nome de imagem contém `app`.
pub async fn close(params: Value) -> Result<Value, WinError> {
    let app = fold(req_str(&params, "app")?);
    let wanted = app.trim_end_matches(".exe").to_string();
    let force = params["force"].as_bool().unwrap_or(false);

    let out = hidden_command("tasklist")
        .args(["/FO", "CSV", "/NH"])
        .output()
        .await?;
    let listing = String::from_utf8_lossy(&out.stdout);
    let mut images: Vec<String> = listing
        .lines()
        .filter_map(|l| l.split("\",\"").next())
        .map(|s| s.trim_start_matches('"').to_string())
        .filter(|img| fold(img).trim_end_matches(".exe").contains(&wanted))
        .collect();
    images.sort();
    images.dedup();
    if images.is_empty() {
        return Err(WinError::NotFound(format!("processo '{wanted}' em execução")));
    }

    for img in &images {
        let mut cmd = hidden_command("taskkill");
        cmd.args(["/IM", img, "/T"]);
        if force {
            cmd.arg("/F");
        }
        let status = cmd.status().await?;
        if !status.success() && !force {
            // Sem /F o taskkill só pede pra janela fechar; processo sem janela
            // ignora. Reporta em vez de escalar sozinho para kill forçado.
            return Err(WinError::Failed(format!(
                "'{img}' não fechou educadamente — peça para forçar"
            )));
        }
    }
    Ok(json!({ "closed": images, "forced": force }))
}

pub async fn browser_open(params: Value) -> Result<Value, WinError> {
    let raw = req_str(&params, "url")?;
    let raw = if raw.contains("://") { raw.to_string() } else { format!("https://{raw}") };
    let url = normalize_url(&raw)?;
    let mode = crate::browsers::Mode::parse(params["mode"].as_str());
    let via = open_url_in(&url, params["browser"].as_str(), mode).await?;
    Ok(json!({ "opened": url, "browser": via }))
}

/// Pesquisa numa NOVA ABA do navegador (o que já está aberto, se houver).
/// `kind`: web (padrão), images, videos, news — vai direto para a aba de
/// resultados certa, sem simular teclado nem clicar na página.
pub async fn browser_search(params: Value) -> Result<Value, WinError> {
    let query = req_str(&params, "query")?;
    let kind = params["kind"].as_str().unwrap_or("web");
    let url = search_url(query, kind)?;
    let mode = crate::browsers::Mode::parse(params["mode"].as_str());
    let via = open_url_in(&url, params["browser"].as_str(), mode).await?;
    Ok(json!({ "searched": query, "kind": kind, "url": url, "browser": via }))
}

/// URL de busca do Google para o tipo pedido.
pub(crate) fn search_url(query: &str, kind: &str) -> Result<String, WinError> {
    let mut url = reqwest::Url::parse("https://www.google.com/search").expect("url fixa");
    url.query_pairs_mut().append_pair("q", query);
    let tbm = match fold(kind).as_str() {
        "web" | "" | "todos" | "all" => None,
        "images" | "imagens" | "image" | "imagem" => Some("isch"),
        "videos" | "video" => Some("vid"),
        "news" | "noticias" => Some("nws"),
        other => return Err(WinError::InvalidParams(format!("tipo de busca '{other}' (web, images, videos, news)"))),
    };
    if let Some(t) = tbm {
        url.query_pairs_mut().append_pair("tbm", t);
    }
    Ok(url.to_string())
}

/// URL canônica http(s): espaços, aspas e afins saem codificados (%20,
/// %22…). Ela vira argumento de linha de comando do navegador — sem isso,
/// "https://a.com --gpu-launcher=calc" viraria uma flag do Chromium.
pub(crate) fn normalize_url(raw: &str) -> Result<String, WinError> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|e| WinError::InvalidParams(format!("url inválida: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(WinError::InvalidParams("só URLs http(s)".into()));
    }
    let s = url.to_string();
    if s.chars().any(|c| c.is_whitespace() || c == '"') {
        return Err(WinError::InvalidParams("url com caracteres inválidos".into()));
    }
    Ok(s)
}

/// Navegador com janela aberta, o de cima na ordem Z (o último usado).
fn running_browser() -> Option<String> {
    crate::window::list()
        .into_iter()
        .map(|w| fold(&w.app))
        .find(|app| crate::browsers::family_of_exe(app).is_some())
}

/// Abre a URL no navegador certo, na família certa:
/// 1. o navegador citado (precisa ser conhecido);
/// 2. senão, o navegador PADRÃO do usuário (lido do registro);
/// 3. senão, o que já estiver aberto; 4. senão, o padrão do Windows.
/// `mode`: nova aba (padrão), nova janela ou janela privada.
async fn open_url_in(url: &str, browser: Option<&str>, mode: crate::browsers::Mode) -> Result<String, WinError> {
    use crate::browsers::{self, Family};
    let url = normalize_url(url)?;
    let named = browser.map(str::trim).filter(|b| !b.is_empty());

    // (executável, família, nome)
    let target: Option<(std::path::PathBuf, Family, String)> = match named {
        Some(name) => {
            let Some(b) = browsers::known(name) else {
                return Err(WinError::InvalidParams(format!(
                    "'{name}' não é um navegador conhecido ({})",
                    browsers::ids().join(", ")
                )));
            };
            let id = b.id;
            // Janela aberta desse navegador: usa o executável dela.
            let running = blocking(move || {
                Ok(crate::window::find_app(id)
                    .filter(|w| browsers::family_of_exe(&w.app).is_some())
                    .and_then(|w| crate::window::exe_of(w.id)))
            })
            .await?;
            let exe = running.unwrap_or_else(|| std::path::PathBuf::from(format!("{id}.exe")));
            Some((exe, b.family, id.to_string()))
        }
        None => match blocking(|| Ok(browsers::default_browser())).await? {
            Some(d) => Some((d.exe, d.family, d.id)),
            None => blocking(|| Ok(running_browser())).await?.and_then(|app| {
                browsers::known(&app).map(|b| (std::path::PathBuf::from(format!("{}.exe", b.id)), b.family, b.id.to_string()))
            }),
        },
    };
    let Some((exe, family, id)) = target else {
        shell_open(url).await?;
        return Ok("padrão".into());
    };
    let args = family.open_args(&exe, mode, &url);
    // Sem shell: cada argumento vai separado, nunca reinterpretado.
    if std::process::Command::new(&exe).args(&args).spawn().is_ok() {
        return Ok(id);
    }
    // Não está no PATH: App Paths do Windows resolve "<nome>.exe".
    let exe_name = exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| format!("{id}.exe"));
    shell_exec_with(&exe_name, &quote_args(&args)?)
        .await
        .map(|_| id.clone())
        .map_err(|_| WinError::NotFound(format!("navegador '{id}'")))
}

/// Linha de parâmetros para ShellExecute: cada argumento entre aspas;
/// argumento com aspas é recusado (não há como escapar com segurança).
fn quote_args(args: &[String]) -> Result<String, WinError> {
    args.iter()
        .map(|a| {
            if a.contains('"') {
                Err(WinError::InvalidParams("argumento com aspas".into()))
            } else {
                Ok(format!("\"{a}\""))
            }
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|v| v.join(" "))
}

/// Operação de aba (nova, fechar, próxima, anterior, reabrir, endereço,
/// localizar, recarregar) por teclado, na janela do navegador. Os atalhos
/// são iguais em Firefox e Chromium. Sem `browser`, usa o padrão do usuário.
pub async fn browser_tab(params: Value) -> Result<Value, WinError> {
    use crate::browsers::{self, TabOp};
    let op = TabOp::parse(req_str(&params, "op")?)
        .ok_or_else(|| WinError::InvalidParams("op: new, close, next, previous, reopen, address, find, reload".into()))?;
    let named = params["browser"].as_str().map(str::trim).filter(|b| !b.is_empty()).map(str::to_string);
    let id = match named {
        Some(n) => browsers::known(&n)
            .map(|b| b.id.to_string())
            .ok_or_else(|| WinError::InvalidParams(format!("'{n}' não é um navegador conhecido")))?,
        None => blocking(|| Ok(browsers::default_browser().map(|d| d.id).or_else(running_browser))).await?
            .ok_or_else(|| WinError::NotFound("navegador padrão".into()))?,
    };
    let wid = id.clone();
    let win = blocking(move || Ok(crate::window::find_app(&wid))).await?
        .ok_or_else(|| WinError::NotFound(format!("janela do {id} (abra o navegador antes)")))?;
    blocking(move || {
        crate::window::focus(win.id)?;
        std::thread::sleep(std::time::Duration::from_millis(250));
        let (shift, key) = match op {
            TabOp::New => (false, "t"),
            TabOp::Close => (false, "w"),
            TabOp::Next => (false, "tab"),
            TabOp::Previous => (true, "tab"),
            TabOp::Reopen => (true, "t"),
            TabOp::Address => (false, "l"),
            TabOp::Find => (false, "f"),
            TabOp::Reload => (false, "r"),
        };
        crate::input::press_ctrl_key(shift, key)
    })
    .await?;
    Ok(json!({ "browser": id, "op": format!("{op:?}").to_lowercase() }))
}

async fn shell_exec_with(file: &str, args: &str) -> Result<(), WinError> {
    let (file, args) = (file.to_string(), args.to_string());
    blocking(move || unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let h = ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from(file.as_str()),
            &HSTRING::from(args.as_str()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        if h.0 as usize > 32 {
            Ok(())
        } else {
            Err(WinError::Failed(format!("ShellExecute '{file}'")))
        }
    })
    .await
}

/// Abre pastas e documentos. Qualquer outro tipo (executáveis, scripts,
/// atalhos, instaladores…) seria *executado* pelo handler padrão do Windows,
/// então é recusado aqui e precisa passar por file.run, que pede aprovação.
/// Allowlist em vez de denylist: o Windows tem dezenas de tipos executáveis.
pub async fn file_open(params: Value) -> Result<Value, WinError> {
    let path = resolve(req_str(&params, "path")?);
    if !path.exists() {
        return Err(WinError::NotFound(path.display().to_string()));
    }
    if !path.is_dir() && !is_document(&path) {
        return Err(WinError::InvalidParams(format!(
            "'{}' não é um documento; para executar use file.run (pede aprovação)",
            path.display()
        )));
    }
    shell_open(path.to_string_lossy().into_owned()).await?;
    Ok(json!({ "opened": path }))
}

/// Executa um arquivo (programa, script, instalador, atalho). Scope perigoso.
pub async fn file_run(params: Value) -> Result<Value, WinError> {
    let path = resolve(req_str(&params, "path")?);
    if !path.is_file() {
        return Err(WinError::NotFound(path.display().to_string()));
    }
    shell_open(path.to_string_lossy().into_owned()).await?;
    Ok(json!({ "ran": path }))
}

const DOCUMENT_EXTS: &[&str] = &[
    "txt", "md", "log", "csv", "json", "xml", "yaml", "yml", "toml", "ini", "pdf", "epub", "doc", "docx", "odt",
    "rtf", "xls", "xlsx", "ods", "ppt", "pptx", "odp", "png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "ico",
    "heic", "avif", "tif", "tiff", "mp3", "wav", "flac", "ogg", "m4a", "aac", "opus", "mp4", "mkv", "avi", "mov",
    "webm", "wmv", "m4v", "zip", "7z", "rar", "tar", "gz",
];

fn is_document(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| DOCUMENT_EXTS.contains(&e.to_lowercase().as_str()))
}

/// `ShellExecuteW(open)` — o mesmo que dar duplo clique / Win+R.
pub(crate) async fn shell_open(target: String) -> Result<(), WinError> {
    blocking(move || unsafe {
        // ShellExecute pode delegar para extensões COM do Shell.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
        let h = ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from(target.as_str()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
        // Valores <= 32 são códigos de erro (doc do ShellExecute).
        if h.0 as usize > 32 {
            Ok(())
        } else {
            Err(WinError::Failed(format!("ShellExecute '{target}' → código {}", h.0 as usize)))
        }
    })
    .await
}

fn alias(name: &str) -> Option<&'static str> {
    Some(match fold(name).as_str() {
        "bloco de notas" | "notepad" => "notepad.exe",
        "calculadora" | "calculator" => "calc.exe",
        "paint" => "mspaint.exe",
        "explorador de arquivos" | "explorer" | "arquivos" => "explorer.exe",
        "configuracoes" | "configuracao" | "settings" => "ms-settings:",
        "gerenciador de tarefas" | "task manager" => "taskmgr.exe",
        "terminal" => "wt.exe",
        "prompt de comando" | "cmd" => "cmd.exe",
        "painel de controle" | "control panel" => "control.exe",
        _ => return None,
    })
}

#[derive(Debug, Clone, Deserialize)]
struct StartApp {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "AppID")]
    app_id: String,
}

static START_APPS: Mutex<Option<Vec<StartApp>>> = Mutex::new(None);

/// Busca no cache de `Get-StartApps`; se não achar, recarrega uma vez
/// (o app pode ter sido instalado depois do cache).
async fn find_start_app(query: &str) -> Option<StartApp> {
    let cached = START_APPS.lock().unwrap().clone();
    if let Some(apps) = cached {
        if let Some(hit) = best_match(&apps, query) {
            return Some(hit);
        }
    }
    let fresh = load_start_apps().await.ok()?;
    let hit = best_match(&fresh, query);
    *START_APPS.lock().unwrap() = Some(fresh);
    hit
}

async fn load_start_apps() -> Result<Vec<StartApp>, WinError> {
    let out = hidden_command("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-StartApps | ConvertTo-Json -Compress",
        ])
        .output()
        .await?;
    let text = String::from_utf8_lossy(&out.stdout);
    serde_json::from_str(text.trim()).map_err(|e| WinError::Failed(format!("Get-StartApps: {e}")))
}

/// Exato > começa com > contém; empate vai para o nome mais curto
/// ("spotify" casa "Spotify" antes de "Spotify Uninstaller").
fn best_match(apps: &[StartApp], query: &str) -> Option<StartApp> {
    let q = fold(query);
    apps.iter()
        .filter_map(|a| {
            let n = fold(&a.name);
            let rank = if n == q {
                0
            } else if n.starts_with(&q) {
                1
            } else if n.contains(&q) {
                2
            } else {
                return None;
            };
            Some((rank, n.len(), a))
        })
        .min_by_key(|(rank, len, _)| (*rank, *len))
        .map(|(_, _, a)| a.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str) -> StartApp {
        StartApp { name: name.into(), app_id: name.into() }
    }

    #[test]
    fn best_match_prefers_exact_then_shortest() {
        let apps = vec![app("Spotify Uninstaller"), app("Spotify"), app("Visual Studio Code")];
        assert_eq!(best_match(&apps, "spotify").unwrap().name, "Spotify");
        assert_eq!(best_match(&apps, "code").unwrap().name, "Visual Studio Code");
        assert!(best_match(&apps, "discord").is_none());
    }

    #[test]
    fn search_urls_per_kind() {
        let u = search_url("anime girl icon", "imagens").unwrap();
        assert!(u.starts_with("https://www.google.com/search?q=anime+girl+icon"));
        assert!(u.ends_with("&tbm=isch"));
        assert!(!search_url("x", "web").unwrap().contains("tbm"));
        assert!(search_url("x", "videos").unwrap().ends_with("tbm=vid"));
        assert!(search_url("x", "holograma").is_err());
        // Caracteres especiais vão codificados, nunca crus na URL.
        assert!(search_url("a&b=c", "web").unwrap().contains("q=a%26b%3Dc"));
    }

    #[test]
    fn only_known_browsers_can_be_launched() {
        assert_eq!(crate::browsers::known_id("Waterfox"), Some("waterfox"));
        assert_eq!(crate::browsers::known_id("waterfox.exe"), Some("waterfox"));
        assert_eq!(crate::browsers::known_id("Microsoft Edge"), Some("msedge"));
        for bad in ["calc", "cmd", "powershell", "notepad", "../../evil", "chrome2", ""] {
            assert_eq!(crate::browsers::known_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn urls_cannot_smuggle_browser_flags() {
        let u = normalize_url("https://a.com/x --gpu-launcher=calc").unwrap();
        assert!(!u.contains(' '), "{u}");
        assert!(u.contains("%20--gpu-launcher"));
        assert!(normalize_url("https://a.com/\"x").unwrap().contains("%22"));
        assert!(normalize_url("file:///C:/x.exe").is_err());
        assert!(normalize_url("javascript:alert(1)").is_err());
        assert!(normalize_url("--gpu-launcher=calc").is_err());
        assert_eq!(quote_args(&["-new-tab".into(), "https://a.com/".into()]).unwrap(), "\"-new-tab\" \"https://a.com/\"");
        assert!(quote_args(&["a\"b".into()]).is_err());
    }

    #[test]
    fn new_tab_args_by_browser_family() {
        use crate::browsers::{Family, Mode};
        use std::path::Path;
        let url = "https://x.y";
        assert_eq!(Family::Firefox.open_args(Path::new("waterfox.exe"), Mode::Tab, url), vec!["-new-tab", url]);
        assert_eq!(Family::Chromium.open_args(Path::new("chrome.exe"), Mode::Tab, url), vec![url]);
    }

    #[test]
    fn plain_app_names_only() {
        for ok in ["chrome", "code.exe", "Visual Studio Code", "obs64"] {
            assert!(is_plain_app_name(ok), "{ok}");
        }
        for bad in [
            r"C:\Users\x\evil.exe",
            r"..\evil.exe",
            r"\\server\share\x.exe",
            "evil.bat",
            "script.ps1",
            "file:///C:/x.exe",
            "cmd /c calc",
            "x.lnk",
            ".hidden",
        ] {
            assert!(!is_plain_app_name(bad), "{bad}");
        }
    }

    #[test]
    fn only_documents_open_without_run() {
        use std::path::Path;
        assert!(is_document(Path::new(r"C:\a\relatorio.PDF")));
        assert!(is_document(Path::new("foto.jpg")));
        for exe in ["x.exe", "x.bat", "x.cmd", "x.ps1", "x.vbs", "x.hta", "x.lnk", "x.msi", "x.scr", "x.url", "x.reg", "x"] {
            assert!(!is_document(Path::new(exe)), "{exe}");
        }
    }

    #[test]
    fn aliases_ignore_accents() {
        assert_eq!(alias("Configurações"), Some("ms-settings:"));
        assert_eq!(alias("Bloco de Notas"), Some("notepad.exe"));
    }
}
