//! Modelo geral de navegadores: duas famílias (Firefox e Chromium) com o
//! mesmo comportamento por dentro, e o navegador PADRÃO do usuário como
//! escolha quando ele não cita nenhum.
//!
//! - Família Firefox: Firefox, Waterfox, LibreWolf, Floorp, Zen…
//! - Família Chromium: Chrome, Edge, Brave, Opera, Vivaldi, Chromium, Arc…
//!
//! O que muda entre as famílias são os argumentos de linha de comando (nova
//! aba, nova janela, janela privada). Os atalhos de teclado das abas são os
//! mesmos nas duas.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Firefox,
    Chromium,
}

#[derive(Debug, PartialEq)]
pub struct Browser {
    pub id: &'static str,
    pub family: Family,
}

const KNOWN: &[Browser] = &[
    Browser { id: "firefox", family: Family::Firefox },
    Browser { id: "waterfox", family: Family::Firefox },
    Browser { id: "librewolf", family: Family::Firefox },
    Browser { id: "floorp", family: Family::Firefox },
    Browser { id: "zen", family: Family::Firefox },
    Browser { id: "chrome", family: Family::Chromium },
    Browser { id: "msedge", family: Family::Chromium },
    Browser { id: "brave", family: Family::Chromium },
    Browser { id: "opera", family: Family::Chromium },
    Browser { id: "vivaldi", family: Family::Chromium },
    Browser { id: "chromium", family: Family::Chromium },
    Browser { id: "arc", family: Family::Chromium },
];

/// Id do navegador conhecido (ex.: "msedge" para "Microsoft Edge").
pub fn known_id(name: &str) -> Option<&'static str> {
    known(name).map(|b| b.id)
}

pub fn ids() -> Vec<&'static str> {
    KNOWN.iter().map(|b| b.id).collect()
}

/// Navegador conhecido pelo nome que o usuário (ou o modelo) usou.
pub fn known(name: &str) -> Option<&'static Browser> {
    let n = crate::paths::fold(name);
    let n = n.trim().trim_end_matches(".exe");
    let n = match n {
        "edge" | "microsoft edge" => "msedge",
        "google chrome" => "chrome",
        "zen browser" => "zen",
        other => other,
    };
    KNOWN.iter().find(|b| b.id == n)
}

/// Família pelo nome do executável (ex.: "waterfox", "msedge").
pub fn family_of_exe(stem: &str) -> Option<Family> {
    let s = crate::paths::fold(stem);
    KNOWN.iter().find(|b| s == b.id || s.starts_with(b.id)).map(|b| b.family)
}

/// Família pelo ProgId do Windows (ex.: "FirefoxURL-308046B0AF4A39CB",
/// "ChromeHTML", "MSEdgeHTM"). Desconhecido com cara de navegador → Chromium.
pub fn family_of_progid(progid: &str) -> Option<Family> {
    let p = progid.to_lowercase();
    if ["firefox", "waterfox", "librewolf", "floorp", "zen"].iter().any(|f| p.contains(f)) {
        Some(Family::Firefox)
    } else if ["chrome", "edge", "brave", "opera", "vivaldi", "chromium", "arc"].iter().any(|f| p.contains(f))
        || p.ends_with("htm")
        || p.ends_with("html")
    {
        Some(Family::Chromium)
    } else {
        None
    }
}

/// O navegador padrão do usuário.
#[derive(Debug, Clone, PartialEq)]
pub struct DefaultBrowser {
    pub exe: PathBuf,
    pub family: Family,
    /// Nome curto (stem do executável), ex.: "waterfox".
    pub id: String,
}

