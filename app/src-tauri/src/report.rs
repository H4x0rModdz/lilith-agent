//! Relatório de bug: quando um pedido falha por um erro DELA (não por falta de
//! permissão ou algo que não existe no PC), a Lilith oferece, num cartão,
//! avisar quem a desenvolve. Só com o "Enviar" do usuário.
//!
//! Vai para a nossa API (Cloudflare Worker, `services/report-worker`), que
//! guarda a chave do GitHub e abre a issue — o app não tem token nenhum. O
//! relatório é montado pelo código com campos fixos: ficha técnica do PC,
//! versões e o código da falha. Nada de texto livre, caminhos, nomes, conversa
//! ou identificador — todas as issues saem da mesma conta (o app da Lilith).

use crate::AppState;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// Endereço da API publicada (`services/report-worker`).
/// O `config.json` pode trocar (`report_url`), por exemplo para o `wrangler dev`.
const DEFAULT_URL: &str = "https://lilith-reports.lilith-report-worker.workers.dev/v1/report";

/// Falhas que são bug dela. PERMISSION_DENIED (o usuário negou), NOT_FOUND
/// (o app/arquivo não existe) e UNAVAILABLE (Ollama fechado, sem internet)
/// não são.
const BUG_CODES: &[&str] = &["EXECUTION_FAILED", "INVALID_PARAMS"];

/// EXECUTION_FAILED que é o comportamento certo, não bug (tirados do registro
/// de auditoria real): o usuário interrompeu, o programa precisa ser forçado,
/// o Windows negou (janela de administrador), ela já está ocupada com aquilo.
/// A mensagem só serve para decidir aqui — nunca é enviada.
const EXPECTED: &[&str] = &["interrompido pelo usuário", "peça para forçar", "acesso negado", "já estou "];

pub(crate) fn is_bug(code: &str, message: &str) -> bool {
    let message = message.to_lowercase();
    BUG_CODES.contains(&code) && !EXPECTED.iter().any(|e| message.contains(e))
}

/// Envios por dia, do lado do app (a API tem os próprios limites).
const MAX_SENT_PER_DAY: u32 = 3;

const OFFERED: &str = "report_offered";
const SENT_TODAY: &str = "report_sent_today";

#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub action: String,
    pub code: String,
}

type ByConversation = Mutex<Option<HashMap<String, Failure>>>;

/// Última falha de bug em cada conversa, até o turno acabar (oferta automática).
static PENDING: ByConversation = Mutex::new(None);
/// Última falha de qualquer tipo em cada conversa, para quando o próprio
/// usuário pede "cria uma issue disso" — ele decide que foi bug (ex.: um
/// NOT_FOUND num caminho que o modelo inventou).
static LAST: ByConversation = Mutex::new(None);

fn put(map: &ByConversation, conversation: &str, f: Failure) {
    map.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(conversation.to_string(), f);
}

/// Chamado pelo despachante a cada ação que deu erro.
pub fn note_failure(conversation: &str, action: &str, code: &str, message: &str) {
    if action == "self.report" || code == "PERMISSION_DENIED" {
        return;
    }
    let f = Failure { action: action.to_string(), code: code.to_string() };
    put(&LAST, conversation, f.clone());
    if is_bug(code, message) {
        put(&PENDING, conversation, f);
    }
}

fn take_failure(conversation: &str) -> Option<Failure> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner()).as_mut()?.remove(conversation)
}

fn last_failure(conversation: &str) -> Option<Failure> {
    LAST.lock().unwrap_or_else(|e| e.into_inner()).as_ref()?.get(conversation).cloned()
}

/// O que vai para a API — os mesmos campos que o Worker aceita, nem um a mais.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Report {
    pub v: u8,
    pub app: String,
    pub action: String,
    pub code: String,
    pub os: String,
    pub cpu: String,
    pub cores: u32,
    pub ram_gb: u32,
    pub gpu: Option<String>,
    pub browser: Option<String>,
    pub browser_version: Option<String>,
    pub ollama: Option<String>,
    pub model: Option<String>,
    pub voice: String,
    /// "auto" (ela ofereceu) ou "user" (o usuário pediu a issue).
    pub source: String,
    /// Só em pedido de melhoria: o que ela ainda não sabe fazer, genérico e
    /// filtrado por [`safe_gap`] (o usuário vê o texto exato antes de enviar).
    pub missing: Option<String>,
}

