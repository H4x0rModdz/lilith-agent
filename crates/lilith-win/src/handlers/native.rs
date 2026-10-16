//! Atalhos para apps nativos do Windows: páginas das Configurações
//! (`ms-settings:`) e o recorte de tela (`ms-screenclip:`). O mapa é fixo e
//! decidido por código — o modelo só diz a página em palavras ("bluetooth",
//! "som", "tela"); URI nenhuma vem do modelo.

use super::{apps::shell_open, blocking};
use crate::paths::fold;
use crate::WinError;
use serde_json::{json, Value};

/// (URI, rótulo em pt-BR, apelidos sem acento). O primeiro apelido que casar
/// com a frase inteira (ou como palavra/expressão dentro dela) vence; a ordem
/// importa: o mais específico primeiro ("apps padrao" antes de "apps").
pub(crate) const PAGES: &[(&str, &str, &[&str])] = &[
    ("ms-settings:defaultapps", "Aplicativos padrão", &["apps padrao", "aplicativos padrao", "programas padrao", "app padrao", "default apps"]),
    ("ms-settings:startupapps", "Aplicativos de inicialização", &["inicializacao", "apps de inicializacao", "iniciar com o windows", "startup"]),
    ("ms-settings:appsfeatures", "Aplicativos instalados", &["apps", "aplicativos", "programas", "apps instalados", "aplicativos instalados", "desinstalar", "apps e recursos"]),
    ("ms-settings:bluetooth", "Bluetooth e dispositivos", &["bluetooth", "dispositivos bluetooth", "fone bluetooth"]),
    ("ms-settings:printers", "Impressoras e scanners", &["impressora", "impressoras", "scanner", "scanners", "printers"]),
    ("ms-settings:mousetouchpad", "Mouse", &["mouse"]),
    ("ms-settings:devices-touchpad", "Touchpad", &["touchpad", "touch pad"]),
    ("ms-settings:mobile-devices", "Dispositivos móveis", &["celular", "dispositivos moveis", "telefone", "vincular ao celular"]),
    ("ms-settings:connecteddevices", "Dispositivos", &["dispositivos", "devices"]),
    ("ms-settings:nightlight", "Luz noturna", &["luz noturna", "filtro de luz azul", "luz azul", "night light"]),
    ("ms-settings:display", "Tela", &["tela", "monitor", "monitores", "display", "resolucao", "brilho", "escala", "video"]),
    ("ms-settings:sound", "Som", &["som", "audio", "sons", "alto-falante", "alto falante", "microfone do pc", "saida de audio", "sound"]),
    ("ms-settings:quiethours", "Foco / Não perturbe", &["nao perturbe", "foco", "modo foco", "assistente de foco", "do not disturb"]),
    ("ms-settings:notifications", "Notificações", &["notificacoes", "notificacao", "notifications"]),
    ("ms-settings:batterysaver", "Bateria", &["bateria", "economia de bateria", "economizador de bateria", "battery"]),
    ("ms-settings:powersleep", "Energia", &["energia", "suspensao", "desligar a tela", "power", "modo de energia"]),
    ("ms-settings:storagesense", "Armazenamento", &["armazenamento", "disco", "espaco em disco", "storage", "limpeza de disco"]),
    ("ms-settings:multitasking", "Multitarefa", &["multitarefa", "ajustar janelas", "snap"]),
    ("ms-settings:clipboard", "Área de transferência", &["area de transferencia", "clipboard", "historico da area de transferencia"]),
    ("ms-settings:project", "Projetar neste PC", &["projetar", "projecao", "segunda tela"]),
    ("ms-settings:about", "Sobre o PC", &["sobre", "sobre o pc", "informacoes do sistema", "especificacoes", "about", "nome do pc"]),
    ("ms-settings:network-wifi", "Wi-Fi", &["wi-fi", "wifi", "wi fi", "rede sem fio", "redes sem fio"]),
    ("ms-settings:network-vpn", "VPN", &["vpn"]),
    ("ms-settings:network-proxy", "Proxy", &["proxy"]),
    ("ms-settings:network-mobilehotspot", "Hotspot móvel", &["hotspot", "ponto de acesso", "roteador movel", "compartilhar internet"]),
    ("ms-settings:network-airplanemode", "Modo avião", &["modo aviao", "aviao", "airplane mode"]),
    ("ms-settings:network-ethernet", "Ethernet", &["ethernet", "cabo de rede"]),
    ("ms-settings:network-status", "Rede e Internet", &["rede", "internet", "redes", "network", "conexao"]),
    ("ms-settings:personalization-background", "Plano de fundo", &["papel de parede", "plano de fundo", "fundo de tela", "wallpaper"]),
    ("ms-settings:colors", "Cores", &["cores", "cor", "modo escuro", "tema escuro", "modo claro", "cor de destaque", "dark mode"]),
    ("ms-settings:themes", "Temas", &["temas", "tema", "themes"]),
    ("ms-settings:lockscreen", "Tela de bloqueio", &["tela de bloqueio", "lock screen"]),
    ("ms-settings:personalization-start", "Iniciar", &["menu iniciar", "iniciar", "start"]),
    ("ms-settings:taskbar", "Barra de tarefas", &["barra de tarefas", "taskbar"]),
    ("ms-settings:fonts", "Fontes", &["fontes", "fonte", "fonts"]),
    ("ms-settings:personalization", "Personalização", &["personalizacao", "personalizar"]),
    ("ms-settings:yourinfo", "Suas informações", &["conta", "contas", "minha conta", "suas informacoes", "accounts"]),
    ("ms-settings:signinoptions", "Opções de entrada", &["opcoes de entrada", "senha", "pin", "windows hello", "login", "biometria"]),
    ("ms-settings:backup", "Backup do Windows", &["backup"]),
    ("ms-settings:dateandtime", "Data e hora", &["data e hora", "data", "hora", "horario", "fuso horario", "relogio"]),
    ("ms-settings:regionlanguage", "Idioma e região", &["idioma", "idiomas", "lingua", "regiao", "language"]),
    ("ms-settings:keyboard", "Teclado", &["teclado", "keyboard", "layout do teclado"]),
    ("ms-settings:typing", "Digitação", &["digitacao", "corretor", "autocorrecao"]),
    ("ms-settings:gaming-gamemode", "Modo de jogo", &["modo de jogo", "modo jogo", "game mode"]),
    ("ms-settings:gaming-gamebar", "Game Bar", &["game bar", "xbox game bar", "jogos", "gravacao de jogos"]),
    ("ms-settings:easeofaccess", "Acessibilidade", &["acessibilidade", "accessibility"]),
    ("ms-settings:privacy-microphone", "Privacidade do microfone", &["permissao do microfone", "privacidade do microfone", "microfone"]),
    ("ms-settings:privacy-webcam", "Privacidade da câmera", &["permissao da camera", "privacidade da camera", "camera", "webcam"]),
    ("ms-settings:privacy-location", "Localização", &["localizacao", "location", "gps"]),
    ("ms-settings:privacy", "Privacidade e segurança", &["privacidade", "privacy"]),
    ("ms-settings:windowsdefender", "Segurança do Windows", &["seguranca do windows", "antivirus", "defender", "windows defender", "seguranca"]),
    ("ms-settings:windowsupdate", "Windows Update", &["windows update", "atualizacao", "atualizacoes", "atualizar o windows", "atualizar", "update"]),
    ("ms-settings:recovery", "Recuperação", &["recuperacao", "restaurar o pc", "resetar o pc", "recovery"]),
    ("ms-settings:activation", "Ativação", &["ativacao", "ativar o windows", "ativar", "chave do windows", "activation"]),
    ("ms-settings:troubleshoot", "Solução de problemas", &["solucao de problemas", "solucionar problemas", "troubleshoot"]),
];

