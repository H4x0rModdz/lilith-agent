import { invoke } from "@tauri-apps/api/core";

/// `invoke` resiliente para a inicialização das janelas: se o backend ainda
/// não estiver pronto (ou o comando falhar), tenta de novo por alguns
/// instantes e, no fim, devolve `fallback` em vez de derrubar quem chamou —
/// uma janela que monta pela metade fica sem conversa e sem Enter.
export async function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: T): Promise<T> {
  const attempts = 15;
  for (let i = 0; i < attempts; i++) {
    try {
      return await invoke<T>(cmd, args);
    } catch (e) {
      if (i === attempts - 1) {
        console.warn(`[lilith] ${cmd} falhou; usando o padrão`, e);
        return fallback;
      }
      await new Promise((r) => setTimeout(r, 200));
    }
  }
  return fallback;
}
