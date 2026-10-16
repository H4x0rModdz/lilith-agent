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
    "responde", "cobra", "mandou", "escreveu", "falou", "disse", "respondeu", "enviou", "mensagens",
];

/// Palavras de posição de janela: o pedido é sobre a tela, não sobre um chat.
const LAYOUT_WORDS: &[&str] = &[
    "tela", "monitor", "canto", "metade", "pro lado", "para o lado", "lado a lado", "visíveis", "visiveis", "screen",
    "direita", "esquerda", "left", "right",
];

const AUDIO_WORDS: &[&str] = &["volume", "som", "áudio", "audio", "mudo", "muta", "mute", "sound", "barulho", "silencia", "baixo", "alto", "aumenta", "diminui"];

const TILE_WORDS: &[&str] = &[
    "divide", "dividir", "lado a lado", "do lado do", "lado do outro", "juntas", "organiza", "visíveis", "visiveis", "grade",
    "todas as janelas", "side by side", "tile",
];

/// O pedido começa com um verbo de guardar na memória?
fn is_remember_request(t: &str) -> bool {
    let t = t.trim_start_matches(['/', ' ']);
    ["lembra que", "lembra de que", "lembre que", "lembre-se que", "anota que", "anota aí que", "guarda que", "salva que"]
        .iter()
        .any(|p| t.starts_with(p))
}

/// Pedido de lembrete com data: "me lembra dia 12/10…", "lembra que amanhã…".
fn is_reminder_request(t: &str) -> bool {
    let t = t.trim_start_matches(['/', ' ']);
    let asks_me = ["me lembra", "me lembre", "me avisa", "me avise"].iter().any(|p| t.starts_with(p));
    let has_date = t.as_bytes().windows(3).any(|w| w[0].is_ascii_digit() && w[1] == b'/' && w[2].is_ascii_digit())
        || has(t, &["amanhã", "amanha", "depois de amanhã", "semana que vem", "próxima semana", "dia "]);
    let has_time = has(t, &[" às ", " as ", "daqui a", "daqui há"])
        || t.as_bytes().windows(2).any(|w| w[0].is_ascii_digit() && w[1] == b'h');
    let has_date = has_date || has_time;
    let when_opening = has(t, &["quando eu abrir", "quando abrir", "ao abrir", "quando eu entrar"]);
    asks_me && (has_date || when_opening) || (is_remember_request(t) && has_date && has(t, &["me lembr", "me avis", "lembrete"]))
}

/// Pedido para ler o que aparece na tela/monitor (não uma janela específica).
fn is_screen_request(t: &str) -> bool {
    let screen = has(t, &["na tela", "na minha tela", "no monitor", "do monitor", "o monitor", "minha tela", "a tela"]);
    let read = has(t, &["o que tem", "o que tá", "o que está", "o que aparece", "lê ", "le ", "ler ", "leia", "o que diz", "o que está escrito", "o que tá escrito"]);
    screen && read && !has(t, &["janela", "bloco de notas", "chat", "conversa", "arquivo"])
}

