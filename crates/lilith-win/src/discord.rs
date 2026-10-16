//! Controles do Discord decididos por código: microfone, áudio (ensurdecer),
//! dispositivos de entrada/saída, status, troca de conta e amizades.
//!
//! Tudo pela acessibilidade do próprio Discord (nomes em pt-BR). Os nomes dos
//! botões de microfone/áudio mudam com o estado, então eles são achados pela
//! POSIÇÃO, ao lado das âncoras fixas "Opções de entrada"/"Opções de saída";
//! o estado vem do nome atual. O código só clica se o estado for diferente do
//! pedido e confere depois.

use crate::paths::fold;
use crate::uia::{self, Found};
use crate::window::{self, WindowInfo};
use crate::WinError;
use serde_json::{json, Value};
use std::time::Duration;

fn pause(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// O Discord na frente e restaurado: minimizado ele não expõe nada.
fn open_discord() -> Result<WindowInfo, WinError> {
    let w = window::find("discord")
        .map_err(|_| WinError::NotFound("o Discord não está aberto".into()))?;
    if w.minimized {
        window::set_state(w.id, "restore")?;
        pause(1500);
    }
    let _ = window::focus(w.id);
    pause(300);
    Ok(w)
}

/// Fecha menus e janelinhas que tenham ficado abertos.
fn close_menus(times: usize) {
    for _ in 0..times {
        let _ = crate::input::press_escape();
        pause(250);
    }
}

/// O botão logo à esquerda de `anchor`, na mesma linha.
fn left_of(all: &[Found], anchor: &str) -> Option<usize> {
    let a = all.iter().find(|f| fold(&f.info.name) == fold(anchor))?;
    let (ay, ax, ah) = (a.info.rect.y + a.info.rect.h / 2, a.info.rect.x, a.info.rect.h.max(12));
    all.iter()
        .enumerate()
        .filter(|(_, f)| {
            f.info.kind == "botão"
                && f.info.rect.x + f.info.rect.w <= ax + 4
                && (f.info.rect.y + f.info.rect.h / 2 - ay).abs() <= ah
        })
        .max_by_key(|(_, f)| f.info.rect.x)
        .map(|(i, _)| i)
}

/// Microfone e áudio do painel de usuário: (índice do mic, índice do áudio).
fn voice_buttons(all: &[Found]) -> Result<(usize, usize), WinError> {
    let by_name = |names: &[&str]| all.iter().position(|f| f.info.kind == "botão" && names.iter().any(|n| fold(&f.info.name) == fold(n)));
    let mic = left_of(all, "Opções de entrada").or_else(|| by_name(&["Silenciar", "Desativar silenciamento", "Ativar microfone"]));
    let audio = left_of(all, "Opções de saída").or_else(|| by_name(&["Desativar áudio", "Ativar áudio"]));
    match (mic, audio) {
        (Some(m), Some(a)) => Ok((m, a)),
        _ => Err(WinError::NotFound("não achei os botões de microfone/áudio do Discord (painel de usuário)".into())),
    }
}

/// O microfone está mudo? O nome do botão é sempre "Silenciar"; o estado
/// vem de "marcado" (alternância/aria). Sem isso, cai no nome (versões antigas).
fn mic_is_muted(b: &Found) -> bool {
    b.is_on().unwrap_or_else(|| mic_name_says_muted(&b.info.name))
}

/// O áudio está desativado (ensurdecido)? Mesmo esquema do microfone.
fn audio_is_off(b: &Found) -> bool {
    b.is_on().unwrap_or_else(|| audio_name_says_off(&b.info.name))
}

pub(crate) fn mic_name_says_muted(name: &str) -> bool {
    fold(name) != fold("Silenciar")
}

pub(crate) fn audio_name_says_off(name: &str) -> bool {
    fold(name) != fold("Desativar áudio")
}

/// Estado atual (true = mudo/desativado) e onde está o botão.
fn read_state(id: isize, is_mic: bool) -> Option<(bool, crate::window::Rect)> {
    let all = uia::clickables(id).ok()?;
    let (mi, ai) = voice_buttons(&all).ok()?;
    let b = &all[if is_mic { mi } else { ai }];
    Some((if is_mic { mic_is_muted(b) } else { audio_is_off(b) }, b.info.rect))
}

/// Espera o estado virar `want` (até `ms`). O Discord atualiza em ~300 ms.
fn wait_state(id: isize, is_mic: bool, want: bool, ms: u64) -> bool {
    let mut waited = 0;
    while waited <= ms {
        if read_state(id, is_mic).is_some_and(|(s, _)| s == want) {
            return true;
        }
        pause(200);
        waited += 200;
    }
    false
}

/// "mute"/"unmute"/"toggle" → estado desejado (true = mudo/desativado).
fn wanted(op: &str, current: bool) -> Result<bool, WinError> {
    match op {
        "mute" | "deafen" | "off" => Ok(true),
        "unmute" | "undeafen" | "on" => Ok(false),
        "toggle" => Ok(!current),
        other => Err(WinError::InvalidParams(format!("'{other}': use mute/unmute ou deafen/undeafen"))),
    }
}

/// Escolhe um dispositivo no menu do botão (o do botão direito).
fn pick_device(w: &WindowInfo, button: &Found, submenu: &str, choice: &str) -> Result<Value, WinError> {
    button.context_menu()?;
    // O menu e o submenu demoram um pouco para aparecer: tenta algumas vezes.
    let mut opened = false;
    for _ in 0..4 {
        pause(500);
        if let Some(item) = uia::find(w.id, submenu)?.into_iter().find(|f| f.info.kind == "item de menu") {
            item.click()?;
            opened = true;
            break;
        }
    }
    if !opened {
        close_menus(1);
        return Err(WinError::NotFound(format!("o menu não mostrou '{submenu}'")));
    }
    pause(600);
    let options: Vec<Found> = uia::clickables(w.id)?.into_iter().filter(|f| f.info.kind == "opção").collect();
    let names: Vec<String> = options.iter().map(|f| f.info.name.clone()).collect();
    if choice.trim().is_empty() || fold(choice) == "list" {
        close_menus(2);
        return Ok(json!({ "devices": names }));
    }
    let want = fold(choice);
    let Some(opt) = options.iter().find(|f| fold(&f.info.name).contains(&want)) else {
        close_menus(2);
        return Err(WinError::NotFound(format!("não achei '{choice}' (disponíveis: {})", names.join(", "))));
    };
    opt.click()?;
    pause(400);
    close_menus(1);
    Ok(json!({ "selected": opt.info.name, "devices": names }))
}

/// `discord.voice`: `mic` (mute/unmute/toggle), `audio` (deafen/undeafen/toggle),
/// `input_device`/`output_device` (nome, ou "list" para listar). Sem nada: o estado atual.
pub fn voice(params: &Value) -> Result<Value, WinError> {
    let w = open_discord()?;
    let mut out = json!({});
    for (key, is_mic) in [("mic", true), ("audio", false)] {
        let Some(op) = params[key].as_str() else { continue };
        let all = uia::clickables(w.id)?;
        let (mi, ai) = voice_buttons(&all)?;
        let b = &all[if is_mic { mi } else { ai }];
        let state = |f: &Found| if is_mic { mic_is_muted(f) } else { audio_is_off(f) };
        let now = state(b);
        let want = wanted(op, now)?;
        let mut done = want == now;
        if !done {
            // 1) clique por acessibilidade; 2) se não pegou, clique real do mouse
            //    (o Discord às vezes ignora o segundo clique por acessibilidade).
            b.click()?;
            done = wait_state(w.id, is_mic, want, 1000);
            if !done {
                if let Some((_, rect)) = read_state(w.id, is_mic) {
                    let (x, y) = rect.center();
                    crate::input::click_at(x, y)?;
                    done = wait_state(w.id, is_mic, want, 1500);
                }
            }
        }
        let after = read_state(w.id, is_mic).map(|(s, _)| s).unwrap_or(!want);
        out[if is_mic { "mic_muted" } else { "audio_off" }] = json!(after);
        if !done {
            out["failed"] = json!(true);
            out["note"] = json!("cliquei, mas o Discord não mudou o estado");
        }
    }
    for (key, submenu, is_mic) in [("input_device", "Dispositivo de entrada", true), ("output_device", "Dispositivo de saída", false)] {
        let Some(choice) = params[key].as_str() else { continue };
        let all = uia::clickables(w.id)?;
        let (mi, ai) = voice_buttons(&all)?;
        out[key] = pick_device(&w, &all[if is_mic { mi } else { ai }], submenu, choice)?;
    }
    // Sem pedido de mudança: só diz o estado atual.
    if out.as_object().is_some_and(|o| o.is_empty()) {
        let all = uia::clickables(w.id)?;
        let (mi, ai) = voice_buttons(&all)?;
        out["mic_muted"] = json!(mic_is_muted(&all[mi]));
        out["audio_off"] = json!(audio_is_off(&all[ai]));
    }
    Ok(out)
}

/// Rótulos do menu de status para cada valor.
fn status_labels(status: &str) -> Option<&'static [&'static str]> {
    match fold(status).as_str() {
        "online" | "disponivel" => Some(&["Disponível", "Online"]),
        "idle" | "ausente" => Some(&["Ausente", "Ocioso"]),
        "dnd" | "nao perturbe" | "ocupado" => Some(&["Não perturbe"]),
        "invisible" | "invisivel" | "offline" => Some(&["Invisível"]),
        _ => None,
    }
}

