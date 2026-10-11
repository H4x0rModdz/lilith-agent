# 0005 — Versão 2: forma 3D (da cintura para cima), expressões e voz

## Contexto

A versão 1 entrega a orb 2D, o chat e o controle do Windows. A versão 2 dá corpo
e voz à Lilith, mantendo as regras do projeto: tudo local, gratuito e leve (PC
médio, sem depender de mais modelos grandes).

## Decisões

1. **Forma 3D com three.js + `@pixiv/three-vrm`** (ADR 0001). O modelo é VRM 0.x
   (VRoid) e é mostrado **da cintura para cima**: a câmera enquadra do quadril ao
   topo da cabeça e a base se dissolve com uma máscara, sem cortar seco. Pernas
   nem entram no enquadramento.
2. **A janela 3D é criada ao invocar e destruída ao voltar para 2D** (duplo clique
   na orb, ou "Forma 3D" no menu da bandeja). 30 fps; fora do corpo o clique
   atravessa a janela transparente.
3. **O modelo não vai no repositório nem no instalador.** A licença embutida no
   `.vrm` (VRoid Hub) proíbe redistribuição e modificação. Cada pessoa coloca o
   seu `lilith.vrm` em `%APPDATA%\Lilith\`.
4. **Expressões** vêm da emoção que o cérebro já devolve (`neutral`, `happy`,
   `concerned`) e do estado (`thinking`), com transição suave, mais piscar,
   respiração, olhar seguindo o cursor e inclinação da cabeça ao pensar.
5. **Voz de saída: Kokoro (82M, Apache-2.0) no sherpa-onnx**, voz feminina em
   português do Brasil (`pf_dora`). O Piper foi descartado porque todas as vozes
   pt-BR dele são masculinas. A biblioteca é carregada no processo (a linha de
   comando do sherpa quebra acentos no Windows); o modelo fica na memória enquanto
   se usa e sai depois de 5 minutos parado. O volume da fala abre a boca do modelo
   (`lilith://mouth`). Os arquivos (≈150 MB) são baixados no primeiro uso para
   `%APPDATA%\Lilith\voice\`, conferidos por SHA-256, e nunca vão no instalador.
   Voz de entrada (fase 2c): whisper.cpp, push-to-talk.

## Fases

| Fase | Entrega | Estado |
|---|---|---|
| 2a | Corpo 3D (cintura para cima), expressões, olhar, piscar | feita |
| 2b | Voz de saída (Kokoro, feminina pt-BR) + lip-sync | feita |
| 2c | Voz de entrada (whisper.cpp, push-to-talk) | a fazer |
| 2d | Gestos (acenar, apontar para a tela), troca de modelo nas Configurações | a fazer |

## Riscos

- O custo de GPU/CPU da forma 3D num PC fraco: por isso 30 fps, pixel ratio ≤ 1,5
  e janela destruída fora do uso.
- O modelo precisa de licença que permita o uso pretendido; vale checar antes de
  distribuir qualquer build que o inclua (hoje não inclui).
