//! Ajudante de desenvolvimento: erros de build lidos do Visual Studio, do VS
//! Code e de terminais (estruturados por código) e um resumo do git SÓ
//! LEITURA. Corrigir fica com um agente de código, pelo app (ver
//! `app/src-tauri/src/devhelp.rs`).

pub mod errors;
pub mod git;
pub mod project;
mod read;

use crate::window::{self, WindowInfo};
use crate::WinError;
use errors::{Diagnostic, Severity};
use project::DevApp;
use serde_json::{json, Value};

/// Quantos erros vão no resultado (a resposta mostra os primeiros).
const MAX_ERRORS_OUT: usize = 30;

/// Diagnósticos de uma janela já lida.
fn diagnostics_of(read: &read::DevRead) -> Vec<Diagnostic> {
    let mut all = errors::parse_rows(&read.rows);
    for text in &read.texts {
        all.extend(errors::parse_text(text));
    }
    errors::dedup(all)
}

/// Lê e estrutura os erros de uma janela de desenvolvimento.
fn errors_in(w: &WindowInfo, app: Option<DevApp>) -> Result<Value, WinError> {
    let read = read::read(w.id)?;
    let diags = diagnostics_of(&read);
    let errors: Vec<&Diagnostic> = diags.iter().filter(|d| d.severity == Severity::Error).collect();
    let warnings = diags.len() - errors.len();
    let mut out = json!({
        "window": w.title,
        "window_id": w.id,
        "app": w.app,
        "source": app.map(DevApp::label).unwrap_or("janela"),
        "error_count": errors.len(),
        "warning_count": warnings,
        "errors": errors.iter().take(MAX_ERRORS_OUT).collect::<Vec<_>>(),
    });
    if errors.is_empty() {
        out["note"] = json!(
            "só vejo o que está visível: se a Lista de Erros/Saída estiver fechada, minimizada ou rolada, os erros não aparecem"
        );
    }
    Ok(out)
}

/// `dev.errors {target?}`: erros de build da janela pedida ou, sem `target`,
/// da janela de desenvolvimento de cima que tiver erros.
pub fn read_errors(params: &Value) -> Result<Value, WinError> {
    let target = params["target"].as_str().map(str::trim).filter(|t| !t.is_empty());
    // "terminal", "devenv", "code": o tipo de programa, não um título.
    let kind = target.and_then(|t| match crate::paths::fold(t).as_str() {
        "terminal" => Some(DevApp::Terminal),
        "devenv" | "visual studio" | "vs" => Some(DevApp::VisualStudio),
        "code" | "vs code" | "vscode" => Some(DevApp::VsCode),
        _ => None,
    });
    if let Some(kind) = kind {
        let (w, app) = project::dev_windows()
            .into_iter()
            .find(|(w, a)| *a == kind && !w.minimized)
            .ok_or_else(|| WinError::Unavailable(format!("não vejo o {} aberto", kind.label())))?;
        return errors_in(&w, Some(app));
    }
    if let Some(target) = target {
        let w = window::find(target)?;
        let app = project::dev_app(&w.app);
        if app.is_none() && !target.starts_with("id:") {
            return Err(WinError::InvalidParams(format!(
                "'{}' não é um programa de desenvolvimento (Visual Studio, VS Code ou terminal)",
                w.app
            )));
        }
        return errors_in(&w, app);
    }
    let wins: Vec<_> = project::dev_windows().into_iter().filter(|(w, _)| !w.minimized).take(4).collect();
    if wins.is_empty() {
        return Err(WinError::Unavailable(
            "não vejo o Visual Studio, o VS Code nem um terminal aberto (abra a janela que mostra o build)".into(),
        ));
    }
    // A primeira com erros; se nenhuma tiver, o resultado (vazio) da de cima.
    let mut first = None;
    for (w, app) in wins {
        let Ok(found) = errors_in(&w, Some(app)) else { continue };
        if found["error_count"].as_u64().unwrap_or(0) > 0 {
            return Ok(found);
        }
        first.get_or_insert(found);
    }
    first.ok_or_else(|| WinError::Unavailable("não consegui ler as janelas de desenvolvimento".into()))
}

/// `dev.git_status {folder?, project?}`: resumo SÓ LEITURA do repositório.
/// Sem pasta: a do projeto aberto no VS/VS Code/terminal (a primeira, se
/// houver várias — o app pergunta antes qual usar).
pub async fn git_status(params: &Value) -> Result<Value, WinError> {
    let folder = match params["folder"].as_str().map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => crate::resolve_path(f),
        None => {
            let found = match params["project"].as_str().map(str::trim).filter(|p| !p.is_empty()) {
                Some(name) => project::candidates(&project::Hint::Folder(name.to_string())).await,
                None => project::detect(None).await.1,
            };
            found.into_iter().next().ok_or_else(|| {
                WinError::Unavailable("não achei o projeto (abra-o no Visual Studio/VS Code ou diga a pasta)".into())
            })?
        }
    };
    git::summary(&folder).await
}
