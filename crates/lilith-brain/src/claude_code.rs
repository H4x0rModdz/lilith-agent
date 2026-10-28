//! Cérebro pelo Claude Code instalado no PC (`claude -p`, na assinatura de
//! quem usa — sem chave de API). Escolha do usuário nas Configurações; o
//! padrão continua o Ollama local.
//!
//! Cada chamada é um `claude -p` isolado: sem as ferramentas do próprio
//! Claude Code (`--tools ""`), sem MCP, sem configurações nem CLAUDE.md
//! (`--setting-sources ""`, pasta de trabalho neutra), sem sessão salva. O
//! modelo só escolhe — quem executa continua sendo o despachante da Lilith,
//! com o guard e as aprovações de sempre.
//!
//! Segurança: o executável é chamado direto (não o shim `.cmd`), então os
//! argumentos não passam pelo `cmd.exe`; todo texto livre (pedido, histórico,
//! resultados) vai pelo stdin, e o prompt de sistema por arquivo.

use crate::error::LilithError;
use crate::ollama::{Ollama, OllamaReply};
use crate::tools::{Tool, ToolCall};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::UnboundedSender;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaudeCodeConfig {
    /// Quem escolhe as ferramentas e responde.
    pub agent_model: String,
    /// Quem interpreta o pedido (e extrai fatos para a memória).
    pub interpret_model: String,
    /// `low` deixa cada passo rápido; pedidos de companion são curtos.
    pub effort: String,
    pub timeout_secs: u64,
    /// Caminho do `claude.exe`, se não estiver no lugar de sempre.
    pub exe: Option<String>,
}

impl Default for ClaudeCodeConfig {
    fn default() -> Self {
        Self {
            agent_model: "claude-haiku-5-5".into(),
            interpret_model: "claude-haiku-5-5".into(),
            effort: "low".into(),
            timeout_secs: 120,
            exe: None,
        }
    }
}

/// Onde está o `claude.exe`: o configurado; um `claude.exe` no PATH (instalador
/// nativo); ou o binário que o shim do npm (`claude.cmd`) chama.
pub fn locate(cfg: &ClaudeCodeConfig) -> Option<PathBuf> {
    if let Some(p) = cfg.exe.as_deref().map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join(".local").join("bin"));
    }
    if let Some(appdata) = dirs::data_dir() {
        dirs.push(appdata.join("npm"));
    }
    dirs.iter().find_map(|d| exe_in(d))
}

fn exe_in(dir: &Path) -> Option<PathBuf> {
    let native = dir.join("claude.exe");
    if native.is_file() {
        return Some(native);
    }
    let npm = dir.join("node_modules").join("@anthropic-ai").join("claude-code").join("bin").join("claude.exe");
    (dir.join("claude.cmd").is_file() && npm.is_file()).then_some(npm)
}

pub struct ClaudeCode {
    cfg: ClaudeCodeConfig,
    exe: PathBuf,
    /// Pasta neutra (sem CLAUDE.md de projeto) onde ficam os prompts de sistema.
    workdir: PathBuf,
}

impl ClaudeCode {
    pub fn new(cfg: ClaudeCodeConfig, workdir: PathBuf) -> Option<Self> {
        let exe = locate(&cfg)?;
        let _ = std::fs::create_dir_all(&workdir);
        Some(Self { cfg, exe, workdir })
    }

