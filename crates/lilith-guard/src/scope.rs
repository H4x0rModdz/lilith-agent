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
];

const DANGEROUS_SCOPES: &[&str] = &[
    "app.install",
    "app.uninstall",
    "filesystem.write",
    "filesystem.delete",
    "settings.modify",
    "terminal.execute",
    "system.power",
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
];

/// Scope exigido por cada ação. Ação sem mapeamento vira `unknown` (negada) —
/// ao adicionar uma ação, adicione aqui e no teste `no_action_is_unknown`.
pub fn scope_for(action: &str) -> &'static str {
    match action {
        "app.open" | "browser.open" | "browser.search" | "file.open" => "app.launch",
        // Navegar entre abas é só mexer na janela; fechar aba é decidido pelo app.
        "browser.tab" => "window.control",
        "app.close" | "window.close" => "app.terminate",
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
        "system.status" => "system.read",
        // OCR da tela pode entregar senhas e conversas, como window.read.
        "screen.read" => "screen.read",
        "clipboard.set" => "clipboard.write",
        "clipboard.get" => "clipboard.read",
        "web.search" | "web.fetch" => "web.access",
        "system.power" => "system.power",
        "agent.delegate" => "agent.delegate",
        "agent.jobs" => "agent.read",
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
            "screen.read",
            "memory.search",
            "memory.remember",
            "reminder.add",
            "memory.forget",
            "memory.write_unrequested",
        ] {
            let scope = scope_for(action);
            assert_ne!(scope, "unknown", "{action} sem scope");
            assert_ne!(classify(scope), Verdict::Unknown, "{action} → {scope} sem classificação");
        }
    }
}
