/// Classificação padrão de um scope, antes das regras do usuário.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Seguro por padrão — executa sem perguntar.
    Safe,
    /// Perigoso — pede aprovação.
    Dangerous,
    /// Desconhecido — negado.
    Unknown,
}

/// Classifica um scope pela lista embutida.
///
/// Casa pelo nome exato ou por prefixo pontuado: `filesystem.write` cobre
/// `filesystem.write.downloads` — o sub-scope herda o perigo do pai.
pub fn classify(scope: &str) -> Verdict {
    if matches_any(scope, SAFE_SCOPES) {
        return Verdict::Safe;
    }
    if matches_any(scope, DANGEROUS_SCOPES) {
        return Verdict::Dangerous;
    }
    Verdict::Unknown
}

pub(crate) fn matches_any(scope: &str, bucket: &[&str]) -> bool {
    bucket
        .iter()
        .any(|prefix| scope == *prefix || scope.starts_with(&format!("{prefix}.")))
}

const SAFE_SCOPES: &[&str] = &[
    "app.launch",
    "window.control",
    "system.notify",
    "filesystem.read",
    // Volume/mudo são reversíveis pelo usuário na hora.
    "audio.control",
    // Escrever no clipboard não vaza nada; ler é outra história.
    "clipboard.write",
    // Trocar wallpaper é cosmético e reversível.
    "desktop.modify",
    // Pesquisar na web é a função de pesquisa que o usuário pediu.
    "web.access",
    // A memória é dela e o usuário a vê/edita no painel. Gravar depois de
    // ler conteúdo externo cai no portão de contaminação (pergunta).
    "memory.read",
    "memory.write",
    // Navegar dentro de um programa (abrir uma conversa, trocar de aba) é o
    // que o usuário faria clicando; ações destrutivas têm tools próprias.
    "ui.interact",
    // Ler o status das tarefas delegadas.
    "agent.read",
    // Tocar/pausar/pular música e ver o que toca.
    "media.control",
    // Medir o PC (CPU, memória, discos, processos): só leitura.
    "system.read",
    // Tirar um print e salvar no próprio PC, como a tecla Print Screen.
    "screen.capture",
    // Ajudante de dev: erros de build (só o que o parser reconhece, de janelas
    // de desenvolvimento) e git SÓ LEITURA (comandos fixos no código).
    "dev.read",
    // Estado do celular no Vincular ao Celular (conectado, bateria, sinal).
    "phone.status",
    // Rotinas do usuário: rodar um modo é seguro porque CADA passo passa de
    // novo pelo guard; criar/apagar modo ou layout mostra prévia e pede
    // confirmação no próprio app.
    "routine.run",
    "routine.read",
    "routine.write",
];

const DANGEROUS_SCOPES: &[&str] = &[
    "app.install",
    "app.uninstall",
    "filesystem.write",
    "filesystem.delete",
    "settings.modify",
    "terminal.execute",
    "system.power",
    // Parar/iniciar serviços do Windows (Docker, SQL Server…).
    "system.service",
    "microphone.listen",
    "camera.access",
    // Clipboard e tela podem entregar senhas, conversas, banco.
    "clipboard.read",
    "screen.read",
    // Mouse/teclado sintéticos dirigem qualquer app, inclusive campo de senha.
    "input.control",
    // Um agente de código edita arquivos e roda ferramentas por conta própria.
    "agent.delegate",
    // Ler o conteúdo de arquivos fora das pastas pessoais (ou com cara de
    // segredo). Nome sem ponto de propósito: não herda de `filesystem.read`.
    "filesystem.read_protected",
    // Clicar num botão/menu (ou em algo com verbo de efeito) dentro de um
    // programa pode enviar, apagar, pagar… Nome sem ponto: não herda de
    // ui.interact.
    "ui.press",
    // Gravar/apagar memória sem o usuário ter pedido no chat: o que entra
    // aqui vai para o contexto de todos os pedidos futuros (injeção
    // persistente). Nome sem ponto: não herda de memory.write.
    "memory.persist",
    // Fechar um app/janela pode perder trabalho não salvo — e um modelo
    // pequeno já confundiu "joga o WhatsApp pro lado" com fechar.
    "app.terminate",
    // Notificações, SMS e chamadas do celular: conversas e códigos de
    // verificação (2FA) passam por ali — como ler a tela, pergunta.
    "phone.read",
    // Mandar SMS fala em nome do usuário.
    "phone.send",
];