    /// Uma chamada `claude -p` com saída JSON validada por `schema`.
    async fn ask(&self, model: &str, system: &str, prompt: &str, schema: &Value) -> Result<Value, LilithError> {
        let system_file = self.system_file(system)?;
        let mut cmd = tokio::process::Command::new(&self.exe);
        cmd.args(["-p", "--model", model, "--effort", &self.cfg.effort])
            .args(["--tools", "", "--strict-mcp-config", "--setting-sources", ""])
            .args(["--disable-slash-commands", "--no-session-persistence", "--output-format", "json"])
            .arg("--system-prompt-file")
            .arg(&system_file)
            .arg("--json-schema")
            .arg(schema.to_string())
            .current_dir(&self.workdir)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = cmd.spawn().map_err(|e| LilithError::OllamaUnreachable(format!("Claude Code não abriu: {e}")))?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await.map_err(|e| LilithError::OllamaInvalid(e.to_string()))?;
        }
        let out = tokio::time::timeout(Duration::from_secs(self.cfg.timeout_secs.max(5)), child.wait_with_output())
            .await
            .map_err(|_| LilithError::OllamaUnreachable("o Claude Code demorou demais para responder".into()))?
            .map_err(|e| LilithError::OllamaUnreachable(e.to_string()))?;
        parse_output(&String::from_utf8_lossy(&out.stdout), &String::from_utf8_lossy(&out.stderr))
    }

    /// Prompt de sistema em arquivo (a linha de comando do Windows tem limite
    /// e o catálogo de ferramentas é grande). Nome pelo conteúdo: o mesmo texto
    /// reaproveita o arquivo.
    fn system_file(&self, system: &str) -> Result<PathBuf, LilithError> {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        system.hash(&mut h);
        let path = self.workdir.join(format!("system-{:016x}.txt", h.finish()));
        if !path.is_file() {
            prune(&self.workdir);
            std::fs::write(&path, system).map_err(|e| LilithError::OllamaInvalid(e.to_string()))?;
        }
        Ok(path)
    }
}

