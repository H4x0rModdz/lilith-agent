import { listen } from "@tauri-apps/api/event";

/// Borda elétrica animada em volta de um card de vidro: um feixe de luz
/// rosa/lilás percorre a borda (conic-gradient girando) e um filtro SVG de
/// turbulência faz a linha "tremer" como eletricidade.
///
/// Custa GPU, e estas janelas ficam abertas o dia todo — então só anima
/// enquanto a janela tem foco ou a Lilith está pensando/agindo. Fora disso,
/// a borda fica parada (e o navegador não redesenha nada).
export function mountElectric(card: HTMLElement) {
  ensureFilter();
  const frame = document.createElement("div");
  frame.className = "electric";
  frame.setAttribute("aria-hidden", "true");
  frame.innerHTML = `<span class="electric-glow"></span><span class="electric-line"></span>`;
  card.prepend(frame);

  let focused = document.hasFocus();
  let busy = false;
  const update = () => card.classList.toggle("live", focused || busy);
  window.addEventListener("focus", () => ((focused = true), update()));
  window.addEventListener("blur", () => ((focused = false), update()));
  const thinking = new Set<string>();
  listen<{ conversation: string; state: string }>("lilith://state", (e) => {
    if (e.payload.state === "thinking") thinking.add(e.payload.conversation);
    else thinking.delete(e.payload.conversation);
    busy = thinking.size > 0;
    card.classList.toggle("busy", busy);
    update();
  });
  update();
}

/// Filtro SVG compartilhado (uma vez por documento). A semente da
/// turbulência anima via SMIL — o "tremido" da eletricidade.
function ensureFilter() {
  if (document.getElementById("electric-filter")) return;
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("width", "0");
  svg.setAttribute("height", "0");
  svg.style.position = "absolute";
  svg.innerHTML = `
    <filter id="electric-filter" x="-10%" y="-10%" width="120%" height="120%">
      <feTurbulence type="fractalNoise" baseFrequency="0.035" numOctaves="2" seed="1" result="noise">
        <animate attributeName="seed" values="1;2;3;4;5;6;7;8" dur="0.8s" repeatCount="indefinite" calcMode="discrete" />
      </feTurbulence>
      <feDisplacementMap in="SourceGraphic" in2="noise" scale="3.5" xChannelSelector="R" yChannelSelector="G" />
    </filter>`;
  document.body.append(svg);
}