/// Aplica as regras ao plano do intérprete para o pedido `text`.
pub fn plan(text: &str, plan: Vec<String>, recent: &[(String, String)]) -> Vec<String> {
    let t = text.to_lowercase();
    let mut plan = plan;
    let is = |plan: &[String], a: &str| plan.iter().any(|p| p == a);

    // Criar/ativar modo, salvar/restaurar layout, lembrete repetido: uma
    // ferramenta só (criar um modo NUNCA executa os passos dele agora).
    if let Some(fixed) = crate::routines::refine(&t) {
        return fixed;
    }

    // "Lembra que…" é guardar um fato, mesmo que cite o Discord, uma pessoa ou
    // "me lembre quando eu abrir o chat": nunca é mandar mensagem.
    // "Repara a vulnerabilidade": achar a pasta da solução e delegar a correção a um
    // agente de código (que roda o dotnet de verdade e valida com build/testes).
    let repair_verb = has(&t, &["repara", "reparar", "corrige", "corrigir", "conserta", "consertar", "resolve", "resolver", "arruma", "arrumar"]);
    let about_vulnerability = has(&t, &["vulnerab"])
        || (has(&t, &[" isso", " ela", " elas", " esse", " essa", "também"])
            && recent.iter().any(|(u, r)| format!("{u} {r}").to_lowercase().contains("vulnerab")));
    if repair_verb && about_vulnerability {
        let steps = ["file.find".to_string(), "agent.delegate".to_string()];
        if has(&t, &["mostra", "mostrar", "quais"]) {
            plan.extend(steps);
            return plan;
        }
        return steps.to_vec();
    }

    // Erros de build ("explica o erro do build", "arruma o que quebrou no
    // build") e git só leitura: o ajudante de dev decide (ver `dev::plan`).
    if let Some(steps) = crate::dev::plan(&t, recent) {
        return steps;
    }

    // "O que tem na minha tela?", "lê o monitor 2": OCR da tela (código decide).
    if is_screen_request(&t) {
        return vec!["screen.read".to_string()];
    }

    if is_reminder_request(&t) {
        return vec!["reminder.add".to_string()];
    }
    if is_remember_request(&t) {
        return vec!["memory.remember".to_string()];
    }

    // Celular (SMS, notificações, chamadas) e páginas das Configurações.
    if let Some(fixed) = crate::native::refine(&t, &plan) {
        return fixed;
    }

    // Continuação de um pedido de chat ("e do user z?") herda o caminho.
    let chat_followup = (t.starts_with("e ") || t.starts_with("agora ") || has(&t, &["o mesmo", "também", "tambem"]))
        && recent.iter().any(|(_, r)| r.contains("ui.open_chat") || r.contains("ui.type"));

    // Etapas de chat só quando o pedido fala com alguém (e não é de layout).
    if !chat_followup
        && (!has(&t, CHAT_WORDS) || has(&t, LAYOUT_WORDS))
        && (is(&plan, "ui.open_chat") || is(&plan, "ui.type")) {
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

    // "Me mostra quais são" depois de um clique: o que o clique abriu precisa ser
    // LIDO depois dele (uma leitura antes do clique não conta). Sem isso, o agente
    // dava o plano por cumprido logo após o clique e respondia "agora vou ler…".
    if has(&t, &["mostra", "mostrar", "quais", "me diz", "me fala", "lista", "listar", " ler ", " lê ", "o que diz", "o que tem"]) {
        let last_click = plan.iter().rposition(|p| p == "ui.click");
        let last_read = plan.iter().rposition(|p| p == "window.read");
        if let Some(c) = last_click {
            if last_read.is_none_or(|r| r < c) {
                plan.push("window.read".to_string());
            }
        }
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
    if !has(&t, &["lembra", "falei", "falou", "falamos", "conversou", "conversaram", "falado", "combinamos", "combinado", "mencionei", "comentei", "conversa", "conversamos", "disse", "ontem", "antes", "remember", "told"]) {
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

    fn plan(text: &str, steps: Vec<String>) -> Vec<String> {
        super::plan(text, steps, &[])
    }

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
    fn keeps_chat_steps_for_chat_followups_and_reads() {
        let recent = vec![(
            "lê o chat do user x no discord".to_string(),
            "ok\n[ações nesse pedido: ui.open_chat {} → window.read {}]".to_string(),
        )];
        let steps = p(&["ui.open_chat", "window.read"]);
        assert_eq!(super::plan("e do user z?", steps.clone(), &recent), steps);
        assert_eq!(plan("o que o user x mandou no whatsapp?", steps.clone()), steps);
        assert_eq!(plan("põe o chrome e o vscode um do lado do outro", p(&["window.tile"])), p(&["window.tile"]));
        assert_eq!(plan("o que a gente conversou sobre o wifi?", p(&["memory.search"])), p(&["memory.search"]));
    }

    #[test]
    fn remember_requests_never_send_messages() {
        let wrong = p(&["ui.open_chat", "ui.type"]);
        assert_eq!(
            plan("lembra que a senha do wifi é example-pass e que o discord é do user x", wrong.clone()),
            p(&["memory.remember"])
        );
        assert_eq!(plan("lembra que meu time é o time a", wrong), p(&["memory.remember"]));
        assert_eq!(
            plan("lembra que dia 12/10, quando eu abrir o chat, me lembre que preciso abrir o discord", p(&["ui.type"])),
            p(&["reminder.add"])
        );
        assert_eq!(plan("me lembra amanhã de ligar pro dentista", p(&[])), p(&["reminder.add"]));
        // "lembra quando…" continua sendo busca na memória.
        assert_eq!(plan("lembra quando eu falei do monitor?", p(&["memory.search"])), p(&["memory.search"]));
    }

    #[test]
    fn show_me_after_a_click_needs_a_read_after_the_click() {
        let ask = "o visual studio diz que a solução tem pacotes vulneráveis. Consegue me mostrar quais são? vai precisar clicar em gerenciar pacotes";
        assert_eq!(
            plan(ask, p(&["window.read", "ui.click"])),
            p(&["window.read", "ui.click", "window.read"]),
            "a leitura antes do clique não conta"
        );
        // Já tem leitura depois do clique: não duplica.
        assert_eq!(plan(ask, p(&["ui.click", "window.read"])), p(&["ui.click", "window.read"]));
        // Clicar sem pedir para ver nada: não acrescenta leitura.
        assert_eq!(plan("clica em histórico no discord", p(&["ui.click"])), p(&["ui.click"]));
    }

    #[test]
    fn repairing_a_vulnerability_finds_the_folder_then_delegates() {
        let want = p(&["file.find", "agent.delegate"]);
        assert_eq!(plan("repara a vulnerabilidade também", p(&["ui.click"])), want);
        assert_eq!(plan("corrige a vulnerabilidade do pacote", p(&[])), want);
        // "isso" só vale com contexto de vulnerabilidade na conversa recente.
        let recent = vec![("mostra as vulnerabilidades".to_string(), "O pacote X está vulnerável".to_string())];
        assert_eq!(super::plan("conserta isso", p(&[]), &recent), want);
        assert_eq!(plan("conserta isso", p(&[])), p(&[]), "sem contexto não vira reparo");
        assert_eq!(
            plan("me mostra quais são e corrige a vulnerabilidade", p(&["window.read"])),
            p(&["window.read", "file.find", "agent.delegate"])
        );
    }

    #[test]
    fn screen_requests_go_to_ocr() {
        assert_eq!(plan("o que tem na minha tela?", p(&["window.read"])), p(&["screen.read"]));
        assert_eq!(plan("lê o monitor 2 pra mim", p(&[])), p(&["screen.read"]));
        assert_eq!(plan("lê o que tá escrito na janela do bloco de notas", p(&["window.read"])), p(&["window.read"]));
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
