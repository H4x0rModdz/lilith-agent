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
//!
//! Os casos ficam em `eval/cases/*.json` como especificações com modelos de
//! frase (`{app}`, `{person}`…) que são expandidos aqui. Só nomes anônimos.

use async_trait::async_trait;
use lilith_brain::{all_tools, interpret, Agent, Dispatcher, OllamaClient, OllamaConfig, Sink, ToolCall};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
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
    println!("modelo={} modo={} casos={}", cfg.model, if agent_mode { "agent" } else { "interpreter" }, cases.len());
    let ollama = Arc::new(OllamaClient::new(cfg));
    let tools = all_tools();
    // Aquecimento: o primeiro pedido paga o carregamento do modelo.
    let _ = interpret::run(ollama.as_ref(), "oi", &[], &tools).await;

    let mut by_cat: BTreeMap<String, Tally> = BTreeMap::new();
    let mut failures: Vec<String> = Vec::new();
    let started = Instant::now();

    for c in &cases {
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
            let got = interpret::run(ollama.as_ref(), &c.text, &recent, &tools).await;
            let (p, m) = got.map(|i| (i.tools, i.message)).unwrap_or_default();
            (p, m, Vec::new())
        };
        let ms = t.elapsed().as_millis();

        let tally = by_cat.entry(c.cat.clone()).or_default();
        tally.n += 1;
        tally.ms += ms;
        let ok = c.plan.iter().any(|p| *p == plan);
        if ok {
            tally.plan_ok += 1;
        }
        if c.plan.iter().any(|p| core(p) == core(&plan)) {
            tally.core_ok += 1;
        }
        let mut notes = String::new();
        if let Some(check) = &c.message {
            tally.msg_n += 1;
            if message_ok(check, &message) {
                tally.msg_ok += 1;
            } else {
                notes += &format!(" | msg={message:?}");
            }
        }
        if agent_mode {
            for (action, expected) in &c.params {
                tally.param_n += 1;
                let good = calls.iter().any(|x| &x.action == action && params_ok(expected, &x.params));
                if good {
                    tally.param_ok += 1;
                } else {
                    let got = calls.iter().find(|x| &x.action == action).map(|x| x.params.to_string()).unwrap_or_default();
                    notes += &format!(" | {action} params={got}");
                }
            }
        }
        if !ok || !notes.is_empty() {
            failures.push(format!("  ✗ [{}] {} ({ms} ms) plano={plan:?}{notes}", c.cat, c.text));
        } else if verbose {
            println!("  ✓ [{}] {} ({ms} ms)", c.cat, c.text);
        }
    }

    for f in &failures {
        println!("{f}");
    }
    println!("\n{:<14} {:>5} {:>8} {:>9} {:>9} {:>8}", "categoria", "casos", "plano", "mensagem", "params", "ms/caso");
    let mut total = Tally::default();
    for (cat, t) in &by_cat {
        let pct = |a: usize, b: usize| if b == 0 { "-".to_string() } else { format!("{:.0}%", 100.0 * a as f64 / b as f64) };
        println!(
            "{cat:<14} {:>5} {:>8} {:>9} {:>9} {:>8}",
            t.n,
            pct(t.plan_ok, t.n),
            pct(t.msg_ok, t.msg_n),
            pct(t.param_ok, t.param_n),
            t.ms / t.n.max(1) as u128
        );
        total.n += t.n;
        total.plan_ok += t.plan_ok;
        total.msg_n += t.msg_n;
        total.msg_ok += t.msg_ok;
        total.param_n += t.param_n;
        total.param_ok += t.param_ok;
        total.ms += t.ms;
    }
    let pct = |a: usize, b: usize| if b == 0 { 0.0 } else { 100.0 * a as f64 / b as f64 };
    println!(
        "TOTAL: {} casos | plano {:.1}% | mensagem {:.1}% ({}) | params {:.1}% ({}) | {} ms/caso | {:.0}s",
        total.n,
        pct(total.plan_ok, total.n),
        pct(total.msg_ok, total.msg_n),
        total.msg_n,
        pct(total.param_ok, total.param_n),
        total.param_n,
        total.ms / total.n.max(1) as u128,
        started.elapsed().as_secs_f32()
    );
    if let Some(path) = value("--json") {
        let summary = json!({
            "mode": if agent_mode { "agent" } else { "interpreter" },
            "cases": total.n,
            "plan_pct": pct(total.plan_ok, total.n),
            "message_pct": pct(total.msg_ok, total.msg_n),
            "params_pct": pct(total.param_ok, total.param_n),
            "ms_per_case": total.ms / total.n.max(1) as u128,
            "by_category": by_cat.iter().map(|(k, t)| (k.clone(), json!({
                "cases": t.n, "plan_pct": pct(t.plan_ok, t.n), "message_pct": pct(t.msg_ok, t.msg_n),
            }))).collect::<BTreeMap<_, _>>(),
            "failures": failures,
        });
        std::fs::write(path, serde_json::to_string_pretty(&summary).unwrap()).unwrap();
    }
}
