//! Intérprete de intenção: antes do loop principal, uma chamada curta ao
//! mesmo modelo reescreve o pedido do usuário como uma intenção clara e
//! sugere as tools certas. Modelos pequenos tropeçam em gírias ("joga o zap
//! pro lado" virou *fechar* o WhatsApp); reformular primeiro, com saída
//! estruturada, corrige isso sem regras fixas.
//!
//! Best-effort: se a chamada falhar ou vier lixo, o turno segue sem dica.

use crate::knowledge::{self, Example, Knowledge};
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
    /// Texto exato a enviar quando o pedido é mandar uma mensagem: o que o
    /// usuário ditou (entre aspas), ou, se ele só descreveu, uma mensagem
    /// curta na VOZ DELE. Vazio quando não há mensagem. O interpretador só
    /// lê o chat do usuário, então isto nunca vem de conteúdo externo.
    #[serde(default)]
    pub message: String,
    /// As tarefas do pedido, uma por item, na ordem ("instalar o Chrome",
    /// "abrir o Chrome", "pesquisar 'batata' no Chrome").
    #[serde(default)]
    pub steps: Vec<String>,
    /// Tarefas que nenhuma ferramenta do catálogo resolve, descritas de forma
    /// genérica — ela diz que ainda não sabe em vez de improvisar.
    #[serde(default)]
    pub missing: Vec<String>,
}

/// JSON Schema enviado ao Ollama em `format` — força a forma da resposta.
/// Os nomes de ferramenta ficam travados no catálogo (`enum`): o modelo não
/// consegue inventar uma ação.
pub fn schema(tools: &[Tool]) -> Value {
    let names: Vec<&str> = tools.iter().map(|t| t.action).collect();
    json!({
        "type": "object",
        "properties": {
            "intent": { "type": "string" },
            "tools": { "type": "array", "items": { "type": "string", "enum": names }, "maxItems": 12 },
            "message": { "type": "string" },
            "steps": { "type": "array", "items": { "type": "string" }, "maxItems": 12 },
            "missing": { "type": "array", "items": { "type": "string" }, "maxItems": 6 }
        },
        "required": ["intent", "tools", "message", "steps", "missing"]
    })
}