/// Lê no registro qual programa abre links https e onde ele está. Funciona
/// para forks que não estão na lista (a família vem do ProgId).
pub fn default_browser() -> Option<DefaultBrowser> {
    let progid = reg_query(
        r"HKCU\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\https\UserChoice",
        &["/v", "ProgId"],
    )
    .and_then(|o| parse_reg_value(&o))?;
    let command = reg_query(&format!(r"HKCR\{progid}\shell\open\command"), &["/ve"])
        .and_then(|o| parse_reg_value(&o))?;
    let exe = exe_from_command(&command)?;
    let id = exe.file_stem()?.to_string_lossy().to_lowercase();
    let family = family_of_progid(&progid).or_else(|| family_of_exe(&id))?;
    Some(DefaultBrowser { exe, family, id })
}

pub(crate) fn reg_query(key: &str, args: &[&str]) -> Option<String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("reg")
        .arg("query")
        .arg(key)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Valor de uma linha do `reg query`: "    ProgId    REG_SZ    ChromeHTML".
pub(crate) fn parse_reg_value(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|l| l.split_once("REG_SZ").or_else(|| l.split_once("REG_EXPAND_SZ")))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Executável de uma linha de comando do registro:
/// `"C:\Program Files\X\x.exe" -osint -url "%1"` ou `C:\X\x.exe --flag`.
pub(crate) fn exe_from_command(command: &str) -> Option<PathBuf> {
    let c = command.trim();
    let path = if let Some(rest) = c.strip_prefix('"') {
        rest.split('"').next()?.to_string()
    } else {
        // Sem aspas: até o primeiro ".exe".
        let end = c.to_lowercase().find(".exe")? + 4;
        c[..end].to_string()
    };
    let p = PathBuf::from(path);
    (p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) && p.is_file()).then_some(p)
}

/// Como abrir uma URL nesse navegador.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Nova aba na janela que já está aberta (padrão).
    Tab,
    /// Janela nova.
    Window,
    /// Janela privada/anônima.
    Private,
}

impl Mode {
    pub fn parse(s: Option<&str>) -> Mode {
        match s.map(str::to_lowercase).as_deref() {
            Some("window" | "janela") => Mode::Window,
            Some("private" | "privada" | "incognito" | "anonima" | "anônima") => Mode::Private,
            _ => Mode::Tab,
        }
    }
}

impl Family {
    /// Argumentos de linha de comando para abrir `url` (já normalizada).
    pub fn open_args(self, exe: &Path, mode: Mode, url: &str) -> Vec<String> {
        let stem = exe.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        match (self, mode) {
            (Family::Firefox, Mode::Tab) => vec!["-new-tab".into(), url.into()],
            (Family::Firefox, Mode::Window) => vec!["-new-window".into(), url.into()],
            (Family::Firefox, Mode::Private) => vec!["-private-window".into(), url.into()],
            // Chromium abre numa nova aba da janela existente por padrão.
            (Family::Chromium, Mode::Tab) => vec![url.into()],
            (Family::Chromium, Mode::Window) => vec!["--new-window".into(), url.into()],
            (Family::Chromium, Mode::Private) => {
                let flag = if stem.contains("edge") { "--inprivate" } else { "--incognito" };
                vec![flag.into(), url.into()]
            }
        }
    }
}

/// Operações de aba por teclado — iguais nas duas famílias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabOp {
    New,
    Close,
    Next,
    Previous,
    Reopen,
    Address,
    Find,
    Reload,
}

