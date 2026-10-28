# Vigia de trabalho (Outlook + Teams → tarefa → Claude/Codex)

Ideia registrada em 2026-10-27; vem depois dos relatórios de bug.

## Fluxo

1. **Gatilho (código):** rotina agendada ("a cada 30 min, 9h–18h, dias úteis").
2. **Onde ler (código):** sempre o app **instalado** primeiro (Outlook/Teams
   desktop — o Teams já é lido/usado por UIA para mandar mensagens); só se não
   houver, o Outlook Web / Teams Web no navegador padrão (`browsers.rs`).
3. **Leitura (código):** lista de mensagens pela UIA — remetente, assunto,
   prévia, não lido. Sem screenshot, sem IA.
4. **Filtro (código):** domínio da empresa, lista VIP, menção ao nome do
   usuário → candidato; `noreply`, newsletter, domínio externo → descarta.
5. **Interpretação (Ollama, só o que sobrou):** JSON — é tarefa para mim? qual?
   prazo? projeto?
6. **Prompt (código + IA):** `delegate::compose_task` com as regras fixas; o
   projeto (pasta) vem de uma tabela do usuário, ou de um cartão de escolha.
7. **Engatilhado:** cartão "Tarefa do Fulano: … [Fazer com Claude] [Editar
   prompt] [Ignorar]". Nada roda sem o clique. Agente padrão configurável.
8. **Revisão:** ao terminar, o código checa diff/build/testes; o *outro* agente
   revisa o diff contra o pedido original (Codex revisa Claude e vice-versa).

## Cuidados

- E-mail é texto de terceiros: entra no prompt só como citação, nunca como
  instrução (prompt injection). O portão de contaminação do despachante vale.
- Triagem 100% local; o conteúdo só vai para Claude/Codex (nuvem) depois da
  aprovação — conferir a política da empresa.