/// Abre o painel do avatar (status, contas).
fn open_avatar(w: &WindowInfo) -> Result<(), WinError> {
    uia::click(w.id, "Status do usuário e configurações", 0)?;
    pause(900);
    Ok(())
}

/// `discord.status`: online | idle | dnd | invisible.
pub fn status(params: &Value) -> Result<Value, WinError> {
    let raw = params["status"].as_str().unwrap_or("");
    let labels = status_labels(raw).ok_or_else(|| WinError::InvalidParams("status: online, idle, dnd ou invisible".into()))?;
    let w = open_discord()?;
    open_avatar(&w)?;
    // "Seu status: …" abre a lista de status.
    if let Some(current) = uia::find(w.id, "Seu status")?.into_iter().next() {
        current.click()?;
        pause(700);
    }
    let options = uia::clickables(w.id)?;
    let pick = options.iter().find(|f| {
        let n = fold(&f.info.name);
        !n.starts_with("seu status") && labels.iter().any(|l| n.starts_with(&fold(l)))
    });
    let Some(pick) = pick else {
        close_menus(2);
        return Err(WinError::NotFound(format!("não achei a opção '{}' no menu de status", labels[0])));
    };
    pick.click()?;
    pause(500);
    close_menus(1);
    Ok(json!({ "status": labels[0] }))
}