const PROMPT: &str = "\
Você é o intérprete da Lilith, uma assistente que controla o Windows do usuário. \
Leia o último pedido do usuário (use as mensagens anteriores só como contexto) e:
1. Reescreva em `intent` o que ele quer, em português claro e literal, \
resolvendo gírias e referências (\"o zap\" = WhatsApp, \"isso\"/\"ela\" = o que foi \
citado antes, \"joga/manda/puxa pro lado\" = mover a janela, \"a outra tela\" = outro \
monitor, \"some com\" = minimizar).
2. Liste em `tools` o PLANO COMPLETO: as ferramentas do catálogo que resolvem TODAS as \
tarefas do pedido, na ordem de uso, REPETINDO a ferramenta quando a tarefa se repete. Um \
pedido com duas tarefas tem as ferramentas das duas: \"oi pra A no whatsapp e oi pra B no \
discord\" = [ui.open_chat, ui.type, ui.open_chat, ui.type]. Lista vazia se for só conversa.

3. Em `message`, quando o pedido é MANDAR uma mensagem a alguém: o texto exato a enviar. \
Se o usuário ditou o texto (entre aspas ou \"manda um oi\"), use exatamente isso, \nletra por letra, sem acrescentar pontuação, emoji nem maiúscula (\"manda um oi\" = \"oi\"). Se ele só \
descreveu o que quer dizer (\"chama ele pra jogar\", \"pergunta se ele vem\"), escreva você uma \
mensagem curta e natural, na voz DO USUÁRIO, em primeira pessoa, falando COM a pessoa (nunca \
sobre ela), sem inventar fatos. Se não há mensagem, deixe vazio.

4. Em `steps`, as tarefas do pedido, uma por item, curtas e na ordem (\"instale o chrome, abra \
e pesquise batata\" = [\"instalar o Chrome\", \"abrir o Chrome\", \"pesquisar 'batata' no Chrome\"]). \
Conversa = lista vazia.

5. Em `missing`, as tarefas que NENHUMA ferramenta do catálogo resolve, descritas de forma \
genérica, sem nomes de pessoas, caminhos, números ou dados pessoais (ex.: \"editar planilhas do \
Excel\"). Vazio se o catálogo resolve tudo. Nunca force uma ferramenta que não serve para a \
tarefa (um programa do Windows não é um site) — a tarefa vai para `missing`.

Regras: fechar/encerrar/apagar só se o usuário disser isso explicitamente. Mover, \
arrastar, ajustar, organizar ou trocar de tela uma janela NUNCA é fechar. \
\"Mais pro lado\", \"um pouco\", \"um pouquinho\", \"ajusta\" = deslocar levemente (nudge), \
mantendo o tamanho; encaixar em metade/canto/tela cheia só se o usuário nomear essa região. \
Programa aberto x arquivo: \"o que está escrito no bloco de notas\", \"na janela\", \"nessa tela\", \
\"no chat do discord\" = ler a janela aberta (window.read). Um nome ou caminho de arquivo \
(\"relatorio.txt\", \"o arquivo X\", \"na pasta Downloads\") = arquivo no disco (file.read/file.find). \
Memória: \"o que eu te falei sobre X\", \"lembra quando…\", \"naquela conversa de ontem\" = \
procurar nas conversas anteriores (memory.search); \"lembra que…\" = memory.remember. \
Navegador: \"abre o youtube numa janela privada/anônima\" = browser.open com mode private (não app.open); \"fecha essa aba\", \"próxima aba\", \"reabre a aba\" = browser.tab; sem citar o navegador, vale o padrão do usuário. \"abre uma aba e pesquisa X\", \"procura imagens de X\", \"pesquisa X no waterfox e \
vai em imagens\" = browser.search (com kind images/videos/news); NUNCA abrir o navegador de novo \
com app.open se ele já está aberto. \
Mandar mensagem: \"manda um oi pra <pessoa ou grupo> no whatsapp/discord/teams\" = abrir a conversa \
(ui.open_chat, que acha pessoas E grupos pelo nome) e digitar+enviar (ui.type com submit). \
Ler um chat do WhatsApp/Discord/Teams: ui.open_chat e depois window.read.
Navegar dentro de um programa: \"lê o chat da <pessoa> no discord\", \"abre a conversa com X no \
whatsapp\", \"abre o servidor Y\", \"vai na aba Z\" = clicar no item (ui.click); para ler depois, \
window.read.

Catálogo:
";

/// Gírias de chat: só entram no prompt as que aparecem no pedido (economiza
/// tokens e evita distrair o modelo com o que não foi dito).
const SLANG: &[(&str, &str)] = &[
    ("cmg", "comigo"),
    ("ctg", "contigo"),
    ("vc", "você"),
    ("voce", "você"),
    ("pq", "porque"),
    ("tb", "também"),
    ("tbm", "também"),
    ("blz", "beleza"),
    ("mt", "muito"),
    ("mto", "muito"),
    ("hj", "hoje"),
    ("amnh", "amanhã"),
    ("q", "que"),
    ("n", "não"),
    ("nn", "não"),
    ("tmj", "estamos juntos"),
    ("vlw", "valeu"),
    ("flw", "falou"),
    ("bora", "vamos"),
];

/// Nota de glossário para as gírias presentes no texto (vazia se não houver).
/// O glossário pessoal do usuário vale mais que o embutido.
fn slang_note(text: &str, personal: &[(String, String)]) -> String {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let mut found: Vec<String> = personal
        .iter()
        .filter(|(g, _)| words.contains(&g.to_lowercase().as_str()))
        .map(|(g, m)| format!("{g} = {m}"))
        .collect();
    found.extend(
        SLANG
            .iter()
            .filter(|(g, _)| words.contains(g) && !personal.iter().any(|(p, _)| p.eq_ignore_ascii_case(g)))
            .map(|(g, m)| format!("{g} = {m}")),
    );
    if found.is_empty() {
        return String::new();
    }
    let mut note = format!(
        "
Gírias neste pedido (leia com o sentido certo, nunca como nome próprio): {}.",
        found.join(", ")
    );
    if words.contains(&"cmg") {
        note += " \"Chama ele pra jogar cmg\" = convidar a pessoa para jogar COM O USUÁRIO (mensagem do tipo \"bora jogar comigo?\"); não é ligar para ninguém.";
    }
    note
}

/// Monta as mensagens do intérprete: prompt + catálogo resumido + últimas
/// falas (contexto) + o pedido atual.
pub fn messages(user_text: &str, recent: &[(String, String)], tools: &[Tool]) -> Vec<Value> {
    messages_with(user_text, recent, tools, None)
}

/// Como `messages`, com o que a Lilith aprendeu: exemplos de planos parecidos
/// (embutidos + os que deram certo antes) e o glossário pessoal.
pub fn messages_with(
    user_text: &str,
    recent: &[(String, String)],
    tools: &[Tool],
    know: Option<&dyn Knowledge>,
) -> Vec<Value> {
    let (learned, personal): (Vec<Example>, Vec<(String, String)>) =
        know.map(|k| (k.plans(), k.glossary())).unwrap_or_default();
    let examples = knowledge::render(&knowledge::pick(user_text, &learned, 3));
    let catalog: String = tools
        .iter()
        .map(|t| format!("- {}: {}\n", t.action, first_sentence(t.description)))
        .collect();
    let mut out = vec![json!({ "role": "system", "content": format!("{PROMPT}{catalog}{examples}{}", slang_note(user_text, &personal)) })];
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
    i.tools.truncate(12);
    Some(i)
}

/// Dica injetada no loop principal como mensagem de sistema.
pub fn hint(i: &Interpretation) -> String {
    // O que nenhuma ferramenta faz: ela admite em vez de improvisar.
    let missing_note = if i.missing.is_empty() {
        String::new()
    } else {
        format!(
            " Estas tarefas NÃO têm ferramenta: {}. Não improvise com outra ferramenta — faça o resto e diga, \
             no final, que isso você ainda não sabe fazer.",
            i.missing.join("; ")
        )
    };
    if i.tools.is_empty() && !i.missing.is_empty() {
        format!("Interpretação do pedido: {}.{missing_note}", i.intent)
    } else if i.tools.is_empty() {
        format!("Interpretação do pedido: {}. (Parece conversa — responda em texto.)", i.intent)
    } else {
        let message = i.message.trim();
        let message_note = if message.is_empty() {
            String::new()
        } else {
            // O texto já está pronto e na voz do usuário: o modelo principal não reescreve.
            format!(" Mensagem a enviar (use EXATAMENTE este texto no `text` de ui.type ou phone.send_sms, sem mudar nada): «{message}».")
        };
        format!(
            "Interpretação do pedido: {}. Plano completo, em ordem: {}. Faça TODAS as etapas que o plano \
             cobre, uma por vez, e só responda em texto no final. Se uma etapa falhar, não faça as que \
             dependem dela (ex.: não pesquisar em outro navegador se o pedido era no Chrome e ele não \
             instalou) — conte o que fez e o que não fez.{missing_note}{message_note}",
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
    run_with(ollama, user_text, recent, tools, None).await
}

pub async fn run_with(
    ollama: &dyn Ollama,
    user_text: &str,
    recent: &[(String, String)],
    tools: &[Tool],
    know: Option<&dyn Knowledge>,
) -> Option<Interpretation> {
    let started = std::time::Instant::now();
    let raw = ollama.structured(&messages_with(user_text, recent, tools, know), &schema(tools)).await;
    let parsed = match raw {
        Ok(v) => parse(&v, tools).map(|mut i| {
            // Correções determinísticas dos erros previsíveis do modelo pequeno.
            i.tools = crate::refine::plan(user_text, std::mem::take(&mut i.tools), recent);
            i.message = crate::refine::message(user_text, &i.tools, std::mem::take(&mut i.message), recent);
            i
        }),
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
        assert_eq!(i.tools, vec!["window.move", "window.move"], "inventada some; repetida fica (plano com 2 tarefas)");
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
    fn repeated_tools_stay_in_the_plan() {
        let tools = all_tools();
        let i = parse(
            &json!({ "intent": "duas mensagens", "tools": ["ui.open_chat", "ui.type", "ui.open_chat", "ui.type"] }),
            &tools,
        )
        .unwrap();
        assert_eq!(i.tools, vec!["ui.open_chat", "ui.type", "ui.open_chat", "ui.type"]);
    }

    #[test]
    fn hint_mentions_tools() {
        let h = hint(&Interpretation { intent: "mover o WhatsApp".into(), tools: vec!["window.move".into()], message: String::new(), steps: vec![], missing: vec![] });
        assert!(h.contains("window.move"));
        assert!(!h.contains("Mensagem a enviar"));
    }

    #[test]
    fn hint_admits_what_has_no_tool() {
        let i = Interpretation {
            intent: "editar a planilha".into(),
            tools: vec![],
            message: String::new(),
            steps: vec!["editar a planilha".into()],
            missing: vec!["editar planilhas do Excel".into()],
        };
        let h = hint(&i);
        assert!(h.contains("NÃO têm ferramenta: editar planilhas do Excel"));
        assert!(!h.contains("Parece conversa"));
        // O conselho antigo empurrava a improvisar.
        assert!(!hint(&Interpretation { missing: vec![], tools: vec!["app.open".into()], ..i }).contains("em vez de dizer"));
    }

    #[test]
    fn composed_message_goes_into_the_hint_verbatim() {
        let i = Interpretation {
            intent: "convidar o user X para jogar".into(),
            tools: vec!["ui.open_chat".into(), "ui.type".into()],
            message: "Bora jogar comigo?".into(),
            steps: vec![],
            missing: vec![],
        };
        assert!(hint(&i).contains("«Bora jogar comigo?»"));
    }
}
