//! Delegação para agentes de código em segundo plano.
//!
//! A Lilith escreve o prompt e um CLI de agente (Claude Code, Codex, ou
//! qualquer outro configurado) executa a tarefa. Cada execução vira um
//! [`Job`] acompanhado em tempo real pelo painel da Lilith.
//!
//! Segurança: o prompt vai pelo **stdin**, nunca pela linha de comando — os
//! CLIs do npm são shims `.cmd` executados via `cmd.exe`, e texto gerado pelo
//! LLM com `&`/`|` na linha de comando viraria injeção. Os argumentos são
//! fixos e vêm só da configuração do usuário.

mod parse;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Como lançar um agente. `command` é procurado no PATH pelo `cmd.exe`
/// (funciona com `.exe` e com shims `.cmd` do npm).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    pub command: String,
    pub args: Vec<String>,
    /// Formato da saída, para extrair texto legível: `claude`, `codex` ou `plain`.
    #[serde(default = "plain")]
    pub format: String,
}

fn plain() -> String {
    "plain".into()
}

/// Agentes padrão. Claude roda com `acceptEdits` (edita arquivos, mas
/// comandos de shell continuam bloqueados no modo `-p`); Codex roda no
/// sandbox `workspace-write`. O usuário pode trocar tudo no config.
pub fn default_agents() -> BTreeMap<String, AgentSpec> {
    let spec = |command: &str, args: &[&str], format: &str| AgentSpec {
        command: command.into(),
        args: args.iter().map(|s| s.to_string()).collect(),
        format: format.into(),
    };
    BTreeMap::from([
        (
            "claude".into(),
            spec(
                "claude",
                &["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits"],
                "claude",
            ),
        ),
        (
            "codex".into(),
            spec(
                "codex",
                &["exec", "--json", "--skip-git-repo-check", "-s", "workspace-write", "-"],
                "codex",
            ),
        ),
    ])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize)]
pub struct Job {
    pub id: u64,
    pub agent: String,
    pub task: String,
    pub folder: PathBuf,
    /// Conversa (aba do chat) que pediu a tarefa — o painel agrupa por ela.
    pub conversation: Option<String>,
    pub status: JobStatus,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    /// Linhas legíveis (texto do agente, ferramentas usadas). Limitado.
    pub log: Vec<String>,
    pub result: Option<String>,
    #[serde(skip)]
    pid: Option<u32>,
}

/// O que o painel recebe: a linha nova de um job, ou o job inteiro quando
/// muda de estado.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JobEvent {
    Updated { job: Job },
    Line { id: u64, line: String },
}

const MAX_LOG_LINES: usize = 400;

pub struct Jobs {
    agents: BTreeMap<String, AgentSpec>,
    next_id: AtomicU64,
    jobs: Mutex<HashMap<u64, Job>>,
    on_event: Box<dyn Fn(JobEvent) + Send + Sync>,
}

impl Jobs {
    pub fn new(agents: BTreeMap<String, AgentSpec>, on_event: impl Fn(JobEvent) + Send + Sync + 'static) -> Arc<Self> {
        Arc::new(Self {
            agents,
            next_id: AtomicU64::new(1),
            jobs: Mutex::new(HashMap::new()),
            on_event: Box::new(on_event),
        })
    }

    pub fn agent_names(&self) -> Vec<String> {
        self.agents.keys().cloned().collect()
    }

    pub fn list(&self) -> Vec<Job> {
        let mut v: Vec<Job> = self.jobs.lock().unwrap().values().cloned().collect();
        v.sort_by_key(|j| std::cmp::Reverse(j.id));
        v
    }

    pub fn get(&self, id: u64) -> Option<Job> {
        self.jobs.lock().unwrap().get(&id).cloned()
    }

    /// Inicia o agente em segundo plano e retorna logo com o job criado.
    pub fn start(
        self: &Arc<Self>,
        agent: &str,
        task: &str,
        folder: PathBuf,
        conversation: Option<String>,
    ) -> Result<Job, String> {
        let spec = self
            .agents
            .get(agent)
            .cloned()
            .ok_or_else(|| format!("agente '{agent}' não configurado (disponíveis: {})", self.agent_names().join(", ")))?;
        if task.trim().is_empty() {
            return Err("tarefa vazia".into());
        }
        if !folder.is_dir() {
            return Err(format!("pasta '{}' não existe", folder.display()));
        }

        let mut child = command_for(&spec)
            .current_dir(&folder)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false)
            .spawn()
            .map_err(|e| format!("não consegui iniciar '{}': {e}", spec.command))?;

        let job = Job {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            agent: agent.into(),
            task: task.into(),
            folder,
            conversation,
            status: JobStatus::Running,
            started_ms: now_ms(),
            finished_ms: None,
            log: Vec::new(),
            result: None,
            pid: child.id(),
        };
        self.jobs.lock().unwrap().insert(job.id, job.clone());
        (self.on_event)(JobEvent::Updated { job: job.clone() });

        let id = job.id;
        let mut stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");
        let task_text = task.to_string();
        let format = spec.format.clone();
        let this = self.clone();