impl Report {
    /// Resumo legível do que vai junto (para o cartão de consentimento).
    pub fn summary(&self) -> String {
        let mut parts = vec![
            format!("Lilith {}", self.app),
            self.os.clone(),
            self.cpu.clone(),
            format!("{} GB de RAM", self.ram_gb),
        ];
        parts.extend(self.gpu.clone());
        if let Some(b) = &self.browser {
            parts.push(format!("{b} {}", self.browser_version.as_deref().unwrap_or("")).trim().to_string());
        }
        if let (Some(o), Some(m)) = (&self.ollama, &self.model) {
            parts.push(format!("Ollama {o} ({m})"));
        }
        parts.push(format!("erro {} em {}", self.code, self.action));
        parts.join(" · ")
    }
}

/// Uma vez por falha em cada versão, e só se o usuário não desligou.
fn key(f: &Failure, version: &str) -> String {
    format!("{}|{}|{version}", f.action, f.code)
}

pub(crate) fn already_offered(offered: &[String], f: &Failure, version: &str) -> bool {
    offered.iter().any(|k| k == &key(f, version))
}

/// "2026-10-27:2" → quantos foram hoje.
pub(crate) fn sent_today(raw: Option<&str>, today: &str) -> u32 {
    raw.and_then(|r| r.split_once(':'))
        .filter(|(day, _)| *day == today)
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(0)
}

fn endpoint() -> Option<String> {
    let url = crate::config::Config::load().report_url.unwrap_or_else(|| DEFAULT_URL.to_string());
    let url = url.trim().to_string();
    (url.starts_with("https://") || url.starts_with("http://127.0.0.1") || url.starts_with("http://localhost")).then_some(url)
}

async fn collect(app: &AppHandle, f: &Failure, source: &str) -> Report {
    let specs = tauri::async_runtime::spawn_blocking(lilith_win::specs::collect).await.ok();
    let cfg = crate::config::Config::load();
    let ollama = lilith_brain::ollama::health(&cfg.ollama).await.ok().map(|(v, _)| v);
    let settings = crate::profile::Settings::load(&app.state::<AppState>().store);
    let specs = specs.unwrap_or(lilith_win::specs::Specs {
        os: String::new(),
        cpu: String::new(),
        cores: 0,
        ram_gb: 0,
        gpu: None,
        browser: None,
        browser_version: None,
    });
    Report {
        v: 1,
        app: app.package_info().version.to_string(),
        action: f.action.clone(),
        code: f.code.clone(),
        os: specs.os,
        cpu: specs.cpu,
        cores: specs.cores,
        ram_gb: specs.ram_gb,
        gpu: specs.gpu,
        browser: specs.browser,
        browser_version: specs.browser_version,
        model: ollama.as_ref().map(|_| cfg.ollama.model.trim().to_string()).filter(|m| !m.is_empty()),
        ollama,
        voice: if settings.voice { settings.voice_engine } else { "off".into() },
        source: source.into(),
        missing: None,
    }
}

/// Resposta da API em português (decidida pelo código).
pub(crate) fn outcome_text(status: u16, body: &serde_json::Value) -> String {
    match (status, body["status"].as_str()) {
        (201, _) => match body["issue"].as_u64() {
            Some(n) => format!("Pronto, avisei o time (#{n}). Obrigada por me ajudar a melhorar!"),
            None => "Pronto, avisei o time. Obrigada por me ajudar a melhorar!".into(),
        },
        (200, Some("known")) => "O time já sabia desse erro — somei mais um caso para ele subir na fila. Obrigada!".into(),
        (429, _) => "Hoje já chegou no limite de avisos. Se o erro continuar, eu ofereço de novo outro dia.".into(),
        _ => "Não consegui enviar o aviso agora. Se o erro voltar, eu ofereço de novo.".into(),
    }
}

async fn send(url: &str, report: &Report) -> String {
    let client = match reqwest::Client::builder().timeout(Duration::from_secs(10)).build() {
        Ok(c) => c,
        Err(_) => return outcome_text(0, &serde_json::Value::Null),
    };
    match client.post(url).header("x-lilith-report", "1").json(report).send().await {
        Ok(res) => {
            let status = res.status().as_u16();
            let body = res.json::<serde_json::Value>().await.unwrap_or_default();
            if status >= 400 {
                tracing::warn!(status, error = %body["error"], "relatório de bug recusado");
            }
            outcome_text(status, &body)
        }
        Err(e) => {
            tracing::warn!(error = %e, "relatório de bug não saiu");
            outcome_text(0, &serde_json::Value::Null)
        }
    }
}

const SEND: &str = "Enviar";
const NOT_NOW: &str = "Agora não";

fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn sends_left(store: &lilith_memory::Memory) -> u32 {
    MAX_SENT_PER_DAY.saturating_sub(sent_today(store.setting(SENT_TODAY).as_deref(), &today()))
}

