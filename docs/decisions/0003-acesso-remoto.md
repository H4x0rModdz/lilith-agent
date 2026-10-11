# 0003 — Acesso remoto: Discord + streaming de tela próprio sobre Tailscale

**Status:** proposta (2026-10-11)

## Contexto

O usuário quer falar com a Lilith pelo celular, pedir coisas e **ver a tela ao vivo**. Bots do Discord não
transmitem vídeo.

## Opções avaliadas

| | Sunshine + Moonlight | Programa próprio |
|---|---|---|
| Latência e qualidade | Excelente (nível jogo, encoder de hardware) | Boa (mesmo encoder de hardware, via WebRTC) |
| No celular | Instalar o app Moonlight e parear | Só abrir uma página no navegador (PWA) |
| Integração com a Lilith | Nenhuma: é só um espelho da tela | Total: chat, aprovações e cursor rosa na mesma tela |
| Controle remoto | Completo (mouse, teclado, gamepad) | O que a gente implementar |
| Esforço | Quase zero | Médio |
| Licença | GPL-3 | nossa |

**Tailscale** não concorre com essas duas opções, é a rede por baixo. Cria uma VPN privada entre o celular e o PC,
grátis pra uso pessoal, sem abrir portas no roteador. Vale usar com qualquer uma das duas.

## Decisão proposta

1. **Rede:** Tailscale.
2. **Tela ao vivo:** um **streamer próprio** embutido na Lilith, em duas etapas:
   - **v0:** Windows.Graphics.Capture → JPEG das regiões alteradas → WebSocket → `<canvas>` na PWA. É simples e
     funciona em qualquer navegador; usa mais banda.
   - **v1:** Windows.Graphics.Capture → H.264 em hardware (Media Foundation: NVENC, QuickSync ou AMF) → **WebRTC**
     (`webrtc-rs`) → `<video>`. Fluido e leve.
3. **Conversa:** o bot do Discord continua para texto, comandos e aprovações rápidas. A PWA tem o mesmo chat
   e voz por WebRTC.
4. **Sunshine fica como integração opcional:** a Lilith pode iniciar e parar o Sunshine quando o usuário quiser
   o controle completo, por exemplo pra jogar.

## Permissões

O canal remoto usa a mesma política do local. Cada scope é configurável por canal (`allow` / `ask` / `deny`),
e o usuário decide. As aprovações pedidas de fora aparecem no próprio celular.
