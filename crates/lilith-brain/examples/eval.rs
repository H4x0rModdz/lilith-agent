//! Bateria de avaliação da Lilith: mede o acerto do intérprete (plano e
//! mensagem) e, no modo `agent`, do laço completo (ferramentas chamadas e
//! parâmetros), com o modelo configurado.
//!
//!   cargo run --release -p lilith-brain --example eval -- [opções]
//!
//!   --mode interpreter|agent   (padrão: interpreter; agent é ~3x mais lento)
//!   --cat NOME                 só uma categoria (pode repetir)
//!   --limit N                  no máximo N casos (amostra espaçada, repetível)
//!   --json CAMINHO             grava o resumo (para comparar antes/depois)
//!   --verbose                  mostra também os acertos
//!   --all                      roda TODOS os casos (padrão: só os que falharam
//!                              e os novos/editados desde a última rodada)
//!
//! Os casos ficam em `eval/cases/*.json` como especificações com modelos de
//! frase (`{app}`, `{person}`…) que são expandidos aqui. Só nomes anônimos.

use async_trait::async_trait;
use lilith_brain::{all_tools, interpret, shortcut, Agent, Dispatcher, OllamaClient, OllamaConfig, Sink, ToolCall};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Deserialize, Clone, Default)]
struct MessageCheck {
    equals: Option<String>,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    contains_any: Vec<String>,
}

#[derive(Deserialize)]
struct Spec {
    cat: String,
    /// Frase única, ou...
    text: Option<String>,
    /// ...modelos com `{slot}`.
    #[serde(default)]
    templates: Vec<String>,
    #[serde(default)]
    slots: HashMap<String, Vec<String>>,
    /// Alternativas aceitas de plano (ordem importa). `[[]]` = só conversa.
    plan: Vec<Vec<String>>,
    #[serde(default)]
    message: Option<MessageCheck>,
    /// Parâmetros esperados por ação (só no modo agent). Valor "~x" = contém x.
    #[serde(default)]
    params: HashMap<String, Value>,
    /// Pedidos anteriores: [[fala do usuário, resposta com "[ações nesse pedido: …]"]].
    #[serde(default)]
    context: Vec<[String; 2]>,
    /// Máximo de expansões por modelo de frase.
    max_per_template: Option<usize>,
}

#[derive(Clone)]
struct Case {
    cat: String,
    text: String,
    plan: Vec<Vec<String>>,
    message: Option<MessageCheck>,
    params: HashMap<String, Value>,
    context: Vec<[String; 2]>,
}

fn expand(spec: &Spec) -> Vec<Case> {
    let make = |text: String| Case {
        cat: spec.cat.clone(),
        text,
        plan: spec.plan.clone(),
        message: spec.message.clone(),
        params: spec.params.clone(),
        context: spec.context.clone(),
    };
    let mut out = Vec::new();
    if let Some(t) = &spec.text {
        out.push(make(t.clone()));
    }
    let cap = spec.max_per_template.unwrap_or(4);
    for tpl in &spec.templates {
        // Slots usados neste modelo, em ordem de aparição.
        let mut used: Vec<&String> = spec.slots.keys().filter(|k| tpl.contains(&format!("{{{k}}}"))).collect();
        used.sort();
        if used.is_empty() {
            out.push(make(tpl.clone()));
            continue;
        }
        let total: usize = used.iter().map(|k| spec.slots[*k].len().max(1)).product();
        let take = cap.min(total);
        for n in 0..take {
            // Índice espaçado no produto cartesiano (determinístico).
            let mut idx = n * total / take;
            let mut text = tpl.clone();
            for k in used.iter().rev() {
                let vals = &spec.slots[*k];
                let v = &vals[idx % vals.len()];
                idx /= vals.len();
                text = text.replace(&format!("{{{k}}}"), v);
            }
            out.push(make(text));
        }
    }
    out
}

fn load_cases(only: &[String]) -> Vec<Case> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("eval").join("cases");
    let mut files: Vec<_> = std::fs::read_dir(&dir).expect("eval/cases").flatten().map(|e| e.path()).collect();
    files.sort();
    let mut cases = Vec::new();
    for f in files.into_iter().filter(|p| p.extension().is_some_and(|e| e == "json")) {
        let specs: Vec<Spec> = serde_json::from_str(&std::fs::read_to_string(&f).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", f.display()));
        for s in &specs {
            if only.is_empty() || only.contains(&s.cat) {
                cases.extend(expand(s));
            }
        }
    }
    cases
}