/// Página das Configurações para a frase. `None` = não sei qual (vai para o início).
pub(crate) fn page_for(raw: &str) -> Option<(&'static str, &'static str)> {
    // Sem acento, sem artigos/palavras de enchimento nas pontas.
    const FILLER: &[&str] = &[
        "configuracoes", "configuracao", "config", "ajustes", "definicoes", "settings", "pagina", "as",
        "os", "a", "o", "de", "do", "da", "dos", "das", "no", "na", "em", "meu", "minha", "meus", "minhas", "pc",
        "computador", "windows",
    ];
    let mut words: Vec<String> = fold(raw).split_whitespace().map(str::to_string).collect();
    while words.first().is_some_and(|w| FILLER.contains(&w.as_str())) {
        words.remove(0);
    }
    while words.last().is_some_and(|w| FILLER.contains(&w.as_str())) {
        words.pop();
    }
    let t = words.join(" ");
    if t.is_empty() {
        return None;
    }
    // 1) frase inteira = apelido; 2) apelido como expressão dentro da frase.
    for (uri, label, aliases) in PAGES {
        if aliases.contains(&t.as_str()) {
            return Some((uri, label));
        }
    }
    let padded = format!(" {t} ");
    PAGES
        .iter()
        .find(|(_, _, aliases)| aliases.iter().any(|a| padded.contains(&format!(" {a} "))))
        .map(|(uri, label, _)| (*uri, *label))
}

