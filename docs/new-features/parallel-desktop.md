# Feature futura — Modo área de trabalho paralela

**Status:** registrada, fora do escopo das fases 1–4.

## Ideia

Hoje, o cursor rosa age via UI Automation e, como fallback, "pega emprestado" o mouse real por alguns
milissegundos (ver [decisão 0002](../decisions/0002-percepcao-local.md)). O Windows tem **um único cursor de
sistema**, então essa abordagem não garante independência total.

Neste modo, a Lilith trabalha num **ambiente separado**, com mouse e teclado próprios. O usuário acompanha tudo
por uma janela de visualização dentro do PC, que pode ser a mesma do streaming remoto.

## Caminhos possíveis

| Caminho | Prós | Contras |
|---|---|---|
| **Windows Sandbox** (Pro/Enterprise) | Nativo, descartável, isolado | Não vê seus apps/arquivos; reinicia limpo |
| **VM Hyper-V** com perfil persistente | Independência total, snapshots | RAM e disco; precisa instalar apps lá |
| **Sessão RDP local extra** (outro usuário Windows) | Apps reais, cursor independente | Exige licença multi-sessão ou patch; pesado |
| **Desktop oculto** (`CreateDesktop`) | Leve, mesma sessão | Apps de GPU/DirectComposition não renderizam bem; captura limitada |

## Pré-requisitos

- Streaming de tela (fase 4), reutilizado como "janela" para o ambiente paralelo.
- Guardrails por canal: o ambiente paralelo vira um canal próprio, com política própria.