/// Scope exigido por cada ação. Ação sem mapeamento vira `unknown` (negada) —
/// ao adicionar uma ação, adicione aqui e no teste `no_action_is_unknown`.
pub fn scope_for(action: &str) -> &'static str {
    match action {
        "app.open" | "browser.open" | "browser.search" | "file.open" => "app.launch",
        // Navegar entre abas é só mexer na janela; fechar aba é decidido pelo app.
        "browser.tab" => "window.control",
        "app.close" | "window.close" => "app.terminate",
        // Instalar programa: a prévia mostra o pacote exato (winget) antes.
        "app.install" => "app.install",
        // Ver o estado de um serviço é só leitura; parar/iniciar pede aprovação
        // (e o UAC do Windows, que é quem dá o admin).
        "service.status" => "system.read",
        "service.control" => "system.service",
        "window.list" | "window.focus" | "window.minimize" | "window.maximize" | "window.restore" | "window.move"
        | "window.tile" | "cursor.point" | "ui.find" => "window.control",
        "ui.click" | "ui.open_chat" => "ui.interact",
        // Digitar/enviar texto num app = falar em nome do usuário.
        "ui.type" => "input.control",
        "memory.search" => "memory.read",
        "memory.remember" | "memory.forget" | "reminder.add" => "memory.write",
        // Escalonado pelo app quando o chat não pediu isso.
        "memory.write_unrequested" => "memory.persist",
        // Escalonado pelo app quando o alvo não é pura navegação.
        "ui.click_effect" => "ui.press",
        "file.find" | "file.list" | "file.read" => "filesystem.read",
        // Escalonado pelo app quando o caminho não é pessoal (ver lilith_win::is_personal_read).
        "file.read_protected" => "filesystem.read_protected",
        // Ler a tela pode entregar senhas e conversas: pede aprovação.
        "window.read" => "screen.read",
        // Executar um arquivo = rodar código arbitrário.
        "file.run" => "terminal.execute",
        "file.move" | "file.copy" => "filesystem.write",
        "download.file" => "filesystem.write.downloads",
        "file.delete" => "filesystem.delete",
        // Varrer pastas (duplicados, maiores arquivos) e o índice de
        // documentos: só leitura. Indexar pasta fora das pessoais é escalado
        // pelo app para leitura protegida.
        "file.duplicates" | "file.big" | "docs.index" | "docs.search" => "filesystem.read",
        // Organizar (e desfazer) move arquivos do usuário: pede, com a prévia.
        "file.organize" | "file.organize_undo" => "filesystem.write",
        // Mandar as cópias duplicadas para a Lixeira: pede, com a prévia.
        "file.dedupe" => "filesystem.delete",
        "desktop.set_wallpaper" => "desktop.modify",
        "audio.set_volume" | "audio.get_volume" | "audio.set_mute" | "audio.app_volume" => "audio.control",
        // Microfone, áudio, dispositivos, status e conta do Discord: o que o
        // usuário faria clicando, e reversível. Ler pedidos pendentes também.
        "discord.voice" | "discord.status" | "discord.account" | "discord.pending" => "ui.interact",
        // Aceitar/recusar/enviar pedido de amizade age em nome do usuário: pergunta.
        "discord.friend" => "ui.press",
        // Tocar/pausar/pular e ler o que toca: reversível e inofensivo.
        "media.control" | "media.now_playing" => "media.control",
        // Medir CPU/memória/discos: só leitura, nada pessoal.
        "system.status" | "system.checkup" | "self.diagnose" => "system.read",
        // Relatório de bug: a ação mostra o que vai e só envia com o "Enviar"
        // do próprio cartão — o consentimento é ali, não numa aprovação a mais.
        "self.report" => "system.read",
        // Print salvo no PC do usuário a pedido dele (o modelo não vê a imagem).
        "screen.capture" => "screen.capture",
        // OCR da tela pode entregar senhas e conversas, como window.read.
        "screen.read" => "screen.read",
        "clipboard.set" => "clipboard.write",
        "clipboard.get" => "clipboard.read",
        // Traduzir/resumir/corrigir o que foi copiado lê o clipboard.
        "clipboard.transform" => "clipboard.read",
        "web.search" | "web.fetch" => "web.access",
        "system.power" => "system.power",
        "agent.delegate" => "agent.delegate",
        "agent.jobs" => "agent.read",
        // Erros de build e resumo do git: só leitura.
        "dev.errors" | "dev.git_status" => "dev.read",
        // Pedir a um agente de código para corrigir os erros = delegar.
        "dev.fix" => "agent.delegate",
        // Celular (Vincular ao Celular): estado é livre; ler conteúdo e enviar perguntam.
        "phone.status" => "phone.status",
        "phone.notifications" | "phone.messages" | "phone.calls" => "phone.read",
        "phone.send_sms" => "phone.send",
        // Páginas das Configurações e recorte de tela: só abrem um app nativo.
        "settings.open" | "screen.snip" => "app.launch",
        // Modos: os passos passam um a um pelo guard ao rodar.
        "mode.run" => "routine.run",
        "mode.list" | "layout.list" => "routine.read",
        "mode.save" | "mode.delete" | "layout.save" | "layout.delete" => "routine.write",
        // Restaurar um layout move janelas; abrir os apps que faltam passa
        // pelo guard como app.open.
        "layout.restore" => "window.control",
        _ => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_and_dangerous_defaults() {
        assert_eq!(classify("app.launch"), Verdict::Safe);
        assert_eq!(classify("desktop.modify"), Verdict::Safe);
        assert_eq!(classify("filesystem.delete"), Verdict::Dangerous);
        assert_eq!(classify("input.control"), Verdict::Dangerous);
        assert_eq!(classify(scope_for("app.close")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("window.close")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("window.move")), Verdict::Safe);
        assert_eq!(classify(scope_for("file.read_protected")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("ui.click")), Verdict::Safe);
        assert_eq!(classify(scope_for("ui.click_effect")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("memory.write_unrequested")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("dev.errors")), Verdict::Safe);
        assert_eq!(classify(scope_for("dev.git_status")), Verdict::Safe);
        assert_eq!(classify(scope_for("dev.fix")), Verdict::Dangerous, "corrigir = delegar: pede aprovação");
        assert_eq!(classify(scope_for("phone.status")), Verdict::Safe);
        assert_eq!(classify(scope_for("phone.messages")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("phone.send_sms")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("settings.open")), Verdict::Safe);
        assert_eq!(classify(scope_for("file.organize")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("file.organize_undo")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("file.dedupe")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("clipboard.transform")), Verdict::Dangerous);
        assert_eq!(classify(scope_for("file.duplicates")), Verdict::Safe);
        assert_eq!(classify(scope_for("docs.search")), Verdict::Safe);
    }

    #[test]
    fn dotted_subscopes_inherit_parent() {
        assert_eq!(classify("filesystem.write.downloads"), Verdict::Dangerous);
        assert_eq!(classify("filesystem.read.C:/Users"), Verdict::Safe);
    }

    #[test]
    fn unknown_and_substring_are_denied() {
        assert_eq!(classify("totally.made.up"), Verdict::Unknown);
        assert_eq!(classify("app.install_helper"), Verdict::Unknown);
        assert_eq!(classify(""), Verdict::Unknown);
    }

    #[test]
    fn no_action_is_unknown() {
        for action in [
            "app.open",
            "app.close",
            "app.install",
            "service.status",
            "service.control",
            "browser.open",
            "browser.search",
            "browser.tab",
            "file.open",
            "file.run",
            "file.find",
            "file.list",
            "file.read",
            "file.read_protected",
            "window.read",
            "file.move",
            "file.copy",
            "file.delete",
            "file.organize",
            "file.organize_undo",
            "file.duplicates",
            "file.dedupe",
            "file.big",
            "docs.index",
            "docs.search",
            "clipboard.transform",
            "download.file",
            "desktop.set_wallpaper",
            "audio.set_volume",
            "audio.get_volume",
            "audio.set_mute",
            "clipboard.set",
            "clipboard.get",
            "web.search",
            "web.fetch",
            "system.power",
            "agent.delegate",
            "agent.jobs",
            "window.list",
            "window.focus",
            "window.minimize",
            "window.maximize",
            "window.restore",
            "window.close",
            "window.move",
            "window.tile",
            "cursor.point",
            "ui.find",
            "ui.click",
            "ui.click_effect",
            "ui.type",
            "ui.open_chat",
            "audio.app_volume",
            "discord.voice",
            "discord.status",
            "discord.account",
            "discord.pending",
            "discord.friend",
            "media.control",
            "media.now_playing",
            "system.status",
            "self.diagnose",
            "self.report",
            "system.checkup",
            "screen.capture",
            "screen.read",
            "memory.search",
            "memory.remember",
            "reminder.add",
            "memory.forget",
            "memory.write_unrequested",
            "dev.errors",
            "dev.git_status",
            "dev.fix",
            "phone.status",
            "phone.notifications",
            "phone.messages",
            "phone.calls",
            "phone.send_sms",
            "settings.open",
            "screen.snip",
            "mode.run",
            "mode.save",
            "mode.list",
            "mode.delete",
            "layout.save",
            "layout.restore",
            "layout.list",
            "layout.delete",
        ] {
            let scope = scope_for(action);
            assert_ne!(scope, "unknown", "{action} sem scope");
            assert_ne!(classify(scope), Verdict::Unknown, "{action} → {scope} sem classificação");
        }
    }
}
