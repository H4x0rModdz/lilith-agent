//! Qual pasta de projeto o usuário está usando: pelo título da janela do
//! Visual Studio (nome da solução), do VS Code (nome da pasta), de uma IDE
//! JetBrains (nome do projeto) ou do terminal (caminho no título), e depois
//! achando essa pasta no disco. Mais de uma candidata: o app pergunta qual.

use crate::paths::fold;
use crate::window::{self, WindowInfo};
use regex::Regex;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Tipo de programa de desenvolvimento, pelo nome do processo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevApp {
    VisualStudio,
    VsCode,
    JetBrains,
    Terminal,
}

impl DevApp {
    pub fn label(self) -> &'static str {
        match self {
            DevApp::VisualStudio => "Visual Studio",
            DevApp::VsCode => "VS Code",
            DevApp::JetBrains => "IDE JetBrains",
            DevApp::Terminal => "terminal",
        }
    }
}

pub fn dev_app(process: &str) -> Option<DevApp> {
    match fold(process).trim_end_matches(".exe") {
        "devenv" => Some(DevApp::VisualStudio),
        "code" | "code - insiders" | "cursor" | "windsurf" | "vscodium" => Some(DevApp::VsCode),
        "rider64" | "idea64" | "webstorm64" | "clion64" | "rustrover64" | "pycharm64" | "goland64" => Some(DevApp::JetBrains),
        "windowsterminal" | "cmd" | "powershell" | "pwsh" | "openconsole" | "conhost" | "wezterm-gui" | "alacritty"
        | "mintty" | "tabby" | "hyper" | "warp" => Some(DevApp::Terminal),
        _ => None,
    }
}

/// O que o título da janela diz sobre o projeto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hint {
    /// Caminho absoluto (título do terminal).
    Path(PathBuf),
    /// Nome da solução do Visual Studio (procura `<nome>.sln`).
    Solution(String),
    /// Nome da pasta aberta (VS Code, JetBrains).
    Folder(String),
}

/// Tira estados entre parênteses/colchetes do fim ("(Executando)", "[WSL: x]").
fn strip_states(s: &str) -> String {
    static STATES: OnceLock<Regex> = OnceLock::new();
    let re = STATES.get_or_init(|| Regex::new(r"(?:\s*[\(\[][^\(\)\[\]]*[\)\]])+\s*$").expect("regex válida"));
    re.replace(s.trim(), "").trim().to_string()
}

/// Caminho de Windows no título do terminal ("PS C:\src\demo>", "MINGW64:/c/src/demo").
fn path_in_title(title: &str) -> Option<PathBuf> {
    static WIN: OnceLock<Regex> = OnceLock::new();
    static MINGW: OnceLock<Regex> = OnceLock::new();
    let win = WIN.get_or_init(|| Regex::new(r#"[A-Za-z]:\\[^<>:"|?*\r\n]*"#).expect("regex válida"));
    let mingw = MINGW.get_or_init(|| Regex::new(r"(?:^|[\s:])/([a-zA-Z])/([^\s:]*)").expect("regex válida"));
    let raw = if let Some(m) = win.find(title) {
        m.as_str().trim().trim_end_matches(['>', ' ']).to_string()
    } else {
        let c = mingw.captures(title)?;
        format!("{}:\\{}", c[1].to_uppercase(), c[2].replace('/', "\\"))
    };
    let p = PathBuf::from(raw.trim_end_matches('\\'));
    // Pasta do sistema (o título padrão do cmd é "C:\WINDOWS\system32\cmd.exe").
    (!fold(&p.to_string_lossy()).starts_with("c:\\windows")).then_some(p)
}

/// Projeto pelo título da janela.
pub fn hint_from_window(app: DevApp, title: &str) -> Option<Hint> {
    let title = title.trim().trim_start_matches(['●', '•', '*']).trim();
    let name = match app {
        DevApp::Terminal => return path_in_title(title).map(Hint::Path),
        DevApp::VisualStudio => {
            // "Demo - Microsoft Visual Studio", "Demo (Executando) - Microsoft Visual Studio (Administrador)".
            let (left, _) = title.rsplit_once(" - Microsoft Visual Studio")?;
            let name = strip_states(left);
            return (!name.is_empty()).then_some(Hint::Solution(name));
        }
        DevApp::VsCode => {
            // "arquivo.rs - pasta - Visual Studio Code"; "pasta - Visual Studio Code".
            let parts: Vec<&str> = title.split(" - ").map(str::trim).collect();
            match parts.len() {
                n if n >= 3 => parts[n - 2].to_string(),
                2 if !parts[0].contains('.') => parts[0].to_string(),
                _ => return None,
            }
        }
        // "demo – Program.cs" (travessão) ou "demo - Program.cs".
        DevApp::JetBrains => title.split([' ']).next().filter(|_| title.contains(" – ") || title.contains(" - "))?.to_string(),
    };
    let name = strip_states(&name);
    let name = name.trim_end_matches(" (Workspace)").trim().to_string();
    (!name.is_empty() && name.chars().count() <= 80).then_some(Hint::Folder(name))
}

/// Marcas de raiz de projeto.
fn is_project_root(dir: &Path) -> bool {
    const MARKERS: &[&str] = &[".git", "Cargo.toml", "package.json", "go.mod", "pyproject.toml", "pom.xml", "build.gradle"];
    if MARKERS.iter().any(|m| dir.join(m).exists()) {
        return true;
    }
    std::fs::read_dir(dir).map(|entries| {
        entries.flatten().any(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.ends_with(".sln") || n.ends_with(".slnx") || n.ends_with(".csproj") || n.ends_with(".vbproj") || n.ends_with(".fsproj")
        })
    })
    .unwrap_or(false)
}

/// Sobe a partir de `start` até achar a raiz do projeto (no máximo 6 níveis).
pub fn project_root(start: &Path) -> Option<PathBuf> {
    let start = if start.is_file() { start.parent()? } else { start };
    start.ancestors().take(7).find(|d| d.is_dir() && is_project_root(d)).map(Path::to_path_buf)
}

/// Pasta de uma entrada `workspace.json` do VS Code ("folder": "file:///c%3A/src/demo").
pub fn folder_from_workspace_json(text: &str) -> Option<PathBuf> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let uri = v["folder"].as_str()?;
    url::Url::parse(uri).ok().filter(|u| u.scheme() == "file")?.to_file_path().ok()
}

