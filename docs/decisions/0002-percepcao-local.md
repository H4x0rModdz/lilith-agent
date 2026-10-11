# 0002 — Percepção da tela: local, em camadas e orientada a eventos

**Status:** aceita (2026-10-11)

## Contexto

Ela precisa "ver a tela em tempo real" sem custo financeiro (nada de API de visão paga) e sem pesar a máquina.
Mandar vídeo contínuo pra um modelo de visão local travaria o PC, mesmo o `qwen3.5:4b` (que tem visão nativa).

## Decisão

"Tempo real" vem de um **fluxo de eventos**, não de vídeo pro LLM. Cada camada só roda quando a anterior não basta:

| Camada | Fonte | Custo | Quando |
|---|---|---|---|
| **T0** | `SetWinEventHook` (janela em foco, título, criar/fechar) + eventos de foco da UI Automation | ~0 | sempre ligado |
| **T1** | Árvore **UI Automation** da janela em foco: botões, campos, textos e posições, serializada compacta pro LLM | ms | quando ela precisa agir ou o usuário pergunta "o que tem na tela" |
| **T2** | **Windows.Graphics.Capture** só das regiões alteradas (dirty rects) + **Windows.Media.Ocr** (nativo, grátis, pt-BR) | ~50–150 ms por região | apps sem acessibilidade (jogos, canvas, alguns Electron) |
| **T3** | O próprio `qwen3.5:4b` (visão nativa, sem modelo extra) com frame reduzido | ~1–3 s, GPU | só sob demanda explícita ("o que é essa imagem?") |

Para agir na tela:
- **Primeiro, UI Automation** (`Invoke`, `SetValue`, `Scroll`, `SelectionItem`). Não toca no mouse do usuário, e
  o cursor rosa é animado no overlay até o alvo, só como visual.
- **Fallback: `SendInput`**. Salva a posição do mouse do usuário, clica e devolve a posição em menos de 50 ms.
  Antes disso, aguarda o usuário ficar ~300 ms sem mexer o mouse, pra evitar briga pelo cursor.

## Consequências

- Em repouso, o custo é praticamente zero: só os hooks ficam ativos.
- A cobertura é boa em apps Win32, WinUI, WPF, Office e navegadores. Jogos e canvas dependem de OCR (T2).
- Independência total entre os cursores (ela e você clicando ao mesmo tempo em qualquer app) fica para o
  [modo área de trabalho paralela](../new-features/parallel-desktop.md).
