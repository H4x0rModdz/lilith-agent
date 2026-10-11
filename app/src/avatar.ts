import { call } from "./ipc";

/// Avatar da Lilith (`%APPDATA%\Lilith\avatar.*`), carregado uma vez por janela.
/// `null` quando o usuário não escolheu um — a UI cai no sigilo padrão.
let cached: Promise<string | null> | null = null;
export function lilithAvatar(): Promise<string | null> {
  cached ??= call<string | null>("lilith_avatar", undefined, null);
  return cached;
}

/// Preenche um `.sigil` com a imagem da Lilith, se houver.
export async function fillSigil(el: HTMLElement) {
  const url = await lilithAvatar();
  if (!url) return;
  const img = document.createElement("img");
  img.src = url;
  img.alt = "";
  el.append(img);
  el.classList.add("has-image");
}

/// Silhueta anônima do usuário (SVG inline, sem rede).
export const USER_AVATAR_SVG = `
  <svg viewBox="0 0 32 32" aria-hidden="true">
    <circle cx="16" cy="12.5" r="5.5" />
    <path d="M5.5 28c1.4-5.6 5.6-8.5 10.5-8.5s9.1 2.9 10.5 8.5" />
  </svg>`;
