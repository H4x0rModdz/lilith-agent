use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Uma tool = uma ação executável. `schema` é o JSON Schema enviado ao LLM.
#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub action: &'static str,
    pub description: &'static str,
    pub schema: Value,
}

/// Chamada de tool já parseada, pronta para o dispatcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub action: String,
    pub params: Value,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

/// Catálogo de ações do Windows. Toda ação aqui precisa de scope em
/// `lilith-guard::scope_for` e de handler em `lilith-win::dispatch` — o teste
/// `catalog_is_wired` no app garante isso.
pub fn all_tools() -> Vec<Tool> {
    vec![
        // ── apps ────────────────────────────────────────────────────────
        Tool {
            action: "app.open",
            description: "Abre um aplicativo pelo nome (ex.: 'spotify', 'discord', 'bloco de notas'). \
                          Se ele JÁ estiver aberto, só traz a janela para frente — não abre outra. \
                          Use `new_window: true` só se o usuário pedir outra janela.",
            schema: obj(
                json!({ "app": { "type": "string" }, "new_window": { "type": "boolean" } }),
                &["app"],
            ),
        },
        Tool {
            action: "app.close",
            description: "ENCERRA um aplicativo (ex.: 'chrome', 'spotify'). Use SÓ quando o usuário pedir \
                          explicitamente para fechar/encerrar/matar. Para mover, arrastar, organizar ou \
                          trocar de tela, use window.move.",
            schema: obj(
                json!({
                    "app": { "type": "string" },
                    "force": { "type": "boolean", "description": "Forçar encerramento" }
                }),
                &["app"],
            ),
        },
        Tool {
            action: "browser.open",
            description: "Abre uma URL numa NOVA ABA do navegador (o PADRÃO do usuário, ou o `browser` \
                          pedido, ex.: 'waterfox' — usa a janela que já está aberta). `mode`: tab \
                          (padrão), window (nova janela) ou private (janela privada/anônima).",
            schema: obj(
                json!({
                    "url": { "type": "string" },
                    "browser": { "type": "string" },
                    "mode": { "type": "string", "enum": ["tab", "window", "private"] }
                }),
                &["url"],
            ),
        },
        Tool {
            action: "browser.tab",
            description: "Mexe nas ABAS do navegador por teclado (igual em Firefox e Chromium): `op` = new \
                          (nova aba vazia), close, next, previous, reopen (reabrir a última fechada), \
                          address (foca a barra de endereço), find (localizar na página), reload. Sem \
                          `browser`, usa o navegador padrão do usuário. Para abrir um site ou pesquisar, \
                          use browser.open / browser.search.",
            schema: obj(
                json!({
                    "op": { "type": "string", "enum": ["new", "close", "next", "previous", "reopen", "address", "find", "reload"] },
                    "browser": { "type": "string" }
                }),
                &["op"],
            ),
        },
        Tool {
            action: "browser.search",
            description: "Pesquisa no Google numa NOVA ABA do navegador já aberto — ela mesma abre a \
                          aba, então NÃO chame browser.open antes. Use para 'abre uma aba e pesquisa X', \
                          'procura imagens de X'. `kind`: web (padrão), images, videos ou news — já abre \
                          na aba de resultados certa (não precisa clicar em 'Imagens'). `browser`: o \
                          navegador que o usuário citou (ex.: 'waterfox'); sem citar, usa o navegador \
                          PADRÃO do usuário.",
            schema: obj(
                json!({
                    "query": { "type": "string" },
                    "kind": { "type": "string", "enum": ["web", "images", "videos", "news"] },
                    "browser": { "type": "string" },
                    "mode": { "type": "string", "enum": ["tab", "window", "private"], "description": "tab (padrão), nova janela ou janela privada" }
                }),
                &["query"],
            ),
        },
        // ── arquivos ────────────────────────────────────────────────────
        Tool {
            action: "file.open",
            description: "Abre uma pasta ou documento (PDF, imagem, vídeo, texto, planilha…)                           com o programa padrão do Windows.",
            schema: obj(json!({ "path": { "type": "string" } }), &["path"]),
        },
        Tool {
            action: "file.run",
            description: "Executa um arquivo: programa (.exe), script, instalador ou atalho.                           Pede aprovação do usuário.",
            schema: obj(json!({ "path": { "type": "string" } }), &["path"]),
        },
        Tool {
            action: "file.find",
            description: "Procura arquivos/pastas pelo nome (busca parcial, sem diferenciar maiúsculas). \
                          `root` opcional; padrão: pasta do usuário.",
            schema: obj(
                json!({
                    "name": { "type": "string" },
                    "root": { "type": "string" },
                    "limit": { "type": "integer" }
                }),
                &["name"],
            ),
        },
        Tool {
            action: "file.read",
            description: "Lê o conteúdo de um ARQUIVO de texto no disco (txt, md, json, csv, código…). \
                          Use quando o usuário citar um arquivo pelo nome ou caminho. Para o que está \
                          aberto na tela num programa, use window.read.",
            schema: obj(json!({ "path": { "type": "string" } }), &["path"]),
        },
        Tool {
            action: "file.list",
            description: "Lista o conteúdo de uma pasta.",
            schema: obj(json!({ "path": { "type": "string" } }), &["path"]),
        },
        Tool {
            action: "file.move",
            description: "Move ou renomeia um arquivo/pasta.",
            schema: obj(
                json!({ "source": { "type": "string" }, "destination": { "type": "string" } }),
                &["source", "destination"],
            ),
        },
        Tool {
            action: "file.copy",
            description: "Copia um arquivo.",
            schema: obj(
                json!({ "source": { "type": "string" }, "destination": { "type": "string" } }),
                &["source", "destination"],
            ),
        },
        Tool {
            action: "file.delete",
            description: "Manda um arquivo/pasta para a Lixeira.",
            schema: obj(json!({ "path": { "type": "string" } }), &["path"]),
        },
        Tool {
            action: "download.file",
            description: "Baixa um arquivo de uma URL para a pasta Downloads (ou `destination`).",
            schema: obj(
                json!({
                    "url": { "type": "string" },
                    "destination": { "type": "string", "description": "Pasta ou caminho final (opcional)" }
                }),
                &["url"],
            ),
        },
        // ── desktop / sistema ───────────────────────────────────────────
        Tool {
            action: "desktop.set_wallpaper",
            description: "Troca o papel de parede. `source` pode ser caminho de imagem, URL, \
                          ou um termo de busca (ex.: 'gato preto', 'cyberpunk rosa').",
            schema: obj(json!({ "source": { "type": "string" } }), &["source"]),
        },
        Tool {
            action: "audio.set_volume",
            description: "Define o volume geral (0 a 100).",
            schema: obj(json!({ "level": { "type": "integer", "minimum": 0, "maximum": 100 } }), &["level"]),
        },
        Tool {
            action: "audio.get_volume",
            description: "Lê o volume geral atual e se está mudo.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "audio.set_mute",
            description: "Liga/desliga o mudo.",
            schema: obj(json!({ "muted": { "type": "boolean" } }), &["muted"]),
        },
        Tool {
            action: "audio.app_volume",
            description: "Volume ou mudo de UM aplicativo (ex.: 'abaixa o spotify pra 20', 'muta o discord'). \
                          `app`: nome do programa; `level` 0–100 e/ou `muted`.",
            schema: obj(
                json!({ "app": { "type": "string" }, "level": { "type": "integer", "minimum": 0, "maximum": 100 }, "muted": { "type": "boolean" } }),
                &["app"],
            ),
        },
        Tool {
            action: "discord.voice",
            description: "Controles de voz do DISCORD: `mic` (mute/unmute/toggle) silencia ou ativa o \
                          microfone; `audio` (deafen/undeafen/toggle) desativa ou ativa o áudio (ensurdecer); \
                          `input_device`/`output_device` escolhe o dispositivo pelo nome (ou 'list' para \
                          listar). Sem parâmetros, diz se o microfone/áudio estão desligados.",
            schema: obj(
                json!({
                    "mic": { "type": "string", "enum": ["mute", "unmute", "toggle"] },
                    "audio": { "type": "string", "enum": ["deafen", "undeafen", "toggle"] },
                    "input_device": { "type": "string" },
                    "output_device": { "type": "string" }
                }),
                &[],
            ),
        },
        Tool {
            action: "discord.status",
            description: "Muda o status do DISCORD: online (disponível), idle (ausente), dnd (não perturbe) \
                          ou invisible (invisível).",
            schema: obj(
                json!({ "status": { "type": "string", "enum": ["online", "idle", "dnd", "invisible"] } }),
                &["status"],
            ),
        },
        Tool {
            action: "discord.account",
            description: "Troca a conta do DISCORD por outra já logada (`account`: parte do nome; 'list' \
                          lista as contas).",
            schema: obj(json!({ "account": { "type": "string" } }), &["account"]),
        },
        Tool {
            action: "discord.pending",
            description: "Lista os pedidos de amizade pendentes do DISCORD (recebidos e enviados).",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "discord.friend",
            description: "Amizades no DISCORD: `op` accept (aceitar pedido), decline (recusar) ou add \
                          (enviar pedido de amizade); `name`: nome de usuário da pessoa.",
            schema: obj(
                json!({ "op": { "type": "string", "enum": ["accept", "decline", "add"] }, "name": { "type": "string" } }),
                &["op", "name"],
            ),
        },
        Tool {
            action: "media.control",
            description: "Controla a música/vídeo tocando no PC (Spotify, navegador…). `action`: play_pause, \
                          play, pause, next, previous ou stop.",
            schema: obj(
                json!({ "action": { "type": "string", "enum": ["play_pause", "play", "pause", "next", "previous", "stop"] } }),
                &["action"],
            ),
        },
        Tool {
            action: "media.now_playing",
            description: "Diz o que está tocando agora (música, artista, app, se está pausado).",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "system.status",
            description: "Diagnóstico do PC agora: CPU, memória, discos, placa de vídeo, bateria, internet e os \
                          programas que mais pesam, com os pontos de atenção já calculados em `findings`. Use \
                          para 'por que o PC tá lento', 'como está o computador', 'o que tá pesando'. Responda \
                          com base em `findings` e `summary`.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "screen.capture",
            description: "Tira um print e salva em Imagens\\Capturas de Tela (e copia para a área de \
                          transferência): sem parâmetros = todos os monitores; `monitor` = só aquele monitor; \
                          `target` = só aquela janela/app ('tira um print do discord').",
            schema: obj(
                json!({ "monitor": { "type": "integer", "minimum": 1 }, "target": { "type": "string" } }),
                &[],
            ),
        },
        Tool {
            action: "system.checkup",
            description: "Checkup COMPLETO do PC (~15 s): Windows (atualizações, reinício pendente), processador, \
                          memória (velocidade, pentes), drivers de vídeo, saúde dos discos, programas na \
                          inicialização, energia/bateria, antivírus, estabilidade (desligamentos, telas azuis, \
                          erros de hardware/disco) e internet. Já vem com `findings` decididos por regras. Use \
                          para 'diagnóstico completo', 'checkup do pc', 'por que o pc tá lento/travando'.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "self.diagnose",
            description: "Autodiagnóstico da própria Lilith: checa o cérebro (Ollama e modelo), a voz, a forma \
                          3D, a atualização, a memória e as ações que falharam nas últimas 24 h. Use para \
                          'você tá funcionando direito?', 'faz um checkup de você', 'por que você não respondeu'.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "screen.read",
            description: "Lê o texto que aparece NA TELA por imagem (OCR): de um monitor (`monitor`: 1, 2…) ou \
                          de uma janela (`target`). Use para 'o que tem na minha tela', 'lê o monitor 2', ou \
                          quando window.read não achar texto (jogo, imagem, PDF escaneado). `query` filtra \
                          linhas por palavra-chave.",
            schema: obj(
                json!({ "monitor": { "type": "integer", "minimum": 1 }, "target": { "type": "string" }, "query": { "type": "string" } }),
                &[],
            ),
        },
        Tool {
            action: "clipboard.set",
            description: "Copia um texto para a área de transferência.",
            schema: obj(json!({ "text": { "type": "string" } }), &["text"]),
        },
        Tool {
            action: "clipboard.get",
            description: "Lê o texto da área de transferência.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "system.power",
            description: "Bloquear, suspender, reiniciar ou desligar o PC.",
            schema: obj(
                json!({ "op": { "type": "string", "enum": ["lock", "sleep", "restart", "shutdown"] } }),
                &["op"],
            ),
        },
        // ── janelas (com o cursor rosa da Lilith) ───────────────────────
        Tool {
            action: "window.list",
            description: "Lista as janelas abertas (título, app, monitor, id) e os monitores. Use quando \
                          precisar saber o que está aberto ou em qual tela.",
            schema: obj(json!({}), &[]),
        },
        Tool {
            action: "window.read",
            description: "Lê o texto que está aberto num PROGRAMA/JANELA agora (Bloco de Notas, Word, \
                          navegador, chat…), mesmo sem estar salvo. Use para 'lê o que está escrito no \
                          bloco de notas', 'o que diz essa janela'. `target`: o PROGRAMA (ex.: 'discord', 'whatsapp'), nunca o nome de uma conversa. Em chats \
                          (Discord…) traz só as mensagens dos últimos 5 minutos. Passe `since_minutes` \
                          SÓ se o usuário disser um período ('da última hora' = 60, 'de hoje' = 1440); \
                          sem período dito, omita. Traz o documento principal E o resto da janela \
                          (avisos, barra de status, listas como a Lista de Erros, botões). Para achar \
                          algo específico ('o aviso de vulnerabilidade', 'o erro de compilação') passe \
                          `query` com uma palavra-chave (ex.: 'vulnerab'): volta `matches` com o que \
                          casa. Só vê o que está visível na tela; se `matches` vier vazio, o item pode \
                          estar rolado ou em outra janela — diga isso, não invente.",
            schema: obj(
                json!({
                    "target": { "type": "string" },
                    "since_minutes": { "type": "integer" },
                    "query": { "type": "string", "description": "Palavra-chave para procurar na janela" }
                }),
                &["target"],
            ),
        },
        Tool {
            action: "ui.click",
            description: "Clica num item DENTRO de um programa pelo nome visível: uma conversa ou \
                          servidor do Discord, um contato do WhatsApp, uma aba, um botão, um menu. Use \
                          para navegar: 'lê o chat da <pessoa> no discord' = ui.click(target='discord', \
                          element='<pessoa>') e depois window.read; 'abre o servidor <nome>' = \
                          ui.click(target='discord', element='<nome>'). Se o programa não estiver \
                          aberto, abra antes com app.open. `index` escolhe entre itens de mesmo nome. A resposta traz \n                          `changed`: se for false, o clique não mudou nada na janela (pode ser lento) — \n                          leia a janela (window.read) antes de dizer que abriu, e espere/tente de novo.",
            schema: obj(
                json!({
                    "target": { "type": "string", "description": "Programa (ex.: 'discord')" },
                    "element": { "type": "string", "description": "Nome visível do item, como o usuário disse" },
                    "index": { "type": "integer" }
                }),
                &["target", "element"],
            ),
        },
        Tool {
            action: "ui.open_chat",
            description: "Abre uma conversa (pessoa OU grupo) pelo NOME no WhatsApp, no Discord ou no Teams, usando \
                          a busca do próprio app — acha qualquer conversa, mesmo fora da lista visível. \
                          É o jeito certo de abrir um chat antes de ler ou mandar mensagem. Se o app \
                          não estiver aberto, abra antes com app.open. Se já estiver nessa conversa, ele \
                          avisa (`already_open`) e não refaz nada — siga para o próximo passo. Para \
                          outros programas use ui.find + ui.click.",
            schema: obj(
                json!({
                    "target": { "type": "string", "description": "'whatsapp', 'discord' ou 'teams'" },
                    "name": { "type": "string", "description": "SÓ o nome da pessoa ou do grupo, sem as palavras 'grupo', 'chat' ou 'conversa' (ex.: 'família', não 'grupo da família')" }
                }),
                &["target", "name"],
            ),
        },
        Tool {
            action: "ui.type",
            description: "Digita um texto num campo de um programa e, com `submit`, envia (Enter). Use \
                          para 'manda um <texto> pra <pessoa> no whatsapp/discord' DEPOIS de abrir a \
                          conversa com ui.open_chat. Ela mesma acha o campo de mensagem da janela, então \
                          NÃO passe `element`. Pede aprovação do usuário, mostrando o que será enviado. Digita como uma pessoa (devagar, com pausas); \
                          '@nome' e '@todos' viram menções de verdade, escolhidas na lista do app.",
            schema: obj(
                json!({
                    "target": { "type": "string", "description": "Programa (ex.: 'whatsapp')" },
                    "element": { "type": "string", "description": "Opcional: nome do campo. Omita — ela acha o campo de mensagem sozinha" },
                    "text": { "type": "string", "description": "O texto a enviar. Se a interpretação do pedido trouxe uma «Mensagem a enviar», use EXATAMENTE ela. Senão, o que o usuário ditou, palavra por palavra — sem emojis, reticências ou palavras suas. Se ele só descreveu o que quer dizer, escreva uma mensagem curta, em primeira pessoa e na voz dele, falando COM a pessoa; gírias como 'cmg' (comigo) nunca são nomes próprios" },
                    "submit": { "type": "boolean", "description": "Enviar com Enter (padrão: sim). false só se o usuário pedir para NÃO enviar" },
                    "mentions": { "type": "boolean", "description": "Transformar @nome em menção de verdade (padrão: sim)" }
                }),
                &["target", "text"],
            ),
        },
        Tool {
            action: "ui.find",
            description: "Procura itens num programa pelo nome e diz onde cada um está (ex.: 'Mensagens \
                          diretas' x um servidor/grupo). Use quando não souber se o nome é uma pessoa \
                          ou um grupo, ou houver vários parecidos; depois use ui.click com o `index`.",
            schema: obj(
                json!({ "target": { "type": "string" }, "element": { "type": "string" } }),
                &["target", "element"],
            ),
        },
        Tool {
            action: "window.move",
            description: "Arrasta uma janela com o seu cursor. Três jeitos: \
                          `nudge` (left/right/up/down) DESLOCA um pouco sem mudar o tamanho — use para \
                          'mais pro lado', 'um pouquinho pra esquerda', 'ajusta pra cima'; `amount`: \
                          small (padrão), medium, large ou pixels. \
                          `position` ENCAIXA numa região (muda o tamanho) — só quando o usuário nomear a \
                          região: left/right (metade), top/bottom, top_left, top_right, bottom_left, \
                          bottom_right, left_third, center_third, right_third, left_two_thirds, \
                          right_two_thirds, center, maximize. \
                          `monitor` leva para outra tela: número (1 = principal), 'next' ou 'previous'. \
                          `target`: nome do app ou trecho do título ('active' = a janela em uso).",
            schema: obj(
                json!({
                    "target": { "type": "string" },
                    "nudge": { "type": "string", "enum": ["left", "right", "up", "down"] },
                    "amount": { "type": ["string", "integer"] },
                    "position": { "type": "string" },
                    "monitor": { "type": ["integer", "string"] }
                }),
                &["target"],
            ),
        },
        Tool {
            action: "window.tile",
            description: "Divide a tela entre várias janelas, como o Snap do Windows: 2 lado a lado, 3 com \
                          uma principal, 4 em grade. `targets`: nomes dos apps na ordem (o primeiro fica \
                          na posição principal); sem `targets`, usa as `count` janelas mais recentes. \
                          `layout`: auto (padrão), columns, rows, grid, main_left, main_right.",
            schema: obj(
                json!({
                    "targets": { "type": "array", "items": { "type": "string" } },
                    "count": { "type": "integer" },
                    "layout": { "type": "string" },
                    "monitor": { "type": ["integer", "string"] }
                }),
                &[],
            ),
        },
        Tool {
            action: "window.focus",
            description: "Traz uma janela para frente e dá foco (`target`: app ou título).",
            schema: obj(json!({ "target": { "type": "string" } }), &["target"]),
        },
        Tool {
            action: "window.minimize",
            description: "Minimiza uma janela (`target`: app ou título). Para minimizar TODAS as \
                          janelas de um monitor ('minimiza as janelas do monitor 1'), passe só `monitor` \
                          (o número que window.list mostra) e NÃO `target`: é uma chamada só.",
            schema: obj(
                json!({ "target": { "type": "string" }, "monitor": { "type": "integer", "minimum": 1 } }),
                &[],
            ),
        },
        Tool {
            action: "window.maximize",
            description: "Maximiza uma janela (`target`: app ou título).",
            schema: obj(json!({ "target": { "type": "string" } }), &["target"]),
        },
        Tool {
            action: "window.restore",
            description: "Restaura uma janela minimizada ou maximizada (`target`: app ou título).",
            schema: obj(json!({ "target": { "type": "string" } }), &["target"]),
        },
        Tool {
            action: "window.close",
            description: "Fecha uma janela (como clicar no X). Só quando o usuário pedir para fechar.",
            schema: obj(json!({ "target": { "type": "string" } }), &["target"]),
        },
        Tool {
            action: "cursor.point",
            description: "Aponta com o seu cursor para uma janela ou monitor, para mostrar algo ao usuário \
                          ('onde está o discord?'). `target`: app/título, ou `monitor`.",
            schema: obj(
                json!({ "target": { "type": "string" }, "monitor": { "type": ["integer", "string"] } }),
                &[],
            ),
        },
        // ── memória de longo prazo ──────────────────────────────────────
        Tool {
            action: "memory.search",
            description: "Procura em TODAS as conversas anteriores com o usuário (outras abas e dias). \
                          Use para 'o que eu te falei sobre X?', 'lembra quando…', 'qual era a senha \
                          do wifi que eu te passei?'. `query`: palavras-chave.",
            schema: obj(json!({ "query": { "type": "string" } }), &["query"]),
        },
        Tool {
            action: "memory.remember",
            description: "Guarda um fato duradouro sobre o usuário quando ele pedir para você lembrar \
                          ('lembra que meu time é o Corinthians'). `key` curta, `value` curto.",
            schema: obj(
                json!({ "key": { "type": "string" }, "value": { "type": "string" } }),
                &["key", "value"],
            ),
        },
        Tool {
            action: "reminder.add",
            description: "Cria um LEMBRETE com data: a Lilith avisa no chat quando ele abrir naquele dia (ou \
                          depois). Use para 'me lembra dia 12/10 de…', 'lembra que amanhã eu preciso…'. \
                          `date`: se o usuário disse uma data numérica (dia/mês, formato BRASILEIRO: 12/10 = 12 de OUTUBRO), \n                          passe exatamente como ele disse (ex.: '12/10'); senão AAAA-MM-DD calculado da data de hoje. \n                          `time`: horário (HH:MM, 24h) quando o usuário disser ('às 15h' = '15:00'); sem \
                          horário, avisa assim que o dia começa. `text`: o que lembrar.",
            schema: obj(
                json!({ "date": { "type": "string" }, "time": { "type": "string" }, "text": { "type": "string" } }),
                &["date", "text"],
            ),
        },
        Tool {
            action: "memory.forget",
            description: "Esquece um fato guardado sobre o usuário ('esquece meu endereço').",
            schema: obj(json!({ "key": { "type": "string" } }), &["key"]),
        },
        // ── agentes de código ───────────────────────────────────────────
        Tool {
            action: "agent.delegate",
            description: "Delega uma tarefa de programação, terminal ou configuração a um agente de código                           que roda em segundo plano (ex.: 'claude' = Claude Code, 'codex' = Codex).                           Escreva em `task` um prompt completo e autossuficiente, como se fosse para um                           dev que não viu a conversa. `folder` é a pasta do projeto (OBRIGATÓRIA para consertar um projeto: ache com file.find, ex.: o .sln). Para vulnerabilidade de pacote, ponha em `task` o que foi lido: pacote, versão, projetos afetados e a solução. O app já acrescenta as regras de segurança (o agente não mexe em git) e o procedimento de correção. Retorna o id do job;                           o usuário acompanha pelo painel da Lilith.",
            schema: obj(
                json!({
                    "agent": { "type": "string", "description": "Nome do agente: 'claude' ou 'codex'" },
                    "task": { "type": "string" },
                    "folder": { "type": "string", "description": "Pasta de trabalho (padrão: pasta do usuário)" }
                }),
                &["agent", "task"],
            ),
        },
        Tool {
            action: "agent.jobs",
            description: "Lista as tarefas delegadas a agentes de código e o status/resultado de cada uma.",
            schema: obj(json!({}), &[]),
        },
        // ── ajudante de dev ─────────────────────────────────────────────
        Tool {
            action: "dev.errors",
            description: "Lê os ERROS DE BUILD/compilação que aparecem no Visual Studio (Lista de Erros, Saída), \
                          no VS Code ou num terminal, já separados (código, arquivo, linha, mensagem). Sem \
                          `target`, usa a janela de desenvolvimento aberta; `target`: 'terminal', 'devenv' \
                          (Visual Studio) ou 'code' (VS Code). Use para 'quais erros tem no build', 'por que \
                          não compila', 'explica esse erro' — explique em linguagem simples com base em \
                          `errors`. Só vê o que está visível; se vier vazio, diga isso, não invente.",
            schema: obj(json!({ "target": { "type": "string" } }), &[]),
        },
        Tool {
            action: "dev.git_status",
            description: "Resumo do GIT de um projeto, SÓ LEITURA: branch, commits à frente/atrás, arquivos \
                          modificados/novos e os últimos commits. `folder`: pasta do projeto; `project`: nome \
                          da pasta; sem os dois, usa o projeto aberto no Visual Studio/VS Code/terminal. Nunca \
                          muda o repositório (commit, push, checkout ficam com o usuário).",
            schema: obj(
                json!({ "folder": { "type": "string" }, "project": { "type": "string" } }),
                &[],
            ),
        },
        Tool {
            action: "dev.fix",
            description: "Pede a um agente de código para CORRIGIR os erros de build que aparecem no Visual \
                          Studio/VS Code/terminal. O app lê os erros, acha a pasta do projeto e monta o pedido \
                          (o agente não mexe em git e roda o build/testes). `agent`: 'claude' (padrão) ou \
                          'codex'; `folder` opcional. Pede aprovação. Nunca diga que corrigiu: o agente roda em \
                          segundo plano e o resultado aparece no painel.",
            schema: obj(
                json!({
                    "agent": { "type": "string" },
                    "target": { "type": "string" },
                    "folder": { "type": "string" }
                }),
                &[],
            ),
        },
        // ── web ─────────────────────────────────────────────────────────
        Tool {
            action: "web.search",
            description: "Pesquisa na web. Retorna títulos, URLs e trechos.",
            schema: obj(
                json!({ "query": { "type": "string" }, "limit": { "type": "integer" } }),
                &["query"],
            ),
        },
        Tool {
            action: "web.fetch",
            description: "Lê o texto de uma página web (para resumir ou extrair informação).",
            schema: obj(json!({ "url": { "type": "string" } }), &["url"]),
        },
    ]
}

/// Formato do array `tools` do `/api/chat` do Ollama (compatível com OpenAI).
pub fn ollama_tools_payload(tools: &[Tool]) -> Value {
    Value::Array(
        tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": { "name": t.action, "description": t.description, "parameters": t.schema }
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_are_unique() {
        let tools = all_tools();
        let mut names: Vec<_> = tools.iter().map(|t| t.action).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), tools.len());
    }
}