impl TabOp {
    pub fn parse(s: &str) -> Option<TabOp> {
        Some(match crate::paths::fold(s).as_str() {
            "new" | "nova" | "nova aba" => TabOp::New,
            "close" | "fechar" => TabOp::Close,
            "next" | "proxima" => TabOp::Next,
            "previous" | "prev" | "anterior" => TabOp::Previous,
            "reopen" | "reabrir" => TabOp::Reopen,
            "address" | "endereco" | "url" => TabOp::Address,
            "find" | "localizar" | "buscar" => TabOp::Find,
            "reload" | "recarregar" => TabOp::Reload,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_names_and_families() {
        assert_eq!(known("Waterfox").map(|b| b.family), Some(Family::Firefox));
        assert_eq!(known("Microsoft Edge").map(|b| b.id), Some("msedge"));
        assert_eq!(known("google chrome.exe").map(|b| b.family), Some(Family::Chromium));
        assert_eq!(known("calc"), None);
        assert_eq!(family_of_exe("firefox"), Some(Family::Firefox));
        assert_eq!(family_of_exe("msedge"), Some(Family::Chromium));
        assert_eq!(family_of_exe("notepad"), None);
    }

    #[test]
    fn progid_gives_the_family_even_for_unknown_forks() {
        assert_eq!(family_of_progid("FirefoxURL-308046B0AF4A39CB"), Some(Family::Firefox));
        assert_eq!(family_of_progid("WaterfoxURL-1A2B"), Some(Family::Firefox));
        assert_eq!(family_of_progid("ChromeHTML"), Some(Family::Chromium));
        assert_eq!(family_of_progid("MSEdgeHTM"), Some(Family::Chromium));
        assert_eq!(family_of_progid("BraveHTML"), Some(Family::Chromium));
        // Fork desconhecido, mas com ProgId no padrão "<Nome>HTML".
        assert_eq!(family_of_progid("SomeForkHTML"), Some(Family::Chromium));
        assert_eq!(family_of_progid("AppXabc123"), None);
    }

    #[test]
    fn registry_output_is_parsed() {
        let out = "\r\nHKEY_CURRENT_USER\\...\\UserChoice\r\n    ProgId    REG_SZ    ChromeHTML\r\n    Hash    REG_SZ    abc\r\n";
        assert_eq!(parse_reg_value(out).as_deref(), Some("ChromeHTML"));
        assert_eq!(parse_reg_value("nada aqui"), None);
        // Caminho sem aspas e com aspas, mas o arquivo precisa existir.
        let exe = std::env::current_exe().unwrap();
        let quoted = format!("\"{}\" -osint -url \"%1\"", exe.display());
        assert_eq!(exe_from_command(&quoted), Some(exe.clone()));
        assert_eq!(exe_from_command(&format!("{} --flag", exe.display())), Some(exe));
        assert_eq!(exe_from_command(r#""C:\nao\existe\x.exe" -url "%1""#), None);
        assert_eq!(exe_from_command("sem executavel"), None);
    }

    #[test]
    fn open_args_per_family_and_mode() {
        let url = "https://x.y/";
        let ff = Path::new("firefox.exe");
        assert_eq!(Family::Firefox.open_args(ff, Mode::Tab, url), vec!["-new-tab", url]);
        assert_eq!(Family::Firefox.open_args(ff, Mode::Window, url), vec!["-new-window", url]);
        assert_eq!(Family::Firefox.open_args(ff, Mode::Private, url), vec!["-private-window", url]);
        assert_eq!(Family::Chromium.open_args(Path::new("chrome.exe"), Mode::Tab, url), vec![url]);
        assert_eq!(Family::Chromium.open_args(Path::new("chrome.exe"), Mode::Private, url), vec!["--incognito", url]);
        assert_eq!(Family::Chromium.open_args(Path::new("msedge.exe"), Mode::Private, url), vec!["--inprivate", url]);
        assert_eq!(Mode::parse(Some("privada")), Mode::Private);
        assert_eq!(Mode::parse(None), Mode::Tab);
    }

    #[test]
    fn tab_operations_parse_in_both_languages() {
        assert_eq!(TabOp::parse("nova aba"), Some(TabOp::New));
        assert_eq!(TabOp::parse("Reabrir"), Some(TabOp::Reopen));
        assert_eq!(TabOp::parse("endereço"), Some(TabOp::Address));
        assert_eq!(TabOp::parse("explodir"), None);
    }

    #[test]
    fn default_browser_lookup_does_not_panic() {
        // Lê o registro de verdade; qualquer resultado é válido (pode não haver padrão).
        let _ = default_browser();
    }
}
