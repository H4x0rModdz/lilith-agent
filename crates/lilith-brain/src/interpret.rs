//! Intérprete de intenção: antes do loop principal, uma chamada curta ao
//! mesmo modelo reescreve o pedido do usuário como uma intenção clara e
//! sugere as tools certas. Modelos pequenos tropeçam em gírias ("joga o zap
//! pro lado" virou *fechar* o WhatsApp); reformular primeiro, com saída
//! estruturada, corrige isso sem regras fixas.
//!
//! Best-effort: se a chamada falhar ou vier lixo, o turno segue sem dica.

use crate::ollama::Ollama;
use crate::tools::Tool;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Interpretation {
    /// O pedido reescrito em português claro e literal.
    pub intent: String,
    /// Tools do catálogo que resolvem o pedido (0 = é só conversa).
    #[serde(default)]
    pub tools: Vec<String>,
}

/// JSON Schema enviado ao Ollama em `format` — força a forma da resposta.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "intent": { "type": "string" },
            "tools": { "type": "array", "items": { "type": "string" }, "maxItems": 3 }
        },
        "required": ["intent", "tools"]
    })
}

const PROMPT: &str = "\
Você é o intérprete da Lilith, uma assistente que controla o Windows do usuário. \
Leia o último pedido do usuário (use as mensagens anteriores só como contexto) e:
1. Reescreva em `intent` o que ele quer, em português claro e literal, \
resolvendo gírias e referências (\"o zap\" = WhatsApp, \"isso\"/\"ela\" = o que foi \
citado antes, \"joga/manda/puxa pro lado\" = mover a janela, \"a outra tela\" = outro \
monitor, \"some com\" = minimizar).
2. Liste em `tools` as ferramentas do catálogo que resolvem o pedido, na ordem de uso \
(no máximo 3). Lista vazia se for só conversa.

Regras: fechar/encerrar/apagar só se o usuário disser isso explicitamente. Mover, \
arrastar, ajustar, organizar ou trocar de tela uma janela NUNCA é fechar. \
\"Mais pro lado\", \"um pouco\", \"um pouquinho\", \"ajusta\" = deslocar levemente (nudge), \
mantendo o tamanho; encaixar em metade/canto/tela cheia só se o usuário nomear essa região.

Catálogo:
";

/// Monta as mensagens do intérprete: prompt + catálogo resumido + últimas
/// falas (contexto) + o pedido atual.
pub fn messages(user_text: &str, recent: &[(String, String)], tools: &[Tool]) -> Vec<Value> {
    let catalog: String = tools
        .iter()
        .map(|t| format!("- {}: {}\n", t.action, first_sentence(t.description)))
        .collect();
    let mut out = vec![json!({ "role": "system", "content": format!("{PROMPT}{catalog}") })];
    for (user, reply) in recent {
        out.push(json!({ "role": "user", "content": user }));
        out.push(json!({ "role": "assistant", "content": reply }));
    }
    out.push(json!({ "role": "user", "content": user_text }));
    out
}

/// Primeira frase da descrição — o catálogo inteiro deixaria a chamada lenta.
fn first_sentence(s: &str) -> String {
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    match s.find(". ") {
        Some(i) => s[..=i].to_string(),
        None => s,
    }
}

/// Valida a resposta: descarta tools inventadas e intenção vazia.
pub fn parse(raw: &Value, tools: &[Tool]) -> Option<Interpretation> {
    let mut i: Interpretation = serde_json::from_value(raw.clone()).ok()?;
    i.intent = i.intent.trim().to_string();
    if i.intent.is_empty() {
        return None;
    }
    i.tools.retain(|name| tools.iter().any(|t| t.action == name));
    i.tools.dedup();
    Some(i)
}

/// Dica injetada no loop principal como mensagem de sistema.
pub fn hint(i: &Interpretation) -> String {
    if i.tools.is_empty() {
        format!("Interpretação do pedido: {}. (Parece conversa — responda em texto.)", i.intent)
    } else {
        format!(
            "Interpretação do pedido: {}. Ferramentas indicadas, em ordem: {}. \
             Use-as em vez de dizer que não é possível.",
            i.intent,
            i.tools.join(", ")
        )
    }
}

/// Reduz o catálogo às famílias (`window.*`, `audio.*`, …) das tools
/// indicadas. Com 30+ tools o modelo de 4B se confunde (pediu "metade
/// direita do chrome" e recebeu um tile do Firefox); com só a família certa,
/// acerta. Família inteira, e não só as tools exatas, para deixar margem a
/// um intérprete que errou o detalhe. Sem indicação, catálogo completo.
pub fn narrow(tools: Vec<Tool>, suggested: &[String]) -> Vec<Tool> {
    let family = |action: &str| action.split('.').next().unwrap_or(action).to_string();
    let mut families: Vec<String> = suggested.iter().map(|s| family(s)).collect();
    // Baixar quase sempre começa achando o link certo.
    if families.iter().any(|f| f == "download") {
        families.push("web".into());
    }
    if families.is_empty() {
        return tools;
    }
    tools.into_iter().filter(|t| families.contains(&family(t.action))).collect()
}

pub async fn run(ollama: &dyn Ollama, user_text: &str, recent: &[(String, String)], tools: &[Tool]) -> Option<Interpretation> {
    let started = std::time::Instant::now();
    let raw = ollama.structured(&messages(user_text, recent, tools), &schema()).await;
    let parsed = match raw {
        Ok(v) => parse(&v, tools),
        Err(e) => {
            tracing::debug!("intérprete indisponível: {e}");
            None
        }
    };
    tracing::info!(ms = started.elapsed().as_millis() as u64, interpretation = ?parsed, "intérprete");
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::all_tools;

    #[test]
    fn parse_drops_invented_tools_and_empty_intent() {
        let tools = all_tools();
        let i = parse(
            &json!({ "intent": " mover a janela do WhatsApp para a esquerda ", "tools": ["window.move", "window.drag_magic", "window.move"] }),
            &tools,
        )
        .unwrap();
        assert_eq!(i.intent, "mover a janela do WhatsApp para a esquerda");
        assert_eq!(i.tools, vec!["window.move"]);
        assert!(parse(&json!({ "intent": "  ", "tools": [] }), &tools).is_none());
        assert!(parse(&json!({ "nope": 1 }), &tools).is_none());
    }

    #[test]
    fn catalog_is_compact() {
        let m = messages("oi", &[], &all_tools());
        let system = m[0]["content"].as_str().unwrap();
        assert!(system.contains("- window.move:"));
        // Só a primeira frase de cada descrição.
        assert!(!system.contains("right_two_thirds"));
    }

    #[test]
    fn narrow_keeps_whole_families() {
        let names = |tools: Vec<Tool>| tools.into_iter().map(|t| t.action).collect::<Vec<_>>();
        let only_windows = names(narrow(all_tools(), &["window.move".into()]));
        assert!(only_windows.contains(&"window.tile") && only_windows.contains(&"window.list"));
        assert!(only_windows.iter().all(|a| a.starts_with("window.")));

        let chain = names(narrow(all_tools(), &["download.file".into()]));
        assert!(chain.contains(&"web.search") && chain.contains(&"download.file"));
        assert!(!chain.contains(&"window.move"));

        assert_eq!(narrow(all_tools(), &[]).len(), all_tools().len());
    }

    #[test]
    fn hint_mentions_tools() {
        let h = hint(&Interpretation { intent: "mover o WhatsApp".into(), tools: vec!["window.move".into()] });
        assert!(h.contains("window.move"));
    }
}
