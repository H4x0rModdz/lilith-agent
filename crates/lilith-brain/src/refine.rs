//! Correções determinísticas do plano do intérprete. O modelo pequeno repete
//! alguns erros previsíveis (encaixar UMA janela com `window.tile`, abrir um
//! site como se fosse programa, puxar etapas de chat para um pedido de janela,
//! repetir a ação do pedido anterior). Cada regra olha só o texto do usuário
//! e é conservadora: na dúvida, deixa o plano como veio.

use serde_json::Value;

fn has(text: &str, words: &[&str]) -> bool {
    words.iter().any(|w| text.contains(w))
}

/// Verbos/palavras de quem quer falar com alguém.
const CHAT_WORDS: &[&str] = &[
    "manda ", "mande", "envia", "enviar", "diz ", "diga", "fala ", "fale ", "pergunta", "avisa", "chama", "convida",
    "agradece", "lê ", "le ", "chat", "mensagem", "conversa", "grupo", "send ", "text ", "tell ", "ask ", "message",
    "responde", "cobra",
];

/// Palavras de posição de janela: o pedido é sobre a tela, não sobre um chat.
const LAYOUT_WORDS: &[&str] = &[
    "tela", "monitor", "canto", "metade", "pro lado", "para o lado", "lado a lado", "visíveis", "visiveis", "screen",
    "direita", "esquerda", "left", "right",
];

const AUDIO_WORDS: &[&str] = &["volume", "som", "áudio", "audio", "mudo", "muta", "mute", "sound", "barulho", "silencia", "baixo", "alto", "aumenta", "diminui"];

const TILE_WORDS: &[&str] = &[
    "divide", "dividir", "lado a lado", "organiza", "visíveis", "visiveis", "grade", "todas as janelas", "side by side",
    "tile",
];

/// Aplica as regras ao plano do intérprete para o pedido `text`.
pub fn plan(text: &str, plan: Vec<String>) -> Vec<String> {
    let t = text.to_lowercase();
    let mut plan = plan;
    let is = |plan: &[String], a: &str| plan.iter().any(|p| p == a);

    // Etapas de chat só quando o pedido fala com alguém (e não é de layout).
    if (!has(&t, CHAT_WORDS) || has(&t, LAYOUT_WORDS)) && (is(&plan, "ui.open_chat") || is(&plan, "ui.type")) {
        plan.retain(|p| p != "ui.open_chat" && p != "ui.type");
    }

    // Uma janela só: `window.tile` divide a tela entre VÁRIAS.
    if is(&plan, "window.tile") && !has(&t, TILE_WORDS) {
        let mut out: Vec<String> = Vec::new();
        for p in plan.drain(..) {
            let p = if p == "window.tile" { "window.move".to_string() } else { p };
            if p != "window.move" || !out.contains(&p) {
                out.push(p);
            }
        }
        plan = out;
    }
    plan.dedup_by(|b, a| *a == "window.tile" && *b == "window.tile");

    // Pesquisa já abre a aba sozinha e escolhe Imagens/Notícias.
    if is(&plan, "browser.search") {
        plan.retain(|p| !matches!(p.as_str(), "browser.open" | "browser.tab" | "app.open"));
    }

    // Baixar não abre programa.
    if is(&plan, "download.file") {
        plan.retain(|p| p != "app.open");
        if !has(&t, &["baix", "download", "http", "instala"]) {
            let mover = has(&t, &["manda", "move", "joga", "leva", "coloca", "põe", "poe"]);
            plan = plan.into_iter().map(|p| if p == "download.file" && mover { "file.move".into() } else { p }).collect();
            plan.retain(|p| p != "download.file");
        }
    }

    // Apagar/lixeira é file.delete, não mover.
    if is(&plan, "file.move") && has(&t, &["lixeira", "apaga", "deleta", "exclui", "remove", "delete", "trash"]) {
        plan = plan.into_iter().map(|p| if p == "file.move" { "file.delete".into() } else { p }).collect();
        plan.dedup();
    }

    // Só executa arquivo quando pediu para executar/instalar.
    if is(&plan, "file.run") && !has(&t, &["roda", "executa", "instala", "run ", ".exe", ".msi", ".bat", "setup"]) {
        plan = plan.into_iter().map(|p| if p == "file.run" { "file.open".into() } else { p }).collect();
    }

    // Listar uma pasta não clica em nada.
    if is(&plan, "file.list") && !has(&t, &["clica", "aperta", "click"]) {
        plan.retain(|p| p != "ui.click");
    }

    // Site não é programa: "abre o github" é navegador.
    if is(&plan, "app.open") && crate::shortcut::mentions_site(&t) && !has(&t, &["app", "programa", "aplicativo"]) {
        plan = plan.into_iter().map(|p| if p == "app.open" { "browser.open".into() } else { p }).collect();
        plan.dedup();
    }

    // Fechar à força é app.close.
    if is(&plan, "window.close") && has(&t, &["força", "forca", "mata", "encerra", "kill", "processo"]) {
        plan = plan.into_iter().map(|p| if p == "window.close" { "app.close".into() } else { p }).collect();
    }

    // "Abre outra janela do X" = app.open com nova janela, não aba.
    if has(&t, &["outra janela", "mais um ", "mais uma ", "outra instância"])
        && !has(&t, &["aba", "tab", "navegador"])
        && is(&plan, "browser.tab")
    {
        plan = vec!["app.open".into()];
    }

    // Áudio só com palavra de áudio.
    if !has(&t, AUDIO_WORDS) {
        plan.retain(|p| !p.starts_with("audio."));
    }
    // Memória só quando pediu para lembrar/procurar o que foi dito.
    if !has(&t, &["lembra", "anota", "guarda", "salva", "memoriza", "remember", "grava"]) {
        plan.retain(|p| p != "memory.remember");
    }
    if !has(&t, &["lembra", "falei", "falou", "falamos", "falado", "combinamos", "combinado", "mencionei", "comentei", "conversa", "conversamos", "disse", "ontem", "antes", "remember", "told"]) {
        plan.retain(|p| p != "memory.search");
    }
    // Definição/tradução se responde de cabeça.
    if has(&t, &["significa", "quer dizer", "traduz", "translate", "meaning of"])
        && !has(&t, &["pesquis", "procura", "google", "internet", "web", "busca", "search"])
    {
        plan.retain(|p| p != "web.search");
    }
    // Só olha a lista de janelas quando a pergunta é sobre janelas/telas.
    if !has(&t, &["janela", "aberto", "abertos", "tela", "monitor", "onde", "window", "open", "organiza", "qual"]) {
        plan.retain(|p| p != "window.list");
    }

    // Repetição: o modelo repete o passo do pedido anterior. Só some quando o
    // pedido não pede dois (conjunção, vírgula, "todos") ou é continuação.
    let followup = t.starts_with("e ") || has(&t, &["também", "tambem", "de novo", "o mesmo", "a mesma"]);
    let plural = has(&t, &[" e ", ",", " and ", "dois", "duas", "todos", "todas", "ambos"]);
    if followup || !plural {
        plan.dedup_by(|b, a| a == b && !a.starts_with("ui."));
    }
    plan
}

