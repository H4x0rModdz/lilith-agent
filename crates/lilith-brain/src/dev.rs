//! Ajudante de desenvolvimento decidido por código: frases curtas ("quais
//! erros tem no build?", "como tá o git do projeto?", "corrige os erros do
//! build") viram a chamada direta (sem modelo), a resposta é montada aqui a
//! partir do resultado, e o plano de pedidos livres é corrigido em [`plan`].
//! O modelo só entra para EXPLICAR um erro ("explica esse erro").

use crate::shortcut::Shortcut;
use crate::tools::ToolCall;
use regex::Regex;
use serde_json::{json, Value};
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("regex do ajudante de dev válida"))
}

fn call(action: &str, params: Value) -> Shortcut {
    Shortcut { calls: vec![ToolCall { action: action.into(), params }], reply: "Pronto!".into() }
}

/// Onde o usuário disse que está o build.
const TARGET: &str = r"(?:o\s+|a\s+)?(?:build|compila[cç][aã]o|projeto|solu[cç][aã]o|visual\s+studio|vs\s*code|vscode|vs|terminal|c[oó]digo)";

/// Janela citada ("no terminal", "no visual studio") → alvo do `dev.errors`.
fn target_in(t: &str) -> Option<&'static str> {
    if t.contains("vs code") || t.contains("vscode") {
        Some("code")
    } else if t.contains("visual studio") || t.split_whitespace().any(|w| w == "vs") {
        Some("devenv")
    } else if t.contains("terminal") {
        Some("terminal")
    } else {
        None
    }
}

/// Atalhos do ajudante de dev. `t` já vem normalizado (minúsculas, sem "?").
pub fn shortcut(t: &str) -> Option<Shortcut> {
    static ERRORS: OnceLock<Regex> = OnceLock::new();
    static GIT: OnceLock<Regex> = OnceLock::new();
    static BRANCH: OnceLock<Regex> = OnceLock::new();
    static FIX: OnceLock<Regex> = OnceLock::new();

    let errors = re(
        &ERRORS,
        &format!(
            r"^(?:(?:quais|que)\s+(?:s[aã]o\s+)?(?:os\s+)?erros?\s+(?:(?:que\s+)?(?:tem|t[eê]m|t[aá]|est[aá]|deu|deram|h[aá]|aparece|aparecem|apareceu|apareceram)\s+)?(?:n[oa]|d[oa])\s+{TARGET}|(?:mostra|lista|l[eê]|v[eê]|olha)\s+(?:os\s+|esses\s+)?erros\s+(?:d[oa]|n[oa])\s+{TARGET}|(?:o\s+)?que\s+(?:t[aá]|est[aá])\s+quebrado(?:\s+n[oa]\s+{TARGET})?|(?:por\s*que|pq|porque)\s+{TARGET}\s+(?:n[aã]o\s+(?:compila|builda|roda|sobe)|(?:t[aá]\s+|est[aá]\s+)?(?:falhou|quebrou|falhando|quebrado))|(?:deu\s+|tem\s+)?erros?\s+(?:n[oa]|d[oa])\s+{TARGET})$"
        ),
    );
    if errors.is_match(t) {
        let mut params = json!({ "offer_fix": true });
        if let Some(target) = target_in(t) {
            params["target"] = json!(target);
        }
        return Some(call("dev.errors", params));
    }

    let fix = re(
        &FIX,
        &format!(
            r"^(?:(?:pede|manda)\s+(?:pro|pra|para\s+o|o)\s+(?P<agent>claude(?:\s+code)?|codex|agente)\s+(?:corrigir|consertar|arrumar|resolver)|corrige|corrija|conserta|arruma|resolve|(?:pode\s+)?corrigir)\s+(?:os\s+|esses\s+|estes\s+|esse\s+|este\s+|o\s+|a\s+)?(?:erros?|build)(?:\s+(?:d[oa]|n[oa])\s+{TARGET})?$"
        ),
    );
    if let Some(c) = fix.captures(t) {
        let mut params = json!({});
        match c.name("agent").map(|m| m.as_str()) {
            Some(a) if a.starts_with("claude") => params["agent"] = json!("claude"),
            Some("codex") => params["agent"] = json!("codex"),
            _ => {}
        }
        if let Some(target) = target_in(t) {
            params["target"] = json!(target);
        }
        return Some(call("dev.fix", params));
    }

    let git = re(
        &GIT,
        r"^(?:como\s+(?:t[aá]|est[aá]|anda)\s+o\s+git|(?:qual\s+[eé]\s+o\s+)?status\s+do\s+git|git\s+status|resumo\s+do\s+git|o\s+que\s+mudou\s+no\s+git)(?:\s+d[oa]\s+(?:projeto|reposit[oó]rio|repo)(?:\s+(?P<n1>[\p{L}0-9._\-]+))?|\s+d[oa]\s+(?P<n2>[\p{L}0-9._\-]+))?$",
    );
    if let Some(c) = git.captures(t) {
        let name = c.name("n1").or(c.name("n2")).map(|m| m.as_str()).filter(|n| !matches!(*n, "pc" | "computador" | "meu"));
        let params = match name {
            Some(n) => json!({ "project": n }),
            None => json!({}),
        };
        return Some(call("dev.git_status", params));
    }
    let branch = re(&BRANCH, r"^(?:em\s+)?(?:qual|que)\s+branch\s+(?:eu\s+)?(?:t[oô]|estou|t[aá]|est[aá])(?:\s+no\s+projeto)?$");
    if branch.is_match(t) {
        return Some(call("dev.git_status", json!({})));
    }
    None
}

