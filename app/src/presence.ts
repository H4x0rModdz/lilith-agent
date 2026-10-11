import { listen } from "@tauri-apps/api/event";

/// Cursor da Lilith (janela overlay que ignora cliques; quem move é o Rust).
export function mountCursor(root: HTMLElement) {
  root.innerHTML = `
    <div class="lilith-cursor" data-state="idle">
      <svg viewBox="0 0 26 30" aria-hidden="true">
        <path d="M6 4 L6 24 L11 19.5 L14.5 27 L18 25.5 L14.6 18.2 L21 18 Z"
              fill="#e8458f" stroke="#fff" stroke-width="2" stroke-linejoin="round" />
      </svg>
      <span class="tag">Lilith</span>
    </div>`;
  const el = root.querySelector<HTMLElement>(".lilith-cursor")!;
  listen<string>("lilith://cursor", (e) => {
    el.dataset.state = e.payload;
    if (e.payload === "release" || e.payload === "point") {
      window.setTimeout(() => (el.dataset.state = "idle"), 700);
    }
  });
}

/// Borda de um monitor enquanto a Lilith mexe na tela.
export function mountBorder(root: HTMLElement) {
  root.innerHTML = `
    <div class="screen-border">
      <span class="notice"><b>Lilith</b> está usando a tela · Ctrl+Alt+Esc para parar</span>
    </div>`;
}
