# Arquitetura — Lilith Companion (Windows)

Lilith é uma companion de IA que vive no canto do Windows. Tem duas formas:

1. **Forma 2D** — ícone pequeno no canto da tela. Clique abre o chat; duplo clique invoca a forma 3D.
2. **Forma 3D** — avatar VRM com expressões, voz e lip-sync.

Ela enxerga a tela, usa o próprio cursor rosa e executa ações no sistema, sempre passando pelos guardrails.
Tudo roda **local e de graça**: LLM via Ollama, sem API paga.

## Visão geral

```
┌──────────── app (Tauri 2: Rust + WebView2) ─────────────────────────┐
│ Janelas: orb 2D (canto) ─duplo clique─► avatar 3D (three-vrm)       │
│          overlay transparente click-through ► cursor rosa + HUD     │
│          diálogo de aprovação (guardrails)                          │
├─────────────────────────────────────────────────────────────────────┤
│ lilith-brain   LLM (Ollama) + catálogo de tools + loop de passos    │
│                + memória + emoção          (portado do jarvis-os)   │
│ lilith-guard   scopes, política por canal, aprovações, auditoria    │
├─────────────────────────────────────────────────────────────────────┤
│ lilith-win     mãos e olhos no Windows:                             │
│   • Shell/Win32: apps, arquivos (lixeira), wallpaper, downloads     │
│   • UI Automation: lê a árvore de UI e age sem mexer no mouse       │
│   • Windows.Graphics.Capture + Windows.Media.Ocr: visão barata      │
│   • SendInput: fallback quando o app não expõe acessibilidade       │
├─────────────────────────────────────────────────────────────────────┤
│ remote (fase 4) Discord bot + streaming de tela próprio (Tailscale) │
└─────────────────────────────────────────────────────────────────────┘
```

## Crates

| Crate | Papel | Origem |
|---|---|---|
| `crates/lilith-brain` | Cliente Ollama (stream + tool calling), loop multi-step, emoção, memória de sessão | `jarvis-os/ai/lilith` |
| `crates/lilith-guard` | Classificação de scopes, política por canal (local / discord / …), log de auditoria | `jarvis-os/system/permission` + `action-bus/permission.rs` |
| `crates/lilith-win` | Handlers das ações no Windows | novo (web/wallpaper portados de `jarvis-os/system/action-bus/handlers`) |
| `app/src-tauri` | Processo principal: janelas, tray, ponte UI ↔ brain, fila de aprovação | novo |

## Fluxo de um comando

```
texto/voz ─► brain.process()
              ├─ LLM decide tool ─► dispatcher ─► guard.check(canal, scope)
              │                                    ├─ allow ─► lilith-win handler
              │                                    ├─ ask   ─► diálogo de aprovação (UI ou celular)
              │                                    └─ deny
              └─ resultado volta pro LLM (até 8 passos) ─► resposta + emoção
```

Toda ação é registrada em `%APPDATA%\Lilith\logs\audit.jsonl`.

## Guardrails

- Cada ação mapeia para um **scope** (`app.launch`, `filesystem.delete`, `input.control`, …).
- Scopes são classificados em **seguro** (auto) ou **perigoso** (pede aprovação). Scope desconhecido é negado.
- A política é **por canal** e **o usuário decide**: o padrão do canal `discord` é igual ao local, mas cada scope
  pode ser trocado para `allow` / `ask` / `deny` por canal em `%APPDATA%\Lilith\permissions.json`.
- Aprovação: *Negar / Permitir uma vez / Permitir sempre*. Sem resposta em 30 s → negado.
- **Kill switch**: `Ctrl+Alt+Esc` cancela a ação em curso e congela o cursor rosa.

## Rotinas

- **Modos** (`mode.*`): sequências de chamadas do catálogo salvas no SQLite. Cada passo é decidido pelo código
  (`lilith_brain::routines::parse_step`/`validate_step`) e mostrado numa prévia antes de salvar; ao rodar, cada passo
  passa de novo pelo dispatcher guardado (aprovações valem).
- **Layouts** (`layout.*`): fotografia das janelas (`lilith_win::layout`, cálculo puro); restaurar abre o que falta via
  `app.open` e arrasta as janelas com o cursor da Lilith.
- **Lembretes repetidos**: regra (`daily`, `weekdays`, `weekly:1,3`, `monthly:15`) decidida em `reminders.rs`; o agendador
  cria uma ocorrência por aviso e reagenda.
- **Avisos proativos** (`alerts.rs`) e **sugestão de modos** pela rotina (`habits.rs`), com chaves nas Configurações.

## Fases

1. **MVP** — orb 2D + chat, brain portado, tools básicas do Windows, guardrails. ← *atual*
2. **Forma 3D** — VRM (three-vrm), expressões, voz (Kokoro para falar, Whisper para ouvir, ambos no sherpa-onnx; push-to-talk), lip-sync.
3. **Olhos e mãos** — percepção em camadas ([decisão 0002](decisions/0002-percepcao-local.md)), overlay e cursor rosa.
4. **Remoto** — Discord + streaming de tela ([decisão 0003](decisions/0003-acesso-remoto.md)).
5. **Extras** — ver [`new-features/`](new-features/).