/// Pastas abertas recentemente no VS Code (e forks), pelo `workspaceStorage`.
fn vscode_recent_folders() -> Vec<PathBuf> {
    let Some(appdata) = dirs::config_dir() else { return Vec::new() };
    let mut out = Vec::new();
    for product in ["Code", "Code - Insiders", "Cursor", "Windsurf", "VSCodium"] {
        let Ok(entries) = std::fs::read_dir(appdata.join(product).join("User").join("workspaceStorage")) else { continue };
        for e in entries.flatten().take(500) {
            if let Ok(text) = std::fs::read_to_string(e.path().join("workspace.json")) {
                if let Some(p) = folder_from_workspace_json(&text) {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// Onde procurar projetos no disco: a pasta do usuário e as pastas de código comuns.
fn search_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = dirs::home_dir().into_iter().collect();
    for drive in ["C", "D", "E"] {
        for dir in ["dev", "src", "source", "projects", "projetos", "repos", "code", "git", "workspace"] {
            let p = PathBuf::from(format!("{drive}:\\{dir}"));
            if p.is_dir() {
                roots.push(p);
            }
        }
    }
    roots
}

/// Procura no disco (busca do `file.find`) entradas com o nome exato.
async fn find_named(name: &str, want_dir: bool) -> Vec<PathBuf> {
    let wanted = fold(name);
    let mut out = Vec::new();
    for root in search_roots() {
        let params = json!({ "name": name, "root": root, "limit": 100 });
        let Ok(found) = crate::handlers::files::find(params).await else { continue };
        for r in found["results"].as_array().into_iter().flatten() {
            let is_dir = r["dir"].as_bool().unwrap_or(false);
            let Some(path) = r["path"].as_str().map(PathBuf::from) else { continue };
            let base = path.file_name().map(|n| fold(&n.to_string_lossy())).unwrap_or_default();
            if is_dir == want_dir && base == wanted {
                out.push(path);
            }
        }
    }
    out
}

/// Mesmo caminho, sem diferenciar maiúsculas.
fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    paths.into_iter().filter(|p| seen.insert(fold(&p.to_string_lossy()))).collect()
}

/// Pastas de projeto que o título indica.
pub async fn candidates(hint: &Hint) -> Vec<PathBuf> {
    let found: Vec<PathBuf> = match hint {
        Hint::Path(p) => project_root(p).into_iter().collect(),
        Hint::Solution(name) => {
            let mut sln = find_named(&format!("{name}.sln"), false).await;
            sln.extend(find_named(&format!("{name}.slnx"), false).await);
            sln.iter().filter_map(|f| f.parent().map(Path::to_path_buf)).collect()
        }
        Hint::Folder(name) => {
            let wanted = fold(name);
            let recent: Vec<PathBuf> = vscode_recent_folders()
                .into_iter()
                .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| fold(&n.to_string_lossy()) == wanted))
                .collect();
            if recent.is_empty() {
                find_named(name, true).await.into_iter().filter(|p| is_project_root(p)).collect()
            } else {
                recent
            }
        }
    };
    dedup_paths(found)
}

/// Janelas de desenvolvimento abertas, a de cima primeiro.
pub fn dev_windows() -> Vec<(WindowInfo, DevApp)> {
    window::list().into_iter().filter_map(|w| dev_app(&w.app).map(|a| (w, a))).collect()
}

/// Projeto(s) da janela de desenvolvimento em uso (ou da `prefer`, se dada).
/// Olha no máximo três janelas; devolve a primeira que indicou alguma pasta.
pub async fn detect(prefer: Option<isize>) -> (Option<WindowInfo>, Vec<PathBuf>) {
    let mut wins = dev_windows();
    if let Some(id) = prefer {
        wins.sort_by_key(|(w, _)| w.id != id);
    }
    for (w, app) in wins.into_iter().take(3) {
        let Some(hint) = hint_from_window(app, &w.title) else { continue };
        let found = candidates(&hint).await;
        if !found.is_empty() {
            return (Some(w), found);
        }
    }
    (None, Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dev_apps_by_process() {
        assert_eq!(dev_app("devenv"), Some(DevApp::VisualStudio));
        assert_eq!(dev_app("Code"), Some(DevApp::VsCode));
        assert_eq!(dev_app("WindowsTerminal"), Some(DevApp::Terminal));
        assert_eq!(dev_app("pwsh.exe"), Some(DevApp::Terminal));
        assert_eq!(dev_app("rider64"), Some(DevApp::JetBrains));
        assert_eq!(dev_app("chrome"), None);
        assert_eq!(dev_app("WhatsApp"), None);
    }

    #[test]
    fn visual_studio_titles() {
        let vs = |t| hint_from_window(DevApp::VisualStudio, t);
        assert_eq!(vs("Demo - Microsoft Visual Studio"), Some(Hint::Solution("Demo".into())));
        assert_eq!(vs("Demo.Api (Executando) - Microsoft Visual Studio (Administrador)"), Some(Hint::Solution("Demo.Api".into())));
        assert_eq!(vs("Demo - Microsoft Visual Studio Preview"), Some(Hint::Solution("Demo".into())));
        assert_eq!(vs("Microsoft Visual Studio"), None);
    }

    #[test]
    fn vscode_titles() {
        let code = |t| hint_from_window(DevApp::VsCode, t);
        assert_eq!(code("main.rs - demo-app - Visual Studio Code"), Some(Hint::Folder("demo-app".into())));
        assert_eq!(code("● main.rs - demo-app - Visual Studio Code"), Some(Hint::Folder("demo-app".into())));
        assert_eq!(code("demo-app - Visual Studio Code"), Some(Hint::Folder("demo-app".into())));
        assert_eq!(code("main.rs - demo [WSL: Ubuntu] - Visual Studio Code"), Some(Hint::Folder("demo".into())));
        assert_eq!(code("main.rs - Visual Studio Code"), None, "sem pasta aberta");
        assert_eq!(code("Visual Studio Code"), None);
    }

    #[test]
    fn terminal_titles() {
        let term = |t| hint_from_window(DevApp::Terminal, t);
        assert_eq!(term("PS C:\\src\\demo> "), Some(Hint::Path(PathBuf::from("C:\\src\\demo"))));
        assert_eq!(term("C:\\src\\demo\\"), Some(Hint::Path(PathBuf::from("C:\\src\\demo"))));
        assert_eq!(term("MINGW64:/c/src/demo"), Some(Hint::Path(PathBuf::from("C:\\src\\demo"))));
        assert_eq!(term("Administrador: C:\\WINDOWS\\system32\\cmd.exe"), None);
        assert_eq!(term("Windows PowerShell"), None);
    }

    #[test]
    fn vscode_workspace_entries() {
        let p = folder_from_workspace_json(r#"{ "folder": "file:///c%3A/src/demo%20app" }"#).unwrap();
        assert_eq!(p, PathBuf::from("c:\\src\\demo app"));
        assert_eq!(folder_from_workspace_json(r#"{ "folder": "vscode-remote://wsl+ubuntu/home/x" }"#), None);
        assert_eq!(folder_from_workspace_json(r#"{ "workspace": "file:///c%3A/x.code-workspace" }"#), None);
    }

    #[test]
    fn project_root_walks_up_to_a_marker() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Demo.sln"), "").unwrap();
        let deep = dir.path().join("src").join("Demo.Api");
        std::fs::create_dir_all(&deep).unwrap();
        assert_eq!(project_root(&deep).as_deref(), Some(dir.path()));
        let file = deep.join("Program.cs");
        std::fs::write(&file, "").unwrap();
        assert_eq!(project_root(&file).as_deref(), Some(dir.path()));
    }
}