fn has(t: &str, words: &[&str]) -> bool {
    words.iter().any(|w| t.contains(w))
}

/// Plano para pedidos livres sobre build/git (o intérprete erra por pouco:
/// manda ler a janela ou delegar sem os erros). `None` = não é sobre isso.
pub fn plan(t: &str, recent: &[(String, String)]) -> Option<Vec<String>> {
    let about_build = has(t, &["build", "compila", "compilar", "lista de erros", "não compila", "nao compila"])
        || recent.iter().any(|(_, r)| r.contains("de build"));
    let errors = has(t, &["erro", "quebr"]);
    if errors && about_build && !has(t, &["vulnerab"]) {
        if has(t, &["corrig", "consert", "arrum", "resolv", "repar"]) {
            return Some(vec!["dev.fix".into()]);
        }
        if has(t, &["explica", "explique", "significa", "quer dizer", "quais", "mostra", "lista", "o que"]) {
            return Some(vec!["dev.errors".into()]);
        }
    }
    let git = t.split(|c: char| !c.is_alphanumeric()).any(|w| w == "git");
    let changes_git = has(t, &["commita", "commit -", "faz commit", "faz um commit", "push", "pull", "merge", "checkout", "rebase", "stash", "reset"]);
    if git && !changes_git && has(t, &["status", "branch", "commit", "mudou", "mudança", "mudanças", "como tá", "como está", "resumo", "pendente", "modificad"]) {
        return Some(vec!["dev.git_status".into()]);
    }
    None
}