/// Prompts de sistema antigos: guarda os 50 mais recentes.
fn prune(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("system-"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= 50 {
        return;
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in files.into_iter().skip(50) {
        let _ = std::fs::remove_file(p);
    }
}

/// Resultado do `claude -p --output-format json`: o JSON estruturado, ou o erro
/// em português (não logado, sem limite…).
pub(crate) fn parse_output(stdout: &str, stderr: &str) -> Result<Value, LilithError> {
    let v: Value = serde_json::from_str(stdout.trim()).map_err(|_| {
        let detail = if stderr.trim().is_empty() { stdout } else { stderr };
        LilithError::OllamaUnreachable(format!("o Claude Code falhou: {}", detail.trim().chars().take(200).collect::<String>()))
    })?;
    if v["is_error"] == true {
        let msg = v["result"].as_str().unwrap_or("erro desconhecido");
        return Err(LilithError::OllamaUnreachable(format!("o Claude Code recusou: {}", msg.chars().take(200).collect::<String>())));
    }
    match &v["structured_output"] {
        Value::Null => v["result"]
            .as_str()
            .and_then(|r| serde_json::from_str(r).ok())
            .ok_or_else(|| LilithError::OllamaInvalid("o Claude Code não devolveu JSON".into())),
        s => Ok(s.clone()),
    }
}

/// Mensagens no formato do Ollama → (prompt de sistema, conversa em texto).
/// A primeira mensagem de sistema é o prompt; as outras viram notas na conversa.
pub(crate) fn flatten(messages: &[Value]) -> (String, String) {
    let mut system = String::new();
    let mut lines = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        let content = m["content"].as_str().unwrap_or("").trim();
        match m["role"].as_str().unwrap_or("") {
            "system" if i == 0 => system = content.to_string(),
            "system" => lines.push(format!("[nota do sistema] {content}")),
            "user" => lines.push(format!("[usuário] {content}")),
            "assistant" => {
                for c in m["tool_calls"].as_array().into_iter().flatten() {
                    lines.push(format!("[lilith chamou] {} {}", c["function"]["name"].as_str().unwrap_or("?"), c["function"]["arguments"]));
                }
                if !content.is_empty() {
                    lines.push(format!("[lilith] {content}"));
                }
            }
            "tool" => lines.push(format!("[resultado] {content}")),
            _ => {}
        }
    }
    (system, lines.join("\n\n"))
}

/// Catálogo + formato da resposta, no fim do prompt de sistema do agente.
pub(crate) fn agent_system(base: &str, tools: &[Tool]) -> String {
    let mut s = String::from(base);
    if tools.is_empty() {
        s.push_str("\n\n# Formato\nResponda só em texto para o usuário, no campo `reply` (`tool` vazio, `params` {}).");
        return s;
    }
    s.push_str(
        "\n\n# Como você age\n\
         Você não executa nada diretamente e não tem ferramentas próprias nesta conversa: a cada passo você \
         ESCOLHE uma ação da lista abaixo respondendo o JSON pedido, e a Lilith (o app) a executa e devolve o \
         [resultado] no próximo passo. Nunca diga que uma ação \"não está disponível\" — se está na lista, \
         escolha-a.\n\n# Ações (tudo o que dá para fazer no PC)\n",
    );
    for t in tools {
        s.push_str(&format!("- {}: {}\n  parâmetros: {}\n", t.action, t.description.split_whitespace().collect::<Vec<_>>().join(" "), t.schema));
    }
    s.push_str(
        "\n# Formato\n\
         Para fazer uma ação: `tool` = o nome, `params` = os parâmetros (`reply` vazio). Para responder ao \
         usuário: `tool` vazio e o texto em `reply`. Uma ação por passo; depois de cada [resultado], decida o \
         próximo.\n\
         Se nenhuma ferramenta resolve uma etapa do pedido, NÃO improvise com outra (um programa do Windows não \
         é um site): faça as etapas que dá e diga claramente, em `reply`, o que você ainda não sabe fazer.\n\
         Se uma etapa falhar, não faça as que dependem dela; conte o que fez e o que não fez.",
    );
    s
}

pub(crate) fn agent_schema(tools: &[Tool]) -> Value {
    let mut names: Vec<&str> = tools.iter().map(|t| t.action).collect();
    names.push("");
    json!({
        "type": "object",
        "properties": {
            "tool": { "type": "string", "enum": names },
            "params": { "type": "object" },
            "reply": { "type": "string" }
        },
        "required": ["tool", "params", "reply"],
        "additionalProperties": false
    })
}

/// Resposta do agente → texto ou uma chamada de ferramenta.
pub(crate) fn to_reply(v: &Value, tools: &[Tool]) -> OllamaReply {
    let tool = v["tool"].as_str().unwrap_or("");
    let reply = v["reply"].as_str().unwrap_or("").trim().to_string();
    if !tool.is_empty() && tools.iter().any(|t| t.action == tool) {
        let params = if v["params"].is_object() { v["params"].clone() } else { json!({}) };
        return OllamaReply { text: String::new(), tool_calls: vec![ToolCall { action: tool.into(), params }] };
    }
    OllamaReply { text: reply, tool_calls: Vec::new() }
}

#[async_trait]
impl Ollama for ClaudeCode {
    async fn chat_messages(
        &self,
        messages: &[Value],
        tools: &[Tool],
        chunks: Option<UnboundedSender<String>>,
    ) -> Result<OllamaReply, LilithError> {
        let (base, conversation) = flatten(messages);
        let prompt = format!("{conversation}\n\nQual é o próximo passo?");
        let v = self.ask(&self.cfg.agent_model, &agent_system(&base, tools), &prompt, &agent_schema(tools)).await?;
        let reply = to_reply(&v, tools);
        // Sem streaming token a token: o texto chega inteiro de uma vez.
        if let (Some(tx), false) = (chunks, reply.text.is_empty()) {
            let _ = tx.send(reply.text.clone());
        }
        Ok(reply)
    }

    async fn structured(&self, messages: &[Value], schema: &Value) -> Result<Value, LilithError> {
        let (system, conversation) = flatten(messages);
        self.ask(&self.cfg.interpret_model, &system, &conversation, schema).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(action: &'static str) -> Tool {
        Tool { action, description: "faz algo.", schema: json!({ "type": "object", "properties": {} }) }
    }

    #[test]
    fn flattens_the_conversation() {
        let m = vec![
            json!({ "role": "system", "content": "Você é a Lilith." }),
            json!({ "role": "user", "content": "abre o spotify" }),
            json!({ "role": "assistant", "content": "", "tool_calls": [{ "function": { "name": "app.open", "arguments": { "app": "spotify" } } }] }),
            json!({ "role": "tool", "content": "{\"status\":\"success\"}" }),
            json!({ "role": "system", "content": "plano: app.open" }),
        ];
        let (system, text) = flatten(&m);
        assert_eq!(system, "Você é a Lilith.");
        assert_eq!(
            text,
            "[usuário] abre o spotify\n\n[lilith chamou] app.open {\"app\":\"spotify\"}\n\n[resultado] {\"status\":\"success\"}\n\n[nota do sistema] plano: app.open"
        );
    }

    #[test]
    fn reply_becomes_tool_call_or_text() {
        let tools = vec![tool("app.open")];
        let call = to_reply(&json!({ "tool": "app.open", "params": { "app": "spotify" }, "reply": "" }), &tools);
        assert_eq!(call.tool_calls[0].action, "app.open");
        assert_eq!(call.tool_calls[0].params, json!({ "app": "spotify" }));
        let text = to_reply(&json!({ "tool": "", "params": {}, "reply": " Pronto! " }), &tools);
        assert!(text.tool_calls.is_empty());
        assert_eq!(text.text, "Pronto!");
        // Ferramenta fora do catálogo deste passo não vira chamada.
        assert!(to_reply(&json!({ "tool": "file.delete", "params": {}, "reply": "" }), &tools).tool_calls.is_empty());
    }

    #[test]
    fn schema_lists_tools_plus_empty() {
        let s = agent_schema(&[tool("app.open")]);
        assert_eq!(s["properties"]["tool"]["enum"], json!(["app.open", ""]));
    }

    #[test]
    fn cli_output_is_parsed() {
        let ok = r#"{"type":"result","is_error":false,"result":"{}","structured_output":{"tool":"","params":{},"reply":"oi"}}"#;
        assert_eq!(parse_output(ok, "").unwrap()["reply"], "oi");
        let err = r#"{"type":"result","is_error":true,"result":"Not logged in · Please run /login"}"#;
        assert!(parse_output(err, "").unwrap_err().to_string().contains("Not logged in"));
        assert!(parse_output("", "claude: command failed").unwrap_err().to_string().contains("command failed"));
    }

    /// `cargo test -p lilith-brain claude_code::tests::live -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live() {
        let dir = std::env::temp_dir().join("lilith-claude-code-test");
        let brain = ClaudeCode::new(ClaudeCodeConfig::default(), dir).expect("claude.exe");
        let tools = vec![Tool {
            action: "app.open",
            description: "Abre um programa.",
            schema: json!({ "type": "object", "properties": { "app": { "type": "string" } }, "required": ["app"] }),
        }];
        let m = vec![json!({ "role": "system", "content": "Você é a Lilith." }), json!({ "role": "user", "content": "abre o spotify" })];
        let r = brain.chat_messages(&m, &tools, None).await.unwrap();
        println!("{:?}", r);
        assert_eq!(r.tool_calls[0].action, "app.open");
    }
}

#[cfg(test)]
mod live_interpret {
    /// `cargo test -p lilith-brain live_interpret -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn interprets_the_real_request() {
        let brain = super::ClaudeCode::new(super::ClaudeCodeConfig::default(), std::env::temp_dir().join("lilith-claude-code-test")).unwrap();
        let tools = crate::tools::all_tools();
        for text in [
            "preciso que você instale o chrome e também abra ele e pesquise \"batata\" em seu search. Logo após, abra o gerenciador de dispositivos e também dê stop no serviço do docker",
            "edita a planilha de gastos no excel e coloca a soma da coluna B",
        ] {
            let t = std::time::Instant::now();
            let i = crate::interpret::run(&brain, text, &[], &tools).await;
            println!("{:?} ms={}\n", i, t.elapsed().as_millis());
        }
    }
}
