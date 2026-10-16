//! Resumo do git de um projeto, SÓ LEITURA. A Lilith nunca muda o
//! repositório: os únicos comandos possíveis são os de [`READ_ONLY`], com
//! argumentos fixos no código (nada vem do modelo), e o `status` roda com
//! `--no-optional-locks` para nem atualizar o índice.

use crate::WinError;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Subcomandos permitidos. Qualquer outro é recusado antes de rodar.
pub const READ_ONLY: &[&str] = &["rev-parse", "status", "log", "diff"];

/// Quantos commits recentes entram no resumo.
const LAST_COMMITS: usize = 5;
const GIT_TIMEOUT: Duration = Duration::from_secs(15);
/// Assunto de commit é texto de terceiros: curto.
const MAX_SUBJECT: usize = 120;

/// Estado do repositório, só contagens (nada de conteúdo de arquivo).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Status {
    /// Nome do branch; `None` com HEAD destacado.
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Arquivos com mudança já no stage (`git add`).
    pub staged: u32,
    /// Arquivos modificados fora do stage.
    pub modified: u32,
    /// Arquivos novos, não rastreados.
    pub untracked: u32,
    /// Conflitos de merge.
    pub conflicts: u32,
}

/// Lê a saída de `git status --porcelain=v2 --branch`.
pub fn parse_status(out: &str) -> Status {
    let mut s = Status::default();
    for line in out.lines() {
        if let Some(head) = line.strip_prefix("# branch.head ") {
            s.branch = (head.trim() != "(detached)").then(|| head.trim().to_string());
        } else if let Some(up) = line.strip_prefix("# branch.upstream ") {
            s.upstream = Some(up.trim().to_string());
        } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
            for part in ab.split_whitespace() {
                if let Some(n) = part.strip_prefix('+') {
                    s.ahead = n.parse().unwrap_or(0);
                } else if let Some(n) = part.strip_prefix('-') {
                    s.behind = n.parse().unwrap_or(0);
                }
            }
        } else if line.starts_with("? ") {
            s.untracked += 1;
        } else if line.starts_with("u ") {
            s.conflicts += 1;
        } else if line.starts_with("1 ") || line.starts_with("2 ") {
            // "1 XY …": X = stage, Y = árvore de trabalho; "." = sem mudança.
            let xy: Vec<char> = line[2..].chars().take(2).collect();
            if xy.first().is_some_and(|c| *c != '.') {
                s.staged += 1;
            }
            if xy.get(1).is_some_and(|c| *c != '.') {
                s.modified += 1;
            }
        }
    }
    s
}

/// Assuntos dos commits (`git log --format=%s`), um por linha.
pub fn parse_log(out: &str) -> Vec<String> {
    out.lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .take(LAST_COMMITS)
        .map(|l| if l.chars().count() > MAX_SUBJECT { l.chars().take(MAX_SUBJECT).collect::<String>() + "…" } else { l.to_string() })
        .collect()
}

/// Linhas adicionadas/removidas de `git diff --shortstat`
/// (" 3 files changed, 10 insertions(+), 2 deletions(-)").
pub fn parse_shortstat(out: &str) -> (u32, u32) {
    let mut ins = 0;
    let mut del = 0;
    for part in out.split(',') {
        let p = part.trim();
        let n = p.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0);
        if p.contains("insertion") {
            ins = n;
        } else if p.contains("deletion") {
            del = n;
        }
    }
    (ins, del)
}