/// Itens que não são contas na lista de troca de conta.
const NOT_ACCOUNTS: &[&str] = &["adicionar uma conta", "gerenciar contas", "fechar", "voltar", "sair", "mudar de conta"];

/// `discord.account`: troca para outra conta já logada (`account`; "list" lista).
pub fn account(params: &Value) -> Result<Value, WinError> {
    let choice = params["account"].as_str().unwrap_or("").trim().to_string();
    let w = open_discord()?;
    open_avatar(&w)?;
    let Some(switch) = uia::find(w.id, "Mudar de conta")?.into_iter().next() else {
        close_menus(2);
        return Err(WinError::NotFound("não achei 'Mudar de conta' no menu do avatar".into()));
    };
    switch.click()?;
    pause(1000);
    let items: Vec<Found> = uia::clickables(w.id)?
        .into_iter()
        .filter(|f| matches!(f.info.kind.as_str(), "botão" | "item de lista" | "opção" | "item de menu"))
        .filter(|f| !NOT_ACCOUNTS.iter().any(|x| fold(&f.info.name).contains(x)))
        .collect();
    let names: Vec<String> = items.iter().map(|f| f.info.name.clone()).collect();
    if choice.is_empty() || fold(&choice) == "list" {
        close_menus(2);
        return Ok(json!({ "accounts": names }));
    }
    let Some(target) = items.iter().find(|f| fold(&f.info.name).contains(&fold(&choice))) else {
        close_menus(2);
        return Err(WinError::NotFound(format!("não achei a conta '{choice}' na lista")));
    };
    target.click()?;
    pause(1500);
    Ok(json!({ "account": target.info.name }))
}

