//! `app.install`: instalar programas pelo winget. O código decide o pacote
//! (busca + escolha por regras) ANTES da aprovação — a prévia mostra nome, id
//! e versão, e é exatamente esse id que instala. Nada vindo do modelo vai
//! para a linha de comando além do id validado.

use super::{hidden_command, tidy::Prepared};
use crate::WinError;
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Package {
    pub name: String,
    pub id: String,
    pub version: String,
}

/// Tabela do `winget search`/`winget list`: as colunas começam onde começa
/// cada palavra do cabeçalho (a linha antes dos tracinhos). Funciona com o
/// cabeçalho em qualquer idioma.
pub(crate) fn parse_table(out: &str) -> Vec<Package> {
    let lines: Vec<&str> = out.lines().map(|l| l.trim_end_matches('\r')).collect();
    let Some(dash) = lines.iter().position(|l| l.trim_start().starts_with("----")) else { return Vec::new() };
    let Some(header) = dash.checked_sub(1).and_then(|i| lines.get(i)) else { return Vec::new() };
    // O winget desenha um "spinner" com \r antes do cabeçalho: fica só o fim.
    let header: Vec<char> = header.rsplit('\r').next().unwrap_or(header).chars().collect();
    let starts: Vec<usize> = (0..header.len()).filter(|&i| header[i] != ' ' && (i == 0 || header[i - 1] == ' ')).collect();
    if starts.len() < 3 {
        return Vec::new();
    }
    let cell = |row: &[char], c: usize| -> String {
        let from = starts[c].min(row.len());
        let to = starts.get(c + 1).copied().unwrap_or(row.len()).min(row.len());
        row[from..to].iter().collect::<String>().trim().to_string()
    };
    lines[dash + 1..]
        .iter()
        .map(|l| l.chars().collect::<Vec<char>>())
        .filter(|row| row.len() > starts[1])
        .map(|row| Package { name: cell(&row, 0), id: cell(&row, 1), version: cell(&row, 2) })
        .filter(|p| !p.name.is_empty() && valid_id(&p.id))
        .collect()
}

/// Id de pacote do winget: "Google.Chrome", "9NKSQGP7F2NH" (Store).
pub(crate) fn valid_id(id: &str) -> bool {
    id.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && id.len() <= 128 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

fn fold(s: &str) -> String {
    s.to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Palavras inteiras de um nome ou id ("Google.Chrome.Beta" → google, chrome, beta).
fn tokens(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(str::to_string).collect()
}

/// O pacote pedido: nome (ou id) igual; senão o de nome mais curto que tem
/// todas as palavras do pedido como palavras INTEIRAS ("chrome" → "Google
/// Chrome"; nunca "xpchrome", nem "Google Chrome Beta"). Sem casamento assim,
/// nenhum — melhor dizer que não achou do que instalar outra coisa.
pub(crate) fn pick<'a>(query: &str, pkgs: &'a [Package]) -> Option<&'a Package> {
    let q = fold(query);
    if let Some(p) = pkgs.iter().find(|p| fold(&p.name) == q || p.id.eq_ignore_ascii_case(query.trim())) {
        return Some(p);
    }
    let words = tokens(&q);
    pkgs.iter()
        .filter(|p| {
            let mut have = tokens(&p.name);
            have.extend(tokens(&p.id));
            !words.is_empty() && words.iter().all(|w| have.contains(w))
        })
        .min_by_key(|p| p.name.chars().count())
}

