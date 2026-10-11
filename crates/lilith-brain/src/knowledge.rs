//! Conhecimento que a Lilith acumula e injeta no intérprete sem gastar mais
//! modelo: exemplos de planos que deram certo (embutidos + aprendidos) e o
//! glossário pessoal de gírias. Tudo escolhido por pedido, para o prompt
//! continuar curto.

use std::collections::HashSet;

/// Um pedido e o plano de ferramentas que o resolve (`[]` = só conversa).
#[derive(Debug, Clone, PartialEq)]
pub struct Example {
    pub text: String,
    pub plan: Vec<String>,
}

/// O que o app guarda (SQLite) e o cérebro consulta.
pub trait Knowledge: Send + Sync {
    /// Planos que deram certo, os mais recentes primeiro.
    fn plans(&self) -> Vec<Example>;
    /// Gírias/apelidos do usuário: (termo, significado).
    fn glossary(&self) -> Vec<(String, String)>;
    /// Registra o plano de um pedido que terminou sem erro.
    fn record_plan(&self, text: &str, plan: &[String]);
}

/// Exemplos embutidos: pedidos típicos com o plano certo. Escritos uma vez,
/// à mão (com um modelo maior como apoio), e deliberadamente diferentes das
/// frases da bateria de testes, para a medição não ser viciada.
const SEEDS: &[(&str, &[&str])] = &[
    // janelas
    ("joga o telegram um tiquinho pro lado", &["window.move"]),
    ("deixa o vscode na metade da direita", &["window.move"]),
    ("encosta o notion no canto de baixo da esquerda", &["window.move"]),
    ("leva o obs pro outro monitor", &["window.move"]),
    ("sobe um pouquinho o paint", &["window.move"]),
    ("deixa o excel ocupando a tela toda", &["window.maximize"]),
    ("esconde o slack", &["window.minimize"]),
    ("traz o telegram de volta", &["window.restore"]),
    ("quero ver o terminal", &["window.focus"]),
    ("põe o vscode e o terminal lado a lado", &["window.tile"]),
    ("arruma as janelas da tela pra mim", &["window.tile"]),
    ("tem alguma coisa aberta no segundo monitor?", &["window.list"]),
    ("mostra onde o telegram tá", &["window.list", "cursor.point"]),
    ("encerra o telegram", &["app.close"]),
    // sistema
    ("deixa o volume em 35", &["audio.set_volume"]),
    ("som em 80 por favor", &["audio.set_volume"]),
    ("tá muito alto, diminui", &["audio.set_volume"]),
    ("cala o computador", &["audio.set_mute"]),
    ("liga o som de novo", &["audio.set_mute"]),
    ("o volume tá em quanto?", &["audio.get_volume"]),
    ("inicia o telegram", &["app.open"]),
    ("quero abrir o vscode", &["app.open"]),
    ("sobe o obs aí", &["app.open"]),
    ("trava o pc que eu vou sair", &["system.power"]),
    ("hiberna o pc", &["system.power"]),
    ("troca o fundo de tela por uma paisagem de montanha", &["desktop.set_wallpaper"]),
    ("coloca um fundo de tela roxo escuro", &["desktop.set_wallpaper"]),
    ("guarda 'abc123' no clipboard", &["clipboard.set"]),
    ("qual texto eu copiei por último?", &["clipboard.get"]),
    // arquivos
    ("manda o contrato.pdf pra lixeira", &["file.delete"]),
    ("exclui a pasta antiga de fotos", &["file.delete"]),
    ("joga o backup.zip fora", &["file.delete"]),
    ("move o orcamento.xlsx pra pasta Trabalho", &["file.move"]),
    ("renomeia o rascunho.txt pra final.txt", &["file.move"]),
    ("duplica o curriculo.docx na área de trabalho", &["file.copy"]),
    ("o que tem escrito no lista.txt?", &["file.read"]),
    ("abre a pasta de imagens", &["file.open"]),
    ("lista o que tem em Documentos", &["file.list"]),
    ("onde está o arquivo planilha.xlsx?", &["file.find"]),
    ("roda o setup.exe que tá em downloads", &["file.run"]),
    // navegador e web
    ("abre o instagram", &["browser.open"]),
    ("vai pro site do reddit", &["browser.open"]),
    ("entra no netflix", &["browser.open"]),
    ("abre o stackoverflow numa aba anônima", &["browser.open"]),
    ("abre uma aba e procura receita de pão caseiro", &["browser.search"]),
    ("procura fotos de cachorro filhote", &["browser.search"]),
    ("acha videos de como trocar pneu", &["browser.search"]),
    ("fecha a aba atual", &["browser.tab"]),
    ("vai pra aba da direita", &["browser.tab"]),
    ("reabre a última aba", &["browser.tab"]),
    ("pesquisa na internet a previsão do tempo e me resume", &["web.search"]),
    ("lê esse link pra mim: https://example.com/artigo", &["web.fetch"]),
    ("baixa esse pdf https://example.com/guia.pdf", &["download.file"]),
    ("baixa o instalador do 7zip", &["web.search", "download.file"]),
    // chats
    ("manda um oi pro user x no telegram", &["ui.open_chat", "ui.type"]),
    ("diz pro user y no discord que eu já chego", &["ui.open_chat", "ui.type"]),
    ("chama o user z pra jogar cmg no whatsapp", &["ui.open_chat", "ui.type"]),
    ("pergunta pro user x no teams se a reunião continua de pé", &["ui.open_chat", "ui.type"]),
    ("manda bom dia pro grupo b do whatsapp", &["ui.open_chat", "ui.type"]),
    ("lê o que o user y escreveu no discord", &["ui.open_chat", "window.read"]),
    ("o que tem de novo no grupo a do whatsapp?", &["ui.open_chat", "window.read"]),
    ("manda oi pro user x no zap e pro user y no teams", &["ui.open_chat", "ui.type", "ui.open_chat", "ui.type"]),
    ("lê o que tá escrito na janela do word", &["window.read"]),
    // dentro de programas
    ("clica em configurações no discord", &["ui.click"]),
    ("vai na aba biblioteca do steam", &["ui.click"]),
    ("aperta o botão de entrar no teams", &["ui.click"]),
    ("entra no servidor do grupo c no discord", &["ui.click"]),
    // memória e agentes
    ("o que a gente falou sobre as férias?", &["memory.search"]),
    ("lembra daquela conversa do orçamento?", &["memory.search"]),
    ("anota que meu aniversário é em março", &["memory.remember"]),
    ("me lembra dia 20 de abrir o discord e falar com a equipe", &["reminder.add"]),
    ("lembra que amanhã tenho dentista, me avisa quando eu abrir o chat", &["reminder.add"]),
    ("esquece o que eu disse do meu endereço", &["memory.forget"]),
    ("pede pro codex consertar os testes do projeto", &["agent.delegate"]),
    ("já acabou aquela tarefa do claude?", &["agent.jobs"]),
    // conversa (não usa ferramenta)
    ("e aí, tudo certo?", &[]),
    ("me explica o que é uma API", &[]),
    ("qual a raiz quadrada de 81?", &[]),
    ("como se diz obrigado em japonês?", &[]),
    ("o que quer dizer 'resilient' em português?", &[]),
    ("quem escreveu Dom Casmurro?", &[]),
    ("me sugere um nome pro meu gato", &[]),
    ("brigadão, ajudou muito", &[]),
];