fn mark_offered(store: &lilith_memory::Memory, offered: &mut Vec<String>, f: &Failure, version: &str) {
    let k = key(f, version);
    if !offered.contains(&k) {
        offered.insert(0, k);
        offered.truncate(200);
        store.set_setting(OFFERED, &serde_json::to_string(offered).unwrap_or_default());
    }
}

async fn count_and_send(store: &lilith_memory::Memory, url: &str, report: &Report) -> String {
    let day = today();
    let sent = sent_today(store.setting(SENT_TODAY).as_deref(), &day);
    store.set_setting(SENT_TODAY, &format!("{day}:{}", sent + 1));
    send(url, report).await
}

/// "Cria uma issue disso": o usuário pediu. Usa a última falha desta conversa
/// (qualquer código — quem decide que foi bug é ele), mostra o que vai e só
/// envia com o "Enviar". Vale mesmo com a oferta automática desligada.
pub async fn report_now(app: &AppHandle, conversation: &str) -> serde_json::Value {
    use serde_json::json;
    let Some(failure) = last_failure(conversation) else {
        return json!({ "status": "none", "text": "Não registrei nenhuma falha minha nesta conversa, então não tenho o que mandar. Se algo deu errado, me pede de novo que eu tento — se falhar, eu consigo reportar." });
    };
    let Some(url) = endpoint() else {
        return json!({ "status": "unavailable", "text": "O envio de relatórios não está configurado nesta versão." });
    };
    let store = app.state::<AppState>().store.clone();
    if sends_left(&store) == 0 {
        return json!({ "status": "limit", "text": "Hoje já mandei o máximo de relatórios. Amanhã eu consigo de novo." });
    }
    let version = app.package_info().version.to_string();
    let mut offered: Vec<String> = store.setting(OFFERED).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
    mark_offered(&store, &mut offered, &failure, &version);

    let report = collect(app, &failure, "user").await;
    let title = format!(
        "Mando para quem me desenvolve? Vai só a ficha do PC e o código da última falha — {}. Nada das suas conversas, arquivos ou nomes.",
        report.summary()
    );
    let choices = app.state::<AppState>().choices.clone();
    match choices.ask(app, conversation, &title, vec![SEND.into(), NOT_NOW.into()]).await.as_deref() {
        Some(SEND) => json!({ "status": "sent", "text": count_and_send(&store, &url, &report).await }),
        _ => json!({ "status": "declined", "text": "Tudo bem, não enviei nada." }),
    }
}

/// O que ela não sabe fazer, pronto para ir numa issue — ou `None`. Só texto
/// genérico: letras, espaços e pontuação simples, sem números (telefone,
/// CPF, datas), @, caminhos ou links. O mesmo filtro roda no Worker.
pub(crate) fn safe_gap(raw: &str) -> Option<String> {
    let t = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.trim_end_matches(['.', ';']).trim();
    let ok_chars = t.chars().all(|c| c.is_alphabetic() || c == ' ' || matches!(c, ',' | '\'' | '(' | ')' | '-'));
    (ok_chars && (3..=100).contains(&t.chars().count())).then(|| t.to_string())
}

/// Fim do turno: ela disse que não sabe fazer algo. Oferece pedir isso como
/// melhoria — uma vez por tarefa em cada versão, com o texto exato no cartão.
pub async fn offer_missing(app: AppHandle, conversation: String, missing: Vec<String>) {
    let state = app.state::<AppState>();
    let store = state.store.clone();
    if !crate::profile::Settings::load(&store).bug_reports || sends_left(&store) == 0 {
        return;
    }
    let Some(url) = endpoint() else { return };
    let version = app.package_info().version.to_string();
    let mut offered: Vec<String> = store.setting(OFFERED).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
    let Some((gap, failure)) = missing.iter().filter_map(|m| safe_gap(m)).find_map(|gap| {
        let f = Failure { action: "capability.missing".into(), code: "MISSING_FEATURE".into() };
        let k = format!("{}|{gap}|{version}", f.action);
        (!offered.contains(&k)).then_some((gap, f))
    }) else {
        return;
    };
    let k = format!("{}|{gap}|{version}", failure.action);
    offered.insert(0, k);
    offered.truncate(200);
    store.set_setting(OFFERED, &serde_json::to_string(&offered).unwrap_or_default());

    let mut report = collect(&app, &failure, "auto").await;
    report.missing = Some(gap.clone());
    let title = format!(
        "Isso eu ainda não sei fazer: «{gap}». Quer que eu peça para quem me desenvolve me ensinar? Vai só esse texto e a ficha do PC — {}.",
        report.summary()
    );
    const NEVER: &str = "Nunca perguntar";
    let choices = state.choices.clone();
    match choices.ask(&app, &conversation, &title, vec!["Pedir".into(), NOT_NOW.into(), NEVER.into()]).await.as_deref() {
        Some("Pedir") => {
            let text = count_and_send(&store, &url, &report).await;
            crate::alerts::notice(&app, &text, "bug");
        }
        Some(NEVER) => {
            let mut settings = crate::profile::Settings::load(&store);
            settings.bug_reports = false;
            settings.save(&store);
            crate::alerts::notice(&app, "Combinado, não pergunto mais. Dá para religar em Configurações.", "bug");
        }
        _ => {}
    }
}

