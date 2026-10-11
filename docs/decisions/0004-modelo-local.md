# 0004 — Modelo local padrão: `qwen3.5:4b` com raciocínio desligado

**Status:** aceita (2026-10-11)

## Medições

RTX 5070 12 GB, `num_ctx` 8192 e `think: false`, com a mesma bateria de comandos:

| Modelo | VRAM | Saudação | Abrir app | Cadeia de 2 tools | Observação |
|---|---|---|---|---|---|
| `qwen3:4b` | 3,9 GB | 8,8 s | 3,8 s | 43,9 s | Ignora `think: false` (tag aponta para a variante só-raciocínio) e vaza `</think>` |
| `qwen3.5:4b` | 3,3 GB | 0,6 s* | 0,4 s | 0,8 s | Respeita `think: false`; tem **tools + visão** |

\* O primeiro turno leva cerca de 20 s, que é o tempo de carregar o modelo na GPU.

## Decisões

- Padrão: `qwen3.5:4b`. Pode ser trocado em `%APPDATA%\Lilith\config.json` (`ollama.model`) ou pela env `LILITH_MODEL`.
- `num_ctx` fixo em 8192. O servidor estava com `OLLAMA_CONTEXT_LENGTH=131072`, o que levava um modelo de 4B
  a ocupar 23 GB, com 56% rodando na CPU.
- `OLLAMA_HOST` no formato de bind (`0.0.0.0:11434`) é normalizado para uma URL de cliente.
- Qualquer `<think>` que vazar na resposta é removido.
- O mesmo modelo cobre a camada T3 de percepção ([0002](0002-percepcao-local.md)), sem precisar de um VLM extra.
