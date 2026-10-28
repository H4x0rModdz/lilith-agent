//! `service.control`: parar/iniciar/reiniciar um serviço do Windows pelo nome
//! falado ("para o serviço do docker"). O código acha o serviço; se o nome
//! casar com vários, um cartão pergunta qual; quem executa é o ajudante
//! elevado (o UAC do Windows dá o administrador).

use crate::dispatcher::GuardedDispatcher;
use lilith_win::services::{self, ServiceOp};
use lilith_win::WinError;
use serde_json::{json, Value};

impl GuardedDispatcher {
    pub(crate) async fn service_control(&self, params: &Value) -> Result<Value, WinError> {
        let query = params["service"].as_str().unwrap_or("").trim().to_string();
        let op = params["op"]
            .as_str()
            .and_then(ServiceOp::parse)
            .ok_or_else(|| WinError::InvalidParams("op deve ser stop, start ou restart".into()))?;
        let mut found = services::matching(&services::list().await?, &query);
        let service = match found.len() {
            0 => return Err(WinError::NotFound(format!("serviço '{query}'"))),
            1 => found.remove(0),
            _ => {
                let options: Vec<String> = found.iter().take(8).map(|s| format!("{} ({})", s.display, s.name)).collect();
                let title = format!("Achei {} serviços com \"{query}\". Qual deles?", found.len());
                let chosen = self
                    .choices
                    .ask(&self.app, &self.conversation, &title, options.clone())
                    .await
                    .ok_or_else(|| WinError::Failed("nenhum serviço escolhido".into()))?;
                let i = options.iter().position(|o| *o == chosen).unwrap_or(0);
                found.swap_remove(i)
            }
        };
        let already = match op {
            ServiceOp::Stop => service.state == "stopped",
            ServiceOp::Start => service.state == "running",
            ServiceOp::Restart => false,
        };
        if !already {
            crate::elevated::run(vec![crate::elevated::Op::Service { name: service.name.clone(), action: op }]).await?;
        }
        let state = match op {
            ServiceOp::Stop => "stopped",
            ServiceOp::Start | ServiceOp::Restart => "running",
        };
        Ok(json!({ "service": service.display, "name": service.name, "state": state, "already": already }))
    }
}