/// Fim do turno: se algo falhou por bug, oferece avisar o time.
pub async fn offer_after_turn(app: AppHandle, conversation: String) {
    let Some(failure) = take_failure(&conversation) else { return };
    let state = app.state::<AppState>();
    let store = state.store.clone();
    if !crate::profile::Settings::load(&store).bug_reports {
        return;
    }
    let Some(url) = endpoint() else {
        tracing::info!(action = %failure.action, "falha de bug, mas a API de relatórios não está configurada");
        return;
    };
    let version = app.package_info().version.to_string();
    let mut offered: Vec<String> = store.setting(OFFERED).and_then(|v| serde_json::from_str(&v).ok()).unwrap_or_default();
    if already_offered(&offered, &failure, &version) {
        return;
    }
    if sends_left(&store) == 0 {
        return;
    }
    mark_offered(&store, &mut offered, &failure, &version);

    let report = collect(&app, &failure, "auto").await;
    let title = format!(
        "Isso falhou por um erro meu. Posso avisar quem me desenvolve? Vai só a ficha do PC e o código do erro — {}. Nada das suas conversas, arquivos ou nomes.",
        report.summary()
    );
    const NEVER: &str = "Nunca perguntar";
    let choices = state.choices.clone();
    match choices.ask(&app, &conversation, &title, vec![SEND.into(), NOT_NOW.into(), NEVER.into()]).await.as_deref() {
        Some(SEND) => {
            let text = count_and_send(&store, &url, &report).await;
            crate::alerts::notice(&app, &text, "bug");
        }
        Some(NEVER) => {
            let mut settings = crate::profile::Settings::load(&store);
            settings.bug_reports = false;
            settings.save(&store);
            crate::alerts::notice(&app, "Combinado, não pergunto mais. Dá para religar em Configurações.", "bug");
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn report() -> Report {
        Report {
            v: 1,
            app: "0.4.1".into(),
            action: "window.read".into(),
            code: "EXECUTION_FAILED".into(),
            os: "Windows 11 25H2 (build 26200)".into(),
            cpu: "AMD Ryzen 9 9900X 12-Core Processor".into(),
            cores: 24,
            ram_gb: 31,
            gpu: Some("NVIDIA GeForce RTX 5070".into()),
            browser: Some("waterfox".into()),
            browser_version: Some("153.5.0".into()),
            ollama: Some("0.12.0".into()),
            model: Some("qwen3.5:4b".into()),
            voice: "kokoro".into(),
            source: "auto".into(),
            missing: None,
        }
    }

    #[test]
    fn only_bug_codes_are_noted() {
        note_failure("c-perm", "file.delete", "PERMISSION_DENIED", "o usuário negou esta ação");
        note_failure("c-missing", "app.open", "NOT_FOUND", "não encontrado: spotify");
        note_failure("c-bug", "window.read", "EXECUTION_FAILED", "falhou: window.read demorou demais e foi interrompida");
        assert_eq!(take_failure("c-perm"), None);
        assert_eq!(take_failure("c-missing"), None);
        assert_eq!(take_failure("c-bug").map(|f| f.action), Some("window.read".into()));
        assert_eq!(take_failure("c-bug"), None, "cada falha é oferecida uma vez");
    }

    #[test]
    fn user_can_report_any_failure_but_not_a_denial() {
        // Caso real: o modelo inventou o caminho do instalador e o usuário pediu a issue.
        note_failure("c-user", "file.run", "NOT_FOUND", "não encontrado: C:\\Users\\x\\Temp\\Setup.exe");
        assert_eq!(take_failure("c-user"), None, "NOT_FOUND não vira oferta automática");
        assert_eq!(last_failure("c-user").map(|f| f.action), Some("file.run".into()));
        note_failure("c-user", "agent.delegate", "PERMISSION_DENIED", "o usuário negou esta ação");
        note_failure("c-user", "self.report", "EXECUTION_FAILED", "falhou");
        assert_eq!(last_failure("c-user").map(|f| f.action), Some("file.run".into()), "negar ou reportar não é a falha");
    }

    #[test]
    fn expected_failures_from_the_real_audit_log_are_not_bugs() {
        assert!(!is_bug("EXECUTION_FAILED", "falhou: 'waterfox.exe' não fechou educadamente — peça para forçar"));
        assert!(!is_bug("EXECUTION_FAILED", "falhou: interrompido pelo usuário"));
        assert!(!is_bug("EXECUTION_FAILED", "falhou: não consegui mover 'Gerenciador de Tarefas' (Acesso negado.)"));
        assert!(!is_bug("EXECUTION_FAILED", "falhou: já estou jogando uma partida (diga \"para de jogar\" para eu sair)"));
        assert!(is_bug("INVALID_PARAMS", "parâmetros inválidos: agente '' não configurado (disponíveis: claude, codex)"));
        assert!(is_bug("INVALID_PARAMS", "parâmetros inválidos: parâmetro 'app' obrigatório"));
    }

    #[test]
    fn payload_has_exactly_the_worker_fields() {
        let v = serde_json::to_value(report()).unwrap();
        let mut keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        keys.sort();
        assert_eq!(
            keys,
            ["action", "app", "browser", "browser_version", "code", "cores", "cpu", "gpu", "missing", "model", "ollama", "os", "ram_gb", "source", "v", "voice"]
        );
    }

    #[test]
    fn summary_lists_what_goes() {
        let s = report().summary();
        assert!(s.contains("Windows 11 25H2") && s.contains("31 GB de RAM") && s.contains("waterfox 153.5.0"));
        assert!(s.ends_with("erro EXECUTION_FAILED em window.read"));
    }

    #[test]
    fn offers_once_per_failure_and_version() {
        let f = Failure { action: "window.read".into(), code: "EXECUTION_FAILED".into() };
        let offered = vec![key(&f, "0.4.1")];
        assert!(already_offered(&offered, &f, "0.4.1"));
        assert!(!already_offered(&offered, &f, "0.4.2"));
    }

    #[test]
    fn only_generic_gaps_can_be_reported() {
        assert_eq!(safe_gap(" editar planilhas do Excel. ").as_deref(), Some("editar planilhas do Excel"));
        assert_eq!(safe_gap("abrir ferramentas do Windows (ex.: serviços)"), None, "dois-pontos fica de fora");
        assert_eq!(safe_gap("ligar para 11 99999-9999"), None, "número de telefone");
        assert_eq!(safe_gap("mandar e-mail para ana@empresa.com"), None);
        assert_eq!(safe_gap(r"ler C:\Users\userx\x.txt"), None);
        assert_eq!(safe_gap("abrir https://x.com"), None);
        assert_eq!(safe_gap("ok"), None, "curto demais");
    }

    #[test]
    fn counts_sends_per_day() {
        assert_eq!(sent_today(Some("2026-10-27:2"), "2026-10-27"), 2);
        assert_eq!(sent_today(Some("2026-10-26:3"), "2026-10-27"), 0);
        assert_eq!(sent_today(None, "2026-10-27"), 0);
    }

    #[test]
    fn api_answers_in_portuguese() {
        assert!(outcome_text(201, &json!({ "status": "created", "issue": 42 })).contains("#42"));
        assert!(outcome_text(200, &json!({ "status": "known" })).contains("já sabia"));
        assert!(outcome_text(429, &json!({})).contains("limite"));
        assert!(outcome_text(0, &json!(null)).contains("Não consegui"));
    }
}

#[cfg(test)]
mod live {
    /// Com o `wrangler dev` rodando em `services/report-worker`:
    /// `cargo test -p lilith-companion report::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn sends_this_pc_to_local_worker() {
        let s = lilith_win::specs::collect();
        let report = super::Report {
            v: 1,
            app: env!("CARGO_PKG_VERSION").into(),
            action: "window.read".into(),
            code: "EXECUTION_FAILED".into(),
            os: s.os,
            cpu: s.cpu,
            cores: s.cores,
            ram_gb: s.ram_gb,
            gpu: s.gpu,
            browser: s.browser,
            browser_version: s.browser_version,
            ollama: None,
            model: None,
            voice: "kokoro".into(),
            source: "auto".into(),
            missing: None,
        };
        let url = "http://127.0.0.1:8787/v1/report";
        for _ in 0..2 {
            println!("{}", tauri::async_runtime::block_on(super::send(url, &report)));
        }
    }
}