fn plural(n: u64, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// "no Visual Studio", "no terminal", "na janela".
fn in_source(source: &str) -> String {
    match source {
        "" => "na janela".into(),
        "janela" | "IDE JetBrains" => format!("na {source}"),
        s => format!("no {s}"),
    }
}

fn agent_label(agent: &str) -> &str {
    match agent {
        "claude" => "Claude Code",
        "codex" => "Codex",
        other => other,
    }
}

/// "CS0103 em Program.cs:12 — mensagem".
fn error_line(e: &Value) -> String {
    let file = e["file"].as_str().map(|f| f.rsplit(['\\', '/']).next().unwrap_or(f).to_string());
    let place = match (file, e["line"].as_u64()) {
        (Some(f), Some(l)) => format!(" em {f}:{l}"),
        (Some(f), None) => format!(" em {f}"),
        _ => String::new(),
    };
    let code = e["code"].as_str().unwrap_or("erro");
    format!("- {code}{place} — {}", e["message"].as_str().unwrap_or("").trim())
}

/// "Pedi para o Claude Code corrigir…" a partir do job criado.
fn delegated(r: &Value) -> String {
    let n = r["error_count"].as_u64().unwrap_or(0);
    let what = if n > 0 { format!(" {}", plural(n, "erro", "erros")) } else { " os erros".into() };
    format!(
        "Pedi para o {} corrigir{what} em {} (tarefa #{}). Ele não mexe no git e roda o build/testes no fim; acompanhe no painel — só te digo que corrigiu quando ele terminar e contar o que mudou.",
        agent_label(r["agent"].as_str().unwrap_or("agente")),
        r["folder"].as_str().unwrap_or("?"),
        r["job_id"],
    )
}

const ONLY_VISIBLE: &str =
    "Só enxergo o que está visível: se a Lista de Erros ou a Saída estiver fechada ou rolada, abre ela e me pede de novo.";

/// Resposta montada pelo código a partir do resultado (sem modelo).
pub fn render(action: &str, r: &Value) -> Option<String> {
    match action {
        "dev.errors" => {
            let n = r["error_count"].as_u64().unwrap_or(0);
            let w = r["warning_count"].as_u64().unwrap_or(0);
            let source = in_source(r["source"].as_str().unwrap_or(""));
            let warn = if w > 0 { format!(" (e {})", plural(w, "aviso", "avisos")) } else { String::new() };
            if n == 0 {
                return Some(format!("Não vi erros de build {source}{warn}. {ONLY_VISIBLE}"));
            }
            let list: Vec<Value> = r["errors"].as_array().cloned().unwrap_or_default();
            let mut lines = vec![format!("Achei {} de build {source}{warn}:", plural(n, "erro", "erros"))];
            lines.extend(list.iter().take(5).map(error_line));
            if n > 5 {
                lines.push(format!("- … e mais {}.", n - 5));
            }
            let fix = &r["fix"];
            if fix["job_id"].is_u64() {
                lines.push(delegated(fix));
            } else if let Some(why) = fix["error"].as_str() {
                lines.push(format!("Não pedi a correção: {why}."));
            } else if r["fix_declined"] != true {
                lines.push("Posso pedir para um agente corrigir (\"corrige os erros do build\") ou explicar algum (\"explica esse erro\").".into());
            }
            Some(lines.join("\n"))
        }
        "dev.git_status" => {
            if r["cancelled"] == true {
                return Some("Tudo bem, não olhei o git.".into());
            }
            let num = |k: &str| r[k].as_u64().unwrap_or(0);
            let branch = match r["branch"].as_str() {
                Some(b) => format!("branch {b}"),
                None => "HEAD destacado (fora de um branch)".into(),
            };
            let sync = match r["upstream"].as_str() {
                None => ", sem branch remoto configurado".to_string(),
                Some(up) if num("ahead") == 0 && num("behind") == 0 => format!(", em dia com {up}"),
                Some(up) => {
                    let mut parts = Vec::new();
                    if num("ahead") > 0 {
                        parts.push(format!("{} à frente", plural(num("ahead"), "commit", "commits")));
                    }
                    if num("behind") > 0 {
                        parts.push(format!("{} atrás", plural(num("behind"), "commit", "commits")));
                    }
                    format!(", {} do {up}", parts.join(" e "))
                }
            };
            let mut out = format!("Git do {}: {branch}{sync}.", r["repo"].as_str().unwrap_or("projeto"));
            if r["clean"] == true {
                out.push_str(" Nada modificado.");
            } else {
                let mut parts = Vec::new();
                for (k, one, many) in [
                    ("modified", "arquivo modificado", "arquivos modificados"),
                    ("staged", "no stage", "no stage"),
                    ("untracked", "arquivo novo (não rastreado)", "arquivos novos (não rastreados)"),
                    ("conflicts", "com conflito", "com conflito"),
                ] {
                    if num(k) > 0 {
                        parts.push(plural(num(k), one, many));
                    }
                }
                let lines = if num("insertions") + num("deletions") > 0 {
                    format!(" (+{} −{} linhas)", num("insertions"), num("deletions"))
                } else {
                    String::new()
                };
                out.push_str(&format!(" Mudanças: {}{lines}.", parts.join(", ")));
            }
            let commits: Vec<String> =
                r["last_commits"].as_array().into_iter().flatten().filter_map(|c| c.as_str()).take(3).map(|c| format!("“{c}”")).collect();
            if !commits.is_empty() {
                out.push_str(&format!(" Últimos commits: {}.", commits.join("; ")));
            }
            Some(out)
        }
        "dev.fix" => Some(if r["cancelled"] == true {
            "Tudo bem, não pedi nada.".into()
        } else if r["no_errors"] == true {
            format!("Não vi erros de build para corrigir {}. {ONLY_VISIBLE}", in_source(r["source"].as_str().unwrap_or("")))
        } else {
            delegated(r)
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shortcut::parse;

    fn one(text: &str) -> Option<(String, Value)> {
        parse(text).map(|s| (s.calls[0].action.clone(), s.calls[0].params.clone()))
    }

    #[test]
    fn build_error_phrases() {
        for t in [
            "quais erros tem no build?",
            "Quais são os erros do build",
            "que erros deu na compilação",
            "mostra os erros do projeto",
            "o que tá quebrado?",
            "por que o build não compila",
            "por que o projeto quebrou",
            "erros do build",
            "tem erro no build?",
        ] {
            assert_eq!(one(t), Some(("dev.errors".into(), json!({ "offer_fix": true }))), "{t}");
        }
        assert_eq!(one("quais erros tem no terminal").unwrap().1, json!({ "offer_fix": true, "target": "terminal" }));
        assert_eq!(one("quais erros tem no visual studio").unwrap().1["target"], "devenv");
        assert_eq!(one("mostra os erros do vs code").unwrap().1["target"], "code");
    }

    #[test]
    fn fix_phrases() {
        assert_eq!(one("corrige os erros do build"), Some(("dev.fix".into(), json!({}))));
        assert_eq!(one("corrige esses erros"), Some(("dev.fix".into(), json!({}))));
        assert_eq!(one("conserta o build por favor").unwrap().0, "dev.fix");
        assert_eq!(one("pede pro codex corrigir os erros").unwrap().1, json!({ "agent": "codex" }));
        assert_eq!(one("manda o claude code consertar o build").unwrap().1, json!({ "agent": "claude" }));
        assert_eq!(one("corrige os erros do terminal").unwrap().1, json!({ "target": "terminal" }));
        // Vulnerabilidade tem caminho próprio (refine + delegate.rs).
        assert_eq!(one("corrige a vulnerabilidade"), None);
    }

    #[test]
    fn git_phrases() {
        assert_eq!(one("como tá o git do projeto?"), Some(("dev.git_status".into(), json!({}))));
        assert_eq!(one("status do git"), Some(("dev.git_status".into(), json!({}))));
        assert_eq!(one("git status").unwrap().0, "dev.git_status");
        assert_eq!(one("em qual branch eu tô").unwrap().0, "dev.git_status");
        assert_eq!(one("como está o git do projeto demo-app").unwrap().1, json!({ "project": "demo-app" }));
        assert_eq!(one("como tá o git do demo").unwrap().1, json!({ "project": "demo" }));
        // Mudar o repositório nunca é atalho.
        for t in ["faz commit de tudo", "dá um git push", "git pull", "faz checkout da main"] {
            assert_eq!(one(t), None, "{t}");
        }
    }

    #[test]
    fn free_text_plans() {
        let p = |t: &str| plan(t, &[]);
        assert_eq!(p("me explica o erro do build"), Some(vec!["dev.errors".to_string()]));
        assert_eq!(p("pede pro claude arrumar o que quebrou no build"), Some(vec!["dev.fix".to_string()]));
        assert_eq!(p("o que tem de mudança no git do projeto?"), Some(vec!["dev.git_status".to_string()]));
        assert_eq!(p("faz commit no git e push"), None, "git só leitura");
        assert_eq!(p("corrige a vulnerabilidade do build"), None, "vulnerabilidade tem regra própria");
        assert_eq!(p("abre o spotify"), None);
        // "esse erro" vale pelo contexto da conversa.
        let recent = vec![("quais erros tem no build".to_string(), "Achei 2 erros de build no terminal:".to_string())];
        assert_eq!(plan("explica esse erro", &recent), Some(vec!["dev.errors".to_string()]));
        assert_eq!(plan("explica esse erro", &[]), None);
    }

    #[test]
    fn error_replies_are_rendered() {
        let r = json!({
            "source": "Visual Studio", "error_count": 7, "warning_count": 1,
            "errors": [
                { "code": "CS0103", "file": "C:\\src\\Demo\\Program.cs", "line": 12, "message": "The name 'foo' does not exist" },
                { "code": "TS2304", "file": "src/app.ts", "message": "Cannot find name 'x'." },
                { "message": "linker failed" },
            ],
        });
        let out = render("dev.errors", &r).unwrap();
        assert!(out.starts_with("Achei 7 erros de build no Visual Studio (e 1 aviso):\n- CS0103 em Program.cs:12 — The name 'foo' does not exist\n- TS2304 em app.ts — Cannot find name 'x'.\n- erro — linker failed\n- … e mais 2."), "{out}");
        assert!(out.ends_with("(\"explica esse erro\")."), "oferece corrigir/explicar: {out}");
        let none = render("dev.errors", &json!({ "source": "terminal", "error_count": 0, "warning_count": 0 })).unwrap();
        assert!(none.starts_with("Não vi erros de build no terminal."), "{none}");
    }

    #[test]
    fn fix_replies_never_claim_a_fix() {
        let job = json!({ "agent": "claude", "job_id": 3, "folder": "C:\\src\\Demo", "error_count": 2 });
        let out = render("dev.fix", &job).unwrap();
        assert!(out.starts_with("Pedi para o Claude Code corrigir 2 erros em C:\\src\\Demo (tarefa #3)."), "{out}");
        assert!(!out.contains("Corrigi"));
        let mut errs = json!({ "source": "terminal", "error_count": 1, "errors": [{ "code": "E0425", "message": "x" }] });
        errs["fix"] = job;
        assert!(render("dev.errors", &errs).unwrap().contains("Pedi para o Claude Code"));
        errs["fix"] = json!({ "error": "o usuário negou esta ação" });
        assert!(render("dev.errors", &errs).unwrap().ends_with("Não pedi a correção: o usuário negou esta ação."));
        assert_eq!(render("dev.fix", &json!({ "cancelled": true })).unwrap(), "Tudo bem, não pedi nada.");
        assert!(render("dev.fix", &json!({ "no_errors": true, "source": "VS Code" })).unwrap().starts_with("Não vi erros de build para corrigir no VS Code."));
    }

    #[test]
    fn git_replies_are_rendered() {
        let r = json!({
            "repo": "demo", "branch": "main", "upstream": "origin/main", "ahead": 2, "behind": 1,
            "staged": 1, "modified": 3, "untracked": 1, "conflicts": 0, "insertions": 10, "deletions": 2, "clean": false,
            "last_commits": ["feat: a", "fix: b", "chore: c", "docs: d"],
        });
        assert_eq!(
            render("dev.git_status", &r).unwrap(),
            "Git do demo: branch main, 2 commits à frente e 1 commit atrás do origin/main. Mudanças: 3 arquivos modificados, 1 no stage, 1 arquivo novo (não rastreado) (+10 −2 linhas). Últimos commits: “feat: a”; “fix: b”; “chore: c”."
        );
        let clean = json!({ "repo": "demo", "branch": "dev", "upstream": "origin/dev", "ahead": 0, "behind": 0, "clean": true, "last_commits": [] });
        assert_eq!(render("dev.git_status", &clean).unwrap(), "Git do demo: branch dev, em dia com origin/dev. Nada modificado.");
        let detached = json!({ "repo": "demo", "branch": null, "upstream": null, "clean": true });
        assert_eq!(render("dev.git_status", &detached).unwrap(), "Git do demo: HEAD destacado (fora de um branch), sem branch remoto configurado. Nada modificado.");
    }
}
