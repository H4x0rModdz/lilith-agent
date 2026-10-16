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
6. **Voz opcional "Serena" (GPU NVIDIA): Qwen3-TTS CustomVoice 1.7B** (Apache-2.0).
   Escolhida de ouvido entre as vozes femininas prontas do modelo (clonar o `mp3`
   de referência, com o Qwen Base, Kokoro+OpenVoice e outros, soou pior). Roda como
   processo Python (`voice/qwen_server.py`, protocolo de linhas JSON) gerenciado
   pelo app, com ambiente instalado sob demanda em `%APPDATA%\Lilith\voice\qwen\`
   (uv + PyTorch/CUDA + modelo, ≈8 GB). Custo medido numa RTX 5070: ≈3,9 GB de
   VRAM carregado, geração mais lenta que o tempo real (RTF ≈ 1,4), por isso a
   fala é gerada em pedaços maiores e, enquanto o modelo carrega, a primeira
   resposta sai com o Kokoro. Descarrega da GPU depois de 20 minutos parada. O
   Kokoro segue como padrão e plano B para qualquer PC.
7. **Voz de entrada (fase 2c): Whisper small int8 no mesmo sherpa-onnx**, com o
   idioma fixo em português. Sem whisper.cpp: o reconhecedor offline do
   sherpa-onnx já vem na biblioteca que a voz de saída usa. Comparado com o
   Kokoro falando frases conhecidas, o NeMo FastConformer pt (≈107 MB) errava a
   grafia de nomes de apps ("Spotífer", "discorde") e o Parakeet v3 (≈490 MB)
   virava inglês em frases curtas ("Oi, Lilith!" → "Oily leads"); o Whisper
   small acertou os dois (WER médio ≈4 %, ≈0,2 s por segundo de fala na CPU).
   Os três arquivos int8 (≈375 MB) são baixados no primeiro uso para
   `%APPDATA%\Lilith\voice\asr\` (SHA-256), carregados ao apertar o atalho e
   liberados depois de 3 minutos parado.
   **Push-to-talk** (padrão Ctrl+Alt+Espaço, configurável): segurar e soltar
   envia; um toque rápido ouve até uma pausa; Esc cancela. Também há o botão do
   microfone no chat. O microfone (WASAPI, 16 kHz mono) só abre durante o uso;
   o silêncio das pontas é cortado por energia; o texto vai para o chat como se
   tivesse sido digitado (responde também cartões de escolha). Nada sai do PC.

## Fases

| Fase | Entrega | Estado |
|---|---|---|
| 2a | Corpo 3D (cintura para cima), expressões, olhar, piscar | feita |
| 2b | Voz de saída (Kokoro, feminina pt-BR) + lip-sync | feita |
| 2c | Voz de entrada (Whisper no sherpa-onnx, push-to-talk) | feita |
| 2d | Gestos (acenar, apontar para a tela), troca de modelo nas Configurações | a fazer |

## Riscos

- O custo de GPU/CPU da forma 3D num PC fraco: por isso 30 fps, pixel ratio ≤ 1,5
  e janela destruída fora do uso.
- O modelo precisa de licença que permita o uso pretendido; vale checar antes de
  distribuir qualquer build que o inclua (hoje não inclui).