fn message_ok(check: &MessageCheck, msg: &str) -> bool {
    let m = msg.trim().to_lowercase();
    check.equals.as_ref().is_none_or(|e| m == e.to_lowercase())
        && check.contains.iter().all(|k| m.contains(&k.to_lowercase()))
        && (check.contains_any.is_empty() || check.contains_any.iter().any(|k| m.contains(&k.to_lowercase())))
}

/// Parâmetros esperados batem com os chamados (valor "~x" = contém x).
fn params_ok(expected: &Value, got: &Value) -> bool {
    let (Some(e), Some(g)) = (expected.as_object(), got.as_object()) else { return false };
    e.iter().all(|(k, v)| {
        let Some(gv) = g.get(k) else { return false };
        match (v.as_str(), gv.as_str()) {
            (Some(ev), Some(gs)) => match ev.strip_prefix('~') {
                Some(sub) => gs.to_lowercase().contains(&sub.to_lowercase()),
                None => gs.to_lowercase() == ev.to_lowercase(),
            },
            _ => v == gv,
        }
    })
}

/// Plano "núcleo": sem os passos que só preparam o terreno e não mudam o
/// resultado (olhar a lista de janelas, focar antes de mover, achar o
/// arquivo antes de abrir). Mede se o EFEITO está certo, sem punir o
/// caminho um pouco mais longo.
fn core(plan: &[String]) -> Vec<String> {
    plan.iter()
        .filter(|a| !matches!(a.as_str(), "window.list" | "window.focus" | "file.find" | "ui.find"))
        .cloned()
        .collect()
}

struct Recorder(Mutex<Vec<ToolCall>>);

#[async_trait]
impl Dispatcher for Recorder {
    async fn dispatch(&self, call: &ToolCall) -> Value {
        self.0.lock().unwrap().push(call.clone());
        let result = match call.action.as_str() {
            "window.read" => json!({ "window": "Chat", "text": "[12:00] user x , olá , 12:00" }),
            "memory.search" => json!({ "results": [] }),
            "window.list" => json!({ "windows": [{ "title": "App", "app": "app" }] }),
            _ => json!({}),
        };
        json!({ "action": call.action, "status": "success", "result": result })
    }
}

struct Quiet;
#[async_trait]
impl Sink for Quiet {
    async fn partial(&self, _: u32, _: &str) {}
    async fn step(&self, _: u32, _: &str) {}
}

/// Resultado de um caso. Fica guardado entre rodadas para reexecutar só o
/// que falhou.
#[derive(Serialize, Deserialize, Clone, Default)]
struct Outcome {
    cat: String,
    text: String,
    plan_ok: bool,
    core_ok: bool,
    /// `None` quando o caso não checa mensagem.
    msg: Option<bool>,
    params_n: usize,
    params_ok: usize,
    ms: u128,
    /// O que ela fez de errado (vazio se passou).
    note: String,
}

impl Outcome {
    fn passed(&self) -> bool {
        self.plan_ok && self.msg.unwrap_or(true) && self.params_n == self.params_ok
    }
}

/// Assinatura do caso: muda se a frase, o contexto ou qualquer expectativa
/// for editada — aí o caso volta a ser executado.
fn signature(c: &Case) -> String {
    let params: BTreeMap<_, _> = c.params.iter().collect();
    json!({
        "cat": c.cat, "text": c.text, "plan": c.plan, "params": params, "context": c.context,
        "message": c.message.as_ref().map(|m| json!([m.equals, m.contains, m.contains_any])),
    })
    .to_string()
}

fn state_path(agent_mode: bool, model: &str) -> PathBuf {
    let safe: String = model.chars().map(|ch| if ch.is_alphanumeric() { ch } else { '_' }).collect();
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("target")
        .join("eval-state")
        .join(format!("{}-{safe}.json", if agent_mode { "agent" } else { "interpreter" }))
}

