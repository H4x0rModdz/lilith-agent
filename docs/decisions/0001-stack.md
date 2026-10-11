# 0001 — Stack: Tauri 2 + Rust + three-vrm

**Status:** aceita (2026-10-11)

## Contexto

O requisito é ser **fluida e leve**. Ela fica sempre aberta no canto da tela, então o custo em repouso importa
mais que o pico.

## Opções

| Opção | RAM em repouso | VRM | Janelas transparentes | Reaproveita jarvis-os |
|---|---|---|---|---|
| **Tauri 2 + three-vrm** | ~40–80 MB (WebView2 já vem no Windows) | three-vrm é a lib VRM mais madura | sim | sim (Rust) |
| Electron | 150–300 MB (Chromium embutido) | three-vrm | sim | não |
| Qt6/QML | ~60 MB | Qt Quick 3D sem suporte VRM real (springbones, expressões) | sim | parcial (UI do jarvis) |
| Godot / Unity | 150 MB+ (engine) | addons OK | sim, com hacks | não |
| Bevy (Rust puro) | baixo | `bevy_vrm` imaturo | sim | sim |

## Decisão

**Tauri 2**, com backend em Rust e UI em TypeScript/HTML. A forma 3D usa **three.js + @pixiv/three-vrm**.

Para economizar recursos:
- A janela 3D só existe enquanto a forma 3D está ativa. Ao voltar pra 2D, a janela é **destruída**, liberando
  GPU e RAM, e não apenas escondida.
- O render 3D cai pra 15 fps quando ela está ociosa e volta pra 60 fps quando fala ou se mexe.
- A orb 2D é HTML/CSS puro, sem WebGL.
