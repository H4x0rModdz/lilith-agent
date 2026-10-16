//! Tarefas delegadas a agentes de código: o app embrulha o pedido do modelo com
//! regras de segurança fixas (nada de git, só editar arquivos) e,
//! para vulnerabilidades de pacotes .NET, com o procedimento de correção — em
//! código, para não depender de um modelo pequeno redigir tudo certo.

/// Regras valem para QUALQUER tarefa delegada: o agente edita arquivos do
/// usuário (`acceptEdits`); git fica por conta do usuário.
pub(crate) const SAFETY_RULES: &str = "\
Rules from Lilith (the user's assistant) — follow them exactly:
- Do NOT touch git at all: no branch, commit, checkout, stash, reset, clean, push, pull or merge. Only edit files in the working folder. (The user reviews the changes with their own git tools.)
- Do not delete files or run other destructive commands (rm -rf and similar).
- Stay inside the working folder.
- When done, summarize in plain language: which files changed (before/after) and anything left undone and why.";

/// Procedimento para vulnerabilidades de pacotes NuGet (aviso do Visual Studio).
const DOTNET_VULNERABILITY_PROCEDURE: &str = "\
This is a .NET solution and the NuGet audit reports vulnerable packages. Goal: remove those vulnerabilities safely.
1. Find the .sln (or the project files) in the working folder. Run `dotnet list <solution> package --vulnerable --include-transitive` and record every finding: package, resolved version, severity, advisory link, and which projects use it.
2. For each finding:
   - TRANSITIVE package: find which DIRECT package pulls it (`dotnet nuget why <solution> <package>` if available; otherwise inspect obj/project.assets.json). Preferred fix: upgrade that direct package to a version that depends on a patched version. If none exists, pin a patched version explicitly with a PackageReference in the affected projects (or in Directory.Packages.props when central package management is used).
   - DIRECT package: upgrade to the lowest patched version listed in the advisory, preferring a patch/minor bump over a major one.
3. Run `dotnet restore`, `dotnet build` and `dotnet test`. If a change breaks the build or the tests, revert only that change and explain why it could not be applied.
4. Run the vulnerability check again. Report: what was vulnerable (package, version, severity, projects), what you changed (file, version before/after), and what remains vulnerable.
Only change package versions and project/props files. No code refactors.";

/// O pedido fala de vulnerabilidade de pacote .NET/NuGet?
fn is_dotnet_vulnerability(task: &str) -> bool {
    let t = task.to_lowercase();
    t.contains("vulnerab") && ["nuget", ".sln", "dotnet", ".net", "csproj", "pacote", "package"].iter().any(|k| t.contains(k))
}

/// O pedido é consertar/alterar um projeto específico (precisa de pasta certa)?
pub fn is_project_repair(task: &str) -> bool {
    is_dotnet_vulnerability(task)
}

/// Pedido final enviado ao agente: regras + (procedimento) + o contexto do modelo.
pub fn compose_task(task: &str) -> String {
    let task = task.trim();
    let mut parts = vec![SAFETY_RULES.to_string()];
    if is_dotnet_vulnerability(task) {
        parts.push(DOTNET_VULNERABILITY_PROCEDURE.to_string());
    }
    parts.push(format!("Context from the user's request (what Lilith saw/was asked):\n{task}"));
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_task_gets_the_safety_rules() {
        let t = compose_task("renomeia as variáveis do arquivo x");
        assert!(t.contains("Do NOT touch git at all") && t.ends_with("do arquivo x"));
        assert!(!t.contains("dotnet list"), "procedimento só para vulnerabilidades");
    }

    #[test]
    fn nuget_vulnerabilities_get_the_procedure_with_the_context() {
        let t = compose_task("Corrigir a vulnerabilidade do pacote Microsoft.OpenApi 2.0.0 (transitivo) no NuGet da solução CaptiveService");
        assert!(t.contains("dotnet list <solution> package --vulnerable --include-transitive"));
        assert!(t.contains("Microsoft.OpenApi 2.0.0") && t.contains("Do NOT touch git at all"));
        assert!(is_dotnet_vulnerability("vulnerabilidade no pacote nuget"));
        assert!(!is_dotnet_vulnerability("vulnerabilidade no servidor web"));
    }
}