/// Vai para Amigos → aba `tab` ("Pendente", "Adicionar amigo").
fn friends_tab(w: &WindowInfo, tab: &str) -> Result<(), WinError> {
    let amigos = uia::find(w.id, "Amigos")?;
    if let Some(link) = amigos.into_iter().find(|f| matches!(f.info.kind.as_str(), "link" | "item de lista" | "aba" | "botão")) {
        link.click()?;
        pause(800);
    }
    let Some(t) = uia::find(w.id, tab)?.into_iter().find(|f| f.info.kind == "aba" || f.info.kind == "botão") else {
        return Err(WinError::NotFound(format!("não achei a aba '{tab}' em Amigos")));
    };
    t.click()?;
    pause(900);
    Ok(())
}

/// Pedidos pendentes: (recebidos, enviados), pelo nome da linha de cada botão.
fn pending_lists(w: &WindowInfo) -> Result<(Vec<Found>, Vec<Found>), WinError> {
    let all = uia::clickables(w.id)?;
    let (mut incoming, mut outgoing) = (Vec::new(), Vec::new());
    for f in all {
        let n = fold(&f.info.name);
        if n == "aceitar" {
            incoming.push(f);
        } else if n == "cancelar" || n.starts_with("cancelar pedido") {
            outgoing.push(f);
        }
    }
    Ok((incoming, outgoing))
}

/// `discord.pending`: lista os pedidos de amizade (recebidos e enviados).
pub fn pending(_params: &Value) -> Result<Value, WinError> {
    let w = open_discord()?;
    friends_tab(&w, "Pendente")?;
    let (incoming, outgoing) = pending_lists(&w)?;
    let names = |v: &[Found]| v.iter().map(|f| f.info.context.clone()).collect::<Vec<_>>();
    Ok(json!({ "received": names(&incoming), "sent": names(&outgoing) }))
}

/// `discord.friend`: `op` accept | decline | add, `name` (usuário).
pub fn friend(params: &Value) -> Result<Value, WinError> {
    let op = params["op"].as_str().unwrap_or("");
    let name = params["name"].as_str().unwrap_or("").trim().to_string();
    if name.is_empty() {
        return Err(WinError::InvalidParams("informe 'name' (nome de usuário)".into()));
    }
    let w = open_discord()?;
    match op {
        "accept" | "decline" => {
            friends_tab(&w, "Pendente")?;
            let want = fold(&name);
            let label = if op == "accept" { vec!["aceitar"] } else { vec!["ignorar", "recusar"] };
            let all = uia::clickables(w.id)?;
            let found = all.iter().find(|f| label.contains(&fold(&f.info.name).as_str()) && fold(&f.info.context).contains(&want));
            let Some(button) = found else {
                return Err(WinError::NotFound(format!("não achei um pedido pendente de '{name}'")));
            };
            button.click()?;
            pause(700);
            Ok(json!({ "op": op, "name": button.info.context }))
        }
        "add" => {
            friends_tab(&w, "Adicionar amigo")?;
            let field = uia::find(w.id, "nome de usuário")?
                .into_iter()
                .next()
                .ok_or_else(|| WinError::NotFound("não achei o campo de adicionar amigo".into()))?;
            let (x, y) = field.info.rect.center();
            crate::input::click_at(x, y)?;
            pause(250);
            crate::input::type_text(&name)?;
            pause(200);
            crate::input::press_enter()?;
            pause(1200);
            // Confirmação (sucesso ou erro) aparece como texto perto do campo.
            let ok = !uia::find(w.id, "sucesso")?.is_empty() || !uia::find(w.id, "enviado")?.is_empty();
            Ok(json!({ "op": "add", "name": name, "confirmed": ok }))
        }
        other => Err(WinError::InvalidParams(format!("op '{other}': use accept, decline ou add"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_come_from_button_names() {
        assert!(!mic_name_says_muted("Silenciar"));
        assert!(mic_name_says_muted("Desativar silenciamento"));
        assert!(!audio_name_says_off("Desativar áudio"));
        assert!(audio_name_says_off("Ativar áudio"));
        assert_eq!(wanted("toggle", true).unwrap(), false);
        assert_eq!(wanted("mute", false).unwrap(), true);
        assert!(wanted("xyz", false).is_err());
    }

    #[test]
    fn status_values_map_to_menu_labels() {
        assert_eq!(status_labels("dnd").unwrap()[0], "Não perturbe");
        assert_eq!(status_labels("Invisível").unwrap()[0], "Invisível");
        assert_eq!(status_labels("ausente").unwrap()[0], "Ausente");
        assert!(status_labels("dançando").is_none());
    }
}
