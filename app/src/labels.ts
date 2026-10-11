/// Rótulos amigáveis para a UI; ações sem rótulo mostram o nome técnico.
const ACTION_LABELS: Record<string, string> = {
  "app.open": "Abrindo app",
  "app.close": "Fechando app",
  "browser.open": "Abrindo no navegador",
  "file.open": "Abrindo arquivo",
  "file.find": "Procurando arquivos",
  "file.list": "Olhando a pasta",
  "file.move": "Movendo arquivo",
  "file.copy": "Copiando arquivo",
  "file.delete": "Mandando pra Lixeira",
  "download.file": "Baixando",
  "desktop.set_wallpaper": "Trocando o wallpaper",
  "audio.set_volume": "Ajustando o volume",
  "audio.get_volume": "Checando o volume",
  "audio.set_mute": "Mudo",
  "clipboard.set": "Copiando",
  "clipboard.get": "Lendo a área de transferência",
  "system.power": "Energia",
  "web.search": "Pesquisando na web",
  "web.fetch": "Lendo a página",
  "agent.delegate": "Delegando para um agente",
  "agent.jobs": "Checando as tarefas",
  kill_switch: "Kill switch",
};

export function actionLabel(action: string): string {
  return ACTION_LABELS[action] ?? action;
}