fn words(s: &str) -> HashSet<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() > 1)
        .map(str::to_string)
        .collect()
}

/// Semelhança de Dice entre os conjuntos de palavras (0 a 1).
fn similarity(a: &HashSet<String>, b: &HashSet<String>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    2.0 * a.intersection(b).count() as f32 / (a.len() + b.len()) as f32
}

/// Até `n` exemplos parecidos com o pedido: aprendidos primeiro (desempate),
/// depois os embutidos. Descarta o que for quase idêntico ao próprio pedido
/// e o que tiver pouco em comum.
pub fn pick(text: &str, learned: &[Example], n: usize) -> Vec<Example> {
    const MIN_SIMILARITY: f32 = 0.3;
    let target = words(text);
    let mut scored: Vec<(f32, usize, Example)> = Vec::new();
    let seeds = SEEDS.iter().map(|(t, p)| Example { text: (*t).into(), plan: p.iter().map(|s| (*s).into()).collect() });
    for (rank, ex) in learned.iter().cloned().chain(seeds).enumerate() {
        let s = similarity(&target, &words(&ex.text));
        if s >= MIN_SIMILARITY {
            scored.push((s, rank, ex));
        }
    }
    // Mais parecido primeiro; no empate, o que veio antes (aprendido > embutido).
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    let mut seen = HashSet::new();
    scored
        .into_iter()
        .map(|(_, _, ex)| ex)
        .filter(|ex| seen.insert(ex.text.to_lowercase()))
        .take(n)
        .collect()
}

/// Bloco de prompt com os exemplos (vazio se não houver nenhum).
pub fn render(examples: &[Example]) -> String {
    if examples.is_empty() {
        return String::new();
    }
    let lines: String = examples
        .iter()
        .map(|e| {
            let plan = if e.plan.is_empty() { "[] (só conversa)".to_string() } else { format!("[{}]", e.plan.join(", ")) };
            format!("- \"{}\" → {plan}\n", e.text)
        })
        .collect();
    format!("\nExemplos de pedidos parecidos com o plano que deu certo (só as ferramentas: o texto de `message` e os nomes vêm SEMPRE do pedido atual, nunca dos exemplos):\n{lines}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_similar_examples_and_prefers_learned() {
        let learned = vec![Example { text: "deixa o volume em 50".into(), plan: vec!["audio.set_volume".into()] }];
        let got = pick("deixa o volume em 20", &learned, 3);
        assert_eq!(got[0].text, "deixa o volume em 50", "o aprendido empata ou ganha");
        assert!(pick("xyzzy plugh", &[], 3).is_empty(), "nada parecido, nada injetado");
        assert!(got.len() <= 3);
    }

    #[test]
    fn render_marks_chat_only_plans() {
        let out = render(&[Example { text: "oi".into(), plan: vec![] }]);
        assert!(out.contains("[] (só conversa)"));
        assert_eq!(render(&[]), "");
    }

    #[test]
    fn seeds_only_use_real_tools() {
        let tools = crate::tools::all_tools();
        for (text, plan) in SEEDS {
            for step in *plan {
                assert!(tools.iter().any(|t| t.action == *step), "{text}: ferramenta inexistente {step}");
            }
        }
    }
}