        tokio::spawn(async move {
            // Prompt pelo stdin e fecha — o agente começa ao ver o EOF.
            let _ = stdin.write_all(task_text.as_bytes()).await;
            drop(stdin);

            let err_task = tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                let mut tail = Vec::new();
                while let Ok(Some(l)) = lines.next_line().await {
                    if !l.trim().is_empty() {
                        tail.push(l);
                        if tail.len() > 20 {
                            tail.remove(0);
                        }
                    }
                }
                tail
            });

            let mut lines = BufReader::new(stdout).lines();
            let mut result: Option<String> = None;
            while let Ok(Some(raw)) = lines.next_line().await {
                let parsed = parse::line(&format, &raw);
                if let Some(r) = parsed.result {
                    result = Some(r);
                }
                if let Some(text) = parsed.display {
                    this.push_line(id, text);
                }
            }

            let status = child.wait().await;
            let stderr_tail = err_task.await.unwrap_or_default();
            this.finish(id, status.map(|s| s.success()).unwrap_or(false), result, stderr_tail);
        });

        Ok(job)
    }

    /// Mata a árvore do processo (o `cmd.exe` e o node/exe filho).
    pub async fn cancel(&self, id: u64) -> Result<(), String> {
        let pid = {
            let jobs = self.jobs.lock().unwrap();
            let job = jobs.get(&id).ok_or_else(|| format!("job {id} não existe"))?;
            if job.status != JobStatus::Running {
                return Err(format!("job {id} já terminou"));
            }
            job.pid
        };
        if let Some(pid) = pid {
            let _ = hidden("taskkill")
                .args(["/PID", &pid.to_string(), "/T", "/F"])
                .status()
                .await;
        }
        self.set_status(id, JobStatus::Cancelled, None);
        Ok(())
    }

    fn push_line(&self, id: u64, line: String) {
        if let Some(job) = self.jobs.lock().unwrap().get_mut(&id) {
            job.log.push(line.clone());
            if job.log.len() > MAX_LOG_LINES {
                job.log.remove(0);
            }
        }
        (self.on_event)(JobEvent::Line { id, line });
    }

    fn finish(&self, id: u64, success: bool, result: Option<String>, stderr_tail: Vec<String>) {
        let already_cancelled = self
            .jobs
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|j| j.status == JobStatus::Cancelled);
        if already_cancelled {
            return;
        }
        let (status, result) = match (success, result) {
            (true, r) => (JobStatus::Done, r.or_else(|| Some("(terminou sem resumo)".into()))),
            (false, r) => (
                JobStatus::Failed,
                r.or_else(|| Some(stderr_tail.join("\n")).filter(|s| !s.is_empty()))
                    .or_else(|| Some("o agente saiu com erro".into())),
            ),
        };
        self.set_status(id, status, result);
    }

    fn set_status(&self, id: u64, status: JobStatus, result: Option<String>) {
        let snapshot = {
            let mut jobs = self.jobs.lock().unwrap();
            let Some(job) = jobs.get_mut(&id) else { return };
            job.status = status;
            job.finished_ms = Some(now_ms());
            if result.is_some() {
                job.result = result;
            }
            job.clone()
        };
        (self.on_event)(JobEvent::Updated { job: snapshot });
    }
}

/// `cmd /D /C <command> <args…>` — resolve `.cmd`/`.exe` pelo PATH. Os args
/// vêm só do config (nunca do LLM), então o `cmd.exe` não vê texto livre.
fn command_for(spec: &AgentSpec) -> tokio::process::Command {
    let mut cmd = hidden("cmd");
    cmd.args(["/D", "/C", &spec.command]).args(&spec.args);
    cmd
}

fn hidden(program: &str) -> tokio::process::Command {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut cmd = tokio::process::Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Agente fake: `findstr` ecoa o stdin — prova que o prompt chega pelo
    /// stdin e que caracteres de shell não são interpretados.
    #[tokio::test]
    async fn runs_agent_with_prompt_on_stdin() {
        let agents = BTreeMap::from([(
            "echo".to_string(),
            AgentSpec { command: "findstr".into(), args: vec!["x*".into()], format: "plain".into() },
        )]);
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = events.clone();
        let jobs = Jobs::new(agents, move |e| ev.lock().unwrap().push(e));
        let dir = tempfile::tempdir().unwrap();
        let job = jobs.start("echo", "olá & echo INJETADO | calc", dir.path().to_path_buf(), None).unwrap();

        for _ in 0..100 {
            if jobs.get(job.id).unwrap().status != JobStatus::Running {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let done = jobs.get(job.id).unwrap();
        assert_eq!(done.status, JobStatus::Done);
        assert_eq!(done.log, vec!["olá & echo INJETADO | calc"]);
    }

    #[test]
    fn unknown_agent_and_missing_folder_are_rejected() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let jobs = Jobs::new(default_agents(), |_| {});
            assert!(jobs.start("gpt-99", "x", std::env::temp_dir(), None).unwrap_err().contains("não configurado"));
            assert!(jobs.start("claude", "x", PathBuf::from("Z:\\nao\\existe"), None).unwrap_err().contains("não existe"));
        });
    }
}