/// Roda `git <args>` na pasta, só se o subcomando for de leitura.
async fn git(folder: &Path, args: &[&str]) -> Result<String, WinError> {
    let sub = args.first().copied().unwrap_or("");
    if !READ_ONLY.contains(&sub) {
        return Err(WinError::InvalidParams(format!("git {sub} não é permitido (só leitura)")));
    }
    let mut cmd = crate::handlers::hidden_command("git");
    cmd.arg("--no-optional-locks")
        .arg("-C")
        .arg(folder)
        .args(args)
        // Nunca pede senha nem abre editor/pager.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_PAGER", "cat")
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(GIT_TIMEOUT, cmd.output())
        .await
        .map_err(|_| WinError::Failed("o git demorou demais".into()))?
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => WinError::Unavailable("git não está instalado (ou não está no PATH)".into()),
            _ => WinError::Failed(format!("não consegui rodar o git: {e}")),
        })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err.lines().next().unwrap_or("").trim().to_string();
        return Err(WinError::Failed(format!("git {sub}: {first}")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Raiz do repositório que contém `folder` (None se não é um repositório).
pub async fn repo_root(folder: &Path) -> Option<PathBuf> {
    let out = git(folder, &["rev-parse", "--show-toplevel"]).await.ok()?;
    let root = out.trim();
    (!root.is_empty()).then(|| PathBuf::from(root.replace('/', "\\")))
}

/// Resumo do repositório: branch, à frente/atrás, contagens e últimos commits.
pub async fn summary(folder: &Path) -> Result<serde_json::Value, WinError> {
    if !folder.is_dir() {
        return Err(WinError::NotFound(format!("pasta {}", folder.display())));
    }
    let root = repo_root(folder)
        .await
        .ok_or_else(|| WinError::NotFound(format!("repositório git em {} (a pasta não é um repositório)", folder.display())))?;
    let status = parse_status(&git(&root, &["status", "--porcelain=v2", "--branch"]).await?);
    // Repositório sem nenhum commit: log e diff falham — não é erro.
    let log_n = format!("-n{LAST_COMMITS}");
    let commits = git(&root, &["log", &log_n, "--format=%s"]).await.map(|o| parse_log(&o)).unwrap_or_default();
    let (insertions, deletions) = git(&root, &["diff", "HEAD", "--shortstat"]).await.map(|o| parse_shortstat(&o)).unwrap_or((0, 0));
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(serde_json::json!({
        "repo": name,
        "folder": root,
        "branch": status.branch,
        "detached": status.branch.is_none(),
        "upstream": status.upstream,
        "ahead": status.ahead,
        "behind": status.behind,
        "staged": status.staged,
        "modified": status.modified,
        "untracked": status.untracked,
        "conflicts": status.conflicts,
        "insertions": insertions,
        "deletions": deletions,
        "clean": status.staged + status.modified + status.untracked + status.conflicts == 0,
        "last_commits": commits,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn porcelain_v2_is_counted() {
        let out = "\
# branch.oid 1234567890abcdef
# branch.head feature/x
# branch.upstream origin/feature/x
# branch.ab +2 -1
1 .M N... 100644 100644 100644 aaa bbb src/a.rs
1 M. N... 100644 100644 100644 aaa bbb src/b.rs
1 MM N... 100644 100644 100644 aaa bbb src/c.rs
2 R. N... 100644 100644 100644 aaa bbb R100 src/new.rs\tsrc/old.rs
u UU N... 100644 100644 100644 100644 aaa bbb ccc src/conflict.rs
? notes.txt
? tmp/
! ignored.log
";
        let s = parse_status(out);
        assert_eq!(s.branch.as_deref(), Some("feature/x"));
        assert_eq!(s.upstream.as_deref(), Some("origin/feature/x"));
        assert_eq!((s.ahead, s.behind), (2, 1));
        assert_eq!((s.staged, s.modified, s.untracked, s.conflicts), (3, 2, 2, 1));
    }

    #[test]
    fn detached_head_and_no_upstream() {
        let s = parse_status("# branch.oid abc\n# branch.head (detached)\n");
        assert_eq!(s.branch, None);
        assert_eq!(s.upstream, None);
        assert_eq!(s, Status { branch: None, ..Default::default() });
    }

    #[test]
    fn log_and_shortstat() {
        let log = "feat: um\n\nfix: dois\nchore: três\ndocs: 4\ntest: 5\nrefactor: 6\n";
        assert_eq!(parse_log(log), vec!["feat: um", "fix: dois", "chore: três", "docs: 4", "test: 5"]);
        assert!(parse_log(&"x".repeat(500))[0].chars().count() <= MAX_SUBJECT + 1);
        assert_eq!(parse_shortstat(" 3 files changed, 10 insertions(+), 2 deletions(-)\n"), (10, 2));
        assert_eq!(parse_shortstat(" 1 file changed, 1 deletion(-)"), (0, 1));
        assert_eq!(parse_shortstat(""), (0, 0));
    }

    #[tokio::test]
    async fn only_read_only_subcommands_run() {
        for args in [&["commit", "-m", "x"][..], &["push"], &["checkout", "."], &["reset", "--hard"], &["stash"], &[]] {
            let r = git(Path::new("."), args).await;
            assert!(matches!(r, Err(WinError::InvalidParams(_))), "{args:?}");
        }
    }

    #[tokio::test]
    async fn summary_of_a_fresh_repository() {
        // Repositório descartável numa pasta temporária (nada do usuário).
        let dir = tempfile::tempdir().unwrap();
        let init = std::process::Command::new("git").arg("init").arg("-q").arg(dir.path()).status();
        if !init.is_ok_and(|s| s.success()) {
            return; // sem git na máquina de teste
        }
        std::fs::write(dir.path().join("a.txt"), "oi").unwrap();
        let v = summary(dir.path()).await.unwrap();
        assert_eq!(v["untracked"], 1);
        assert_eq!(v["clean"], false);
        assert_eq!(v["last_commits"], serde_json::json!([]));
        assert!(summary(&dir.path().join("nao-existe")).await.is_err());
    }
}