/// A janela das Configurações apareceu? (confirma em vez de supor)
fn settings_window_shown() -> bool {
    crate::window::list().into_iter().any(|w| {
        let app = fold(&w.app);
        app == "systemsettings" || matches!(fold(&w.title).as_str(), "configuracoes" | "settings")
    })
}

/// Espera (até ~3 s) a condição ficar verdadeira.
async fn wait_for(check: fn() -> bool) -> bool {
    for _ in 0..12 {
        if blocking(move || Ok(check())).await.unwrap_or(false) {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    false
}

/// `settings.open`: `page` em palavras ("bluetooth", "som", "modo escuro").
/// Página desconhecida abre o início das Configurações e avisa.
pub async fn settings_open(params: Value) -> Result<Value, WinError> {
    let asked = params["page"].as_str().unwrap_or("").trim().to_string();
    let found = page_for(&asked);
    let (uri, label) = found.unwrap_or(("ms-settings:", "Início"));
    shell_open(uri.to_string()).await?;
    let verified = wait_for(settings_window_shown).await;
    Ok(json!({
        "opened": label,
        "uri": uri,
        "asked": asked,
        "fallback": found.is_none() && !asked.is_empty(),
        "verified": verified,
    }))
}

/// O recorte de tela está na frente? (a sobreposição é do ScreenClippingHost)
fn snip_shown() -> bool {
    crate::window::list().into_iter().any(|w| {
        let app = fold(&w.app);
        app.contains("screenclipping") || app.contains("snippingtool")
    })
}

/// `screen.snip`: abre o recorte de tela (o mesmo do Win+Shift+S). O
/// usuário escolhe a área; o recorte vai para a área de transferência.
pub async fn screen_snip(_params: Value) -> Result<Value, WinError> {
    shell_open("ms-screenclip:".to_string()).await?;
    let verified = wait_for(snip_shown).await;
    Ok(json!({ "opened": "recorte de tela", "verified": verified }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_are_decided_by_a_fixed_map() {
        assert_eq!(page_for("bluetooth").unwrap().0, "ms-settings:bluetooth");
        assert_eq!(page_for("as configurações de Bluetooth").unwrap().0, "ms-settings:bluetooth");
        assert_eq!(page_for("Wi-Fi").unwrap().0, "ms-settings:network-wifi");
        assert_eq!(page_for("som").unwrap().0, "ms-settings:sound");
        assert_eq!(page_for("tela").unwrap().0, "ms-settings:display");
        assert_eq!(page_for("tela de bloqueio").unwrap().0, "ms-settings:lockscreen");
        assert_eq!(page_for("modo escuro").unwrap().0, "ms-settings:colors");
        assert_eq!(page_for("notificações").unwrap().0, "ms-settings:notifications");
        assert_eq!(page_for("apps padrão").unwrap().0, "ms-settings:defaultapps");
        assert_eq!(page_for("aplicativos").unwrap().0, "ms-settings:appsfeatures");
        assert_eq!(page_for("energia").unwrap().0, "ms-settings:powersleep");
        assert_eq!(page_for("windows update").unwrap().0, "ms-settings:windowsupdate");
        assert_eq!(page_for("rede").unwrap().0, "ms-settings:network-status");
        assert_eq!(page_for("do microfone").unwrap().0, "ms-settings:privacy-microphone");
        assert_eq!(page_for("xyzzy"), None);
        assert_eq!(page_for("configurações"), None, "só 'configurações' = início");
    }

    #[test]
    fn every_uri_is_a_settings_page() {
        for (uri, label, aliases) in PAGES {
            assert!(uri.starts_with("ms-settings:") && !uri.contains(' '), "{uri}");
            assert!(!label.is_empty() && !aliases.is_empty());
            for a in *aliases {
                assert_eq!(fold(a), *a, "apelido sem acento: {a}");
            }
        }
    }
}