#[derive(Default, Clone)]
struct Tally {
    n: usize,
    plan_ok: usize,
    core_ok: usize,
    msg_n: usize,
    msg_ok: usize,
    param_n: usize,
    param_ok: usize,
    ms: u128,
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| args.iter().position(|a| a == name);
    let value = |name: &str| flag(name).and_then(|i| args.get(i + 1)).cloned();
    let agent_mode = value("--mode").as_deref() == Some("agent");
    let verbose = flag("--verbose").is_some();
    let run_all = flag("--all").is_some();
    let only: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(_, a)| *a == "--cat")
        .filter_map(|(i, _)| args.get(i + 1).cloned())
        .collect();
    let mut cases = load_cases(&only);
    if let Some(limit) = value("--limit").and_then(|v| v.parse::<usize>().ok()) {
        if limit < cases.len() {
            let total = cases.len();
            cases = (0..limit).map(|i| cases[i * total / limit].clone()).collect();
        }
    }

    let cfg = OllamaConfig::default();
    let path = state_path(agent_mode, &cfg.model);
    let mut state: HashMap<String, Outcome> =
        std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    // Roda o que nunca rodou, o que falhou e o que foi editado.
    let pending: Vec<&Case> = cases
        .iter()
        .filter(|c| run_all || state.get(&signature(c)).is_none_or(|o| !o.passed()))
        .collect();
    let carried = cases.len() - pending.len();
    println!(
        "modelo={} modo={} casos={} | a executar={} | já aprovados antes={carried}",
        cfg.model,
        if agent_mode { "agent" } else { "interpreter" },
        cases.len(),
        pending.len()
    );

    let ollama = Arc::new(OllamaClient::new(cfg));
    let tools = all_tools();
    let started = Instant::now();
    if !pending.is_empty() {
        // Aquecimento: o primeiro pedido paga o carregamento do modelo.
        let _ = interpret::run(ollama.as_ref(), "oi", &[], &tools).await;
    }

    for c in &pending {
        let t = Instant::now();
        let (plan, message, calls): (Vec<String>, String, Vec<ToolCall>) = if agent_mode {
            let agent = Agent::new(ollama.clone());
            // Contexto de conversa: reproduz os pedidos anteriores.
            for [u, r] in &c.context {
                agent.seed(vec![(u.clone(), r.clone())]);
            }
            let rec = Recorder(Mutex::new(Vec::new()));
            agent.process(&c.text, &rec, Arc::new(Quiet)).await;
            let calls = rec.0.into_inner().unwrap();
            let msg = calls
                .iter()
                .find(|x| x.action == "ui.type")
                .and_then(|x| x.params["text"].as_str())
                .unwrap_or("")
                .to_string();
            (calls.iter().map(|x| x.action.clone()).collect(), msg, calls)
        } else {
            let recent: Vec<(String, String)> = c.context.iter().map(|[u, r]| (u.clone(), r.clone())).collect();
            // Atalho sem IA primeiro, como no agente.
            if let Some(sc) = shortcut::parse(&c.text) {
                (sc.calls.iter().map(|x| x.action.clone()).collect(), String::new(), Vec::new())
            } else {
                let got = interpret::run(ollama.as_ref(), &c.text, &recent, &tools).await;
                let (p, m) = got.map(|i| (i.tools, i.message)).unwrap_or_default();
                (p, m, Vec::new())
            }
        };

        let mut out = Outcome {
            cat: c.cat.clone(),
            text: c.text.clone(),
            plan_ok: c.plan.iter().any(|p| *p == plan),
            core_ok: c.plan.iter().any(|p| core(p) == core(&plan)),
            ms: t.elapsed().as_millis(),
            ..Default::default()
        };
        let mut notes = String::new();
        if let Some(check) = &c.message {
            let good = message_ok(check, &message);
            out.msg = Some(good);
            if !good {
                notes += &format!(" | msg={message:?}");
            }
        }
        if agent_mode {
            for (action, expected) in &c.params {
                out.params_n += 1;
                if calls.iter().any(|x| &x.action == action && params_ok(expected, &x.params)) {
                    out.params_ok += 1;
                } else {
                    let got = calls.iter().find(|x| &x.action == action).map(|x| x.params.to_string()).unwrap_or_default();
                    notes += &format!(" | {action} params={got}");
                }
            }
        }
        if !out.passed() {
            out.note = format!("plano={plan:?}{notes}");
        } else if verbose {
            println!("  ✓ [{}] {} ({} ms)", c.cat, c.text, out.ms);
        }
        state.insert(signature(c), out);
    }

    // Guarda para a próxima rodada (sem apagar o que esta rodada não tocou).
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, serde_json::to_string(&state).unwrap());

    // Relatório do conjunto selecionado, com os resultados guardados.
    let selected: Vec<&Outcome> = cases.iter().filter_map(|c| state.get(&signature(c))).collect();
    let mut failures: Vec<String> = Vec::new();
    let mut by_cat: BTreeMap<String, Tally> = BTreeMap::new();
    for o in &selected {
        let t = by_cat.entry(o.cat.clone()).or_default();
        t.n += 1;
        t.ms += o.ms;
        t.plan_ok += o.plan_ok as usize;
        t.core_ok += o.core_ok as usize;
        if let Some(m) = o.msg {
            t.msg_n += 1;
            t.msg_ok += m as usize;
        }
        t.param_n += o.params_n;
        t.param_ok += o.params_ok;
        if !o.passed() {
            failures.push(format!("  ✗ [{}] {} ({} ms) {}", o.cat, o.text, o.ms, o.note));
        }
    }
    for f in &failures {
        println!("{f}");
    }
    println!("\n{:<14} {:>5} {:>8} {:>8} {:>9} {:>9} {:>8}", "categoria", "casos", "plano", "núcleo", "mensagem", "params", "ms/caso");
    let mut total = Tally::default();
    for (cat, t) in &by_cat {
        let pct = |a: usize, b: usize| if b == 0 { "-".to_string() } else { format!("{:.0}%", 100.0 * a as f64 / b as f64) };
        println!(
            "{cat:<14} {:>5} {:>8} {:>8} {:>9} {:>9} {:>8}",
            t.n,
            pct(t.plan_ok, t.n),
            pct(t.core_ok, t.n),
            pct(t.msg_ok, t.msg_n),
            pct(t.param_ok, t.param_n),
            t.ms / t.n.max(1) as u128
        );
        total.n += t.n;
        total.plan_ok += t.plan_ok;
        total.core_ok += t.core_ok;
        total.msg_n += t.msg_n;
        total.msg_ok += t.msg_ok;
        total.param_n += t.param_n;
        total.param_ok += t.param_ok;
        total.ms += t.ms;
    }
    let pct = |a: usize, b: usize| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
    println!(
        "TOTAL: {} casos | plano {:.1}% | núcleo {:.1}% | mensagem {:.1}% ({}) | params {:.1}% ({}) | {} ms/caso | {:.0}s",
        total.n,
        pct(total.plan_ok, total.n),
        pct(total.core_ok, total.n),
        pct(total.msg_ok, total.msg_n),
        total.msg_n,
        pct(total.param_ok, total.param_n),
        total.param_n,
        total.ms / total.n.max(1) as u128,
        started.elapsed().as_secs_f32()
    );
    if carried > 0 {
        println!("⚠ {carried} casos aprovados em rodadas anteriores não foram reexecutados; use --all para confirmar que nada regrediu.");
    }
    if let Some(path) = value("--json") {
        let summary = json!({
            "mode": if agent_mode { "agent" } else { "interpreter" },
            "cases": total.n,
            "plan_pct": pct(total.plan_ok, total.n),
            "core_pct": pct(total.core_ok, total.n),
            "message_pct": pct(total.msg_ok, total.msg_n),
            "params_pct": pct(total.param_ok, total.param_n),
            "ms_per_case": total.ms / total.n.max(1) as u128,
            "by_category": by_cat.iter().map(|(k, t)| (k.clone(), json!({
                "cases": t.n, "plan_pct": pct(t.plan_ok, t.n), "core_pct": pct(t.core_ok, t.n),
                "message_pct": pct(t.msg_ok, t.msg_n),
            }))).collect::<BTreeMap<_, _>>(),
            "failures": failures,
        });
        std::fs::write(path, serde_json::to_string_pretty(&summary).unwrap()).unwrap();
    }
}