/// Última mensagem digitada (`ui.type`) nas ações do pedido anterior.
fn last_typed(recent: &[(String, String)]) -> Option<String> {
    recent.iter().rev().find_map(|(_, reply)| {
        let start = reply.rfind("ui.type ")? + "ui.type ".len();
        let value: Value = serde_json::Deserializer::from_str(&reply[start..]).into_iter::<Value>().next()?.ok()?;
        value["text"].as_str().map(str::to_string).filter(|s| !s.trim().is_empty())
    })
}

/// "manda o mesmo pro user y", "agora no whatsapp": sem texto novo, repete o
/// da mensagem anterior.
pub fn message(text: &str, plan: &[String], message: String, recent: &[(String, String)]) -> String {
    let t = text.to_lowercase();
    let again = has(&t, &["o mesmo", "a mesma", "mesma coisa", "de novo", "também", "tambem"]) || t.starts_with("agora ");
    if message.trim().is_empty() && plan.iter().any(|p| p == "ui.type") && again {
        return last_typed(recent).unwrap_or(message);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn single_window_never_tiles() {
        assert_eq!(plan("põe o chrome na metade direita da tela", p(&["window.tile"])), p(&["window.move"]));
        assert_eq!(plan("divide a tela entre o discord e o chrome", p(&["window.tile"])), p(&["window.tile"]));
        assert_eq!(
            plan("coloca chrome, spotify e whatsapp todos visíveis", p(&["window.tile", "window.tile", "window.tile"])),
            p(&["window.tile"])
        );
    }

    #[test]
    fn chat_steps_need_a_chat_request() {
        assert_eq!(plan("manda o discord pra outra tela", p(&["ui.open_chat", "window.move"])), p(&["window.move"]));
        assert_eq!(plan("tô entediado, me anima", p(&["ui.open_chat", "ui.type"])), p(&[]));
        let chat = p(&["ui.open_chat", "ui.type", "ui.open_chat", "ui.type"]);
        assert_eq!(plan("manda oi pro user x no whatsapp e pro user y no discord", chat.clone()), chat);
    }

    #[test]
    fn browser_files_and_talk_fixes() {
        assert_eq!(plan("abre uma aba e pesquisa gatos", p(&["browser.open", "browser.search"])), p(&["browser.search"]));
        assert_eq!(plan("joga o teste.txt na lixeira", p(&["file.move"])), p(&["file.delete"]));
        assert_eq!(plan("abre o arquivo musica.mp3", p(&["file.run"])), p(&["file.open"]));
        assert_eq!(plan("manda o foto.png pra imagens", p(&["download.file"])), p(&["file.move"]));
        assert_eq!(plan("força o fechamento do discord", p(&["window.close"])), p(&["app.close"]));
        assert_eq!(plan("tell me a joke", p(&["memory.remember"])), p(&[]));
        assert_eq!(plan("o que significa a palavra efêmero?", p(&["web.search"])), p(&[]));
        assert_eq!(plan("fecha a janela do spotify", p(&["app.close", "app.close"])), p(&["app.close"]));
        assert_eq!(plan("e o spotify também", p(&["window.minimize", "window.minimize"])), p(&["window.minimize"]));
        assert_eq!(
            plan("minimiza o discord e o spotify", p(&["window.minimize", "window.minimize"])),
            p(&["window.minimize", "window.minimize"])
        );
    }

    #[test]
    fn repeats_the_previous_message() {
        let recent = vec![(
            "manda um bom dia pro user x no discord".to_string(),
            "Mandei!\n[ações nesse pedido: ui.open_chat {\"name\":\"user x\"} → ui.type {\"target\":\"discord\",\"text\":\"bom dia\"}]".to_string(),
        )];
        let plan = p(&["ui.open_chat", "ui.type"]);
        assert_eq!(message("manda o mesmo pro user y", &plan, String::new(), &recent), "bom dia");
        assert_eq!(message("agora no whatsapp", &plan, String::new(), &recent), "bom dia");
        assert_eq!(message("manda oi", &plan, "oi".into(), &recent), "oi");
        assert_eq!(message("manda o mesmo", &plan, String::new(), &[]), "");
    }
}