async fn winget(args: &[&str]) -> Result<String, WinError> {
    let out = hidden_command("winget")
        .args(args)
        .args(["--accept-source-agreements", "--disable-interactivity"])
        .output()
        .await
        .map_err(|_| WinError::Unavailable("winget (Instalador de Aplicativos da Microsoft Store)".into()))?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Busca e escolhe o pacote; já instalado = responde sem perguntar.
pub async fn prepare(params: &Value) -> Result<Prepared, WinError> {
    let query = params["app"].as_str().map(str::trim).filter(|s| !s.is_empty() && s.len() <= 80).ok_or_else(|| WinError::InvalidParams("parâmetro 'app' obrigatório".into()))?;
    let found = parse_table(&winget(&["search", "--name", query, "--source", "winget"]).await?);
    let pkg = pick(query, &found).cloned().ok_or_else(|| WinError::NotFound(format!("'{query}' no winget")))?;
    let installed = parse_table(&winget(&["list", "--id", &pkg.id, "--exact"]).await?);
    if installed.iter().any(|p| p.id.eq_ignore_ascii_case(&pkg.id)) {
        return Ok(Prepared::Nothing(json!({ "already_installed": true, "name": pkg.name, "id": pkg.id })));
    }
    let preview = json!({
        "kind": "install",
        "title": format!("Instalar {}", pkg.name),
        "summary": format!("{} ({}), versão {}, pelo winget da Microsoft.", pkg.name, pkg.id, pkg.version),
        "groups": [],
        "notes": ["O Windows pode pedir permissão de administrador durante a instalação."],
        "confirm": "Instalar",
    });
    Ok(Prepared::Ready { params: json!({ "id": pkg.id, "name": pkg.name }), preview })
}

/// Instala o id que o `prepare` escolheu (e a aprovação mostrou).
pub async fn install(params: Value) -> Result<Value, WinError> {
    let id = params["id"].as_str().filter(|id| valid_id(id)).ok_or_else(|| WinError::InvalidParams("pacote não escolhido".into()))?;
    let name = params["name"].as_str().unwrap_or(id).to_string();
    let out = hidden_command("winget")
        .args(["install", "--id", id, "--exact", "--source", "winget", "--silent"])
        .args(["--accept-package-agreements", "--accept-source-agreements", "--disable-interactivity"])
        .output()
        .await
        .map_err(|_| WinError::Unavailable("winget".into()))?;
    if out.status.success() {
        return Ok(json!({ "installed": name, "id": id }));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let last = text.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("").to_string();
    Err(WinError::Failed(format!("a instalação de {name} não terminou: {last}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEARCH: &str = "\r   - \r   \\ \rName                                   Id                                 Version\r\n\
---------------------------------------------------------------------------------------\r\n\
Google Chrome                          Google.Chrome                      156.0.8078.48\r\n\
Google Chrome Beta                     Google.Chrome.Beta                 157.0.8103.15\r\n\
Google Chrome (EXE)                    Google.Chrome.EXE                  156.0.8078.48\r\n";

    #[test]
    fn parses_the_winget_table() {
        let pkgs = parse_table(SEARCH);
        assert_eq!(pkgs.len(), 3);
        assert_eq!(pkgs[0], Package { name: "Google Chrome".into(), id: "Google.Chrome".into(), version: "156.0.8078.48".into() });
        // Cabeçalho em português também.
        let pt = "Nome      ID               Versão\n----------------------------------\nDiscord   Discord.Discord  1.0.9\n";
        assert_eq!(parse_table(pt)[0].id, "Discord.Discord");
        assert!(parse_table("Nenhum pacote encontrado.").is_empty());
    }

    #[test]
    fn picks_the_plain_package() {
        let pkgs = parse_table(SEARCH);
        assert_eq!(pick("chrome", &pkgs).unwrap().id, "Google.Chrome");
        assert_eq!(pick("Google Chrome", &pkgs).unwrap().id, "Google.Chrome");
        assert_eq!(pick("chrome beta", &pkgs).unwrap().id, "Google.Chrome.Beta");
        assert!(pick("firefox", &pkgs).is_none());
        // Caso real: "chrome" escolhia "xpchrome" (substring).
        let xp = vec![Package { name: "xpchrome".into(), id: "weolar.xpchrome".into(), version: "1".into() }];
        assert!(pick("chrome", &xp).is_none());
    }

    #[test]
    fn ids_are_validated() {
        assert!(valid_id("Google.Chrome") && valid_id("9NKSQGP7F2NH"));
        assert!(!valid_id("x & calc") && !valid_id("--override") && !valid_id(""));
    }
}

#[cfg(test)]
mod live {
    /// `cargo test -p lilith-win install::live -- --ignored --nocapture` (só busca, não instala).
    #[tokio::test]
    #[ignore]
    async fn prepares_real_packages() {
        for app in ["chrome", "waterfox", "discord"] {
            match super::prepare(&serde_json::json!({ "app": app })).await {
                Ok(super::Prepared::Ready { params, preview }) => println!("{app}: PRONTO {params} | {}", preview["summary"]),
                Ok(super::Prepared::Nothing(v)) => println!("{app}: JÁ INSTALADO {v}"),
                Err(e) => println!("{app}: ERRO {e}"),
            }
        }
    }
}
