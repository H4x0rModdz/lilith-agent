import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { hydrateIcons, icon } from "./icons";

interface DocsProgress {
  running: boolean;
  done: number;
  total: number;
  semantic: boolean;
  failed: number;
}

interface DocsStatus {
  folders: string[];
  stats: { files: number; chunks: number; embedded_chunks: number };
  model: string;
  model_installed: boolean;
  ollama_ok: boolean;
  pulling: boolean;
  progress: DocsProgress;
}

/// Seção "Documentos" das Configurações: as pastas que entram na busca nos
/// documentos, o andamento da indexação e o modelo de embeddings (opcional).
export function mountDocsSettings(form: HTMLElement) {
  const card = document.createElement("section");
  card.className = "settings-card";
  card.innerHTML = `
    <h3><span data-icon="file" data-size="16"></span>Documentos</h3>
    <small class="hint">Pastas onde ela procura quando você pergunta “qual documento fala de…”. Lê txt, md, pdf e docx, tudo aqui no seu PC.</small>
    <ul class="facts docs-folders"></ul>
    <div class="fact-add">
      <input class="docs-folder-new" maxlength="300" placeholder="ex.: Documentos, ou C:\\Projetos\\notas" />
      <button type="button" class="icon-btn neon-soft docs-add-btn" title="Adicionar pasta"><span data-icon="plus" data-size="15"></span></button>
    </div>
    <div class="docs-progress" hidden><span></span></div>
    <div class="update-row">
      <small class="hint docs-status"></small>
      <button type="button" class="neon-soft update-btn docs-reindex">Atualizar agora</button>
    </div>
    <div class="update-row">
      <small class="hint docs-model"></small>
      <button type="button" class="neon-soft update-btn docs-pull" hidden>Baixar modelo (≈270 MB)</button>
    </div>`;
  form.append(card);
  hydrateIcons(card);

  const list = card.querySelector<HTMLUListElement>(".docs-folders")!;
  const input = card.querySelector<HTMLInputElement>(".docs-folder-new")!;
  const statusEl = card.querySelector<HTMLElement>(".docs-status")!;
  const modelEl = card.querySelector<HTMLElement>(".docs-model")!;
  const pullBtn = card.querySelector<HTMLButtonElement>(".docs-pull")!;
  const reindexBtn = card.querySelector<HTMLButtonElement>(".docs-reindex")!;
  const bar = card.querySelector<HTMLElement>(".docs-progress")!;
  let pullText = "";

  function renderFolders(folders: string[]) {
    if (folders.length === 0) {
      const li = document.createElement("li");
      li.className = "facts-empty";
      li.textContent = "Nenhuma pasta ainda. Comece pela de Documentos.";
      list.replaceChildren(li);
      return;
    }
    list.replaceChildren(
      ...folders.map((f) => {
        const li = document.createElement("li");
        li.className = "fact";
        const text = document.createElement("div");
        text.className = "fact-text";
        const span = document.createElement("span");
        span.textContent = f;
        span.style.color = "var(--text)";
        text.append(span);
        const del = document.createElement("button");
        del.type = "button";
        del.className = "icon-btn neon-soft";
        del.title = "Tirar da busca";
        del.append(icon("trash", 14));
        del.addEventListener("click", async () => {
          await invoke("docs_remove_folder", { folder: f });
          void load();
        });
        li.append(text, del);
        return li;
      }),
    );
  }

  function renderProgress(p: DocsProgress) {
    bar.hidden = !p.running;
    const pct = p.total > 0 ? Math.round((p.done / p.total) * 100) : 0;
    bar.querySelector<HTMLElement>("span")!.style.width = `${pct}%`;
    reindexBtn.disabled = p.running;
    if (p.running) statusEl.textContent = `Lendo documentos… ${p.done} de ${p.total}`;
  }

  async function load() {
    let st: DocsStatus;
    try {
      st = await invoke<DocsStatus>("docs_status");
    } catch {
      return;
    }
    renderFolders(st.folders);
    const n = st.stats.files;
    const mode = st.stats.embedded_chunks > 0 ? "busca pelo sentido" : "busca por palavras";
    statusEl.textContent = n === 0 ? "Nada indexado ainda." : `${n} documento${n === 1 ? "" : "s"} no índice · ${mode}.`;
    renderProgress(st.progress);
    if (!st.ollama_ok) {
      modelEl.textContent = "Ollama fora do ar: a busca segue por palavras até ele voltar.";
      pullBtn.hidden = true;
    } else if (st.model_installed) {
      modelEl.textContent = `Modelo de embeddings: ${st.model} ✓`;
      pullBtn.hidden = true;
    } else {
      modelEl.textContent = pullText || `Para achar pelo sentido (não só pela palavra exata), baixe o modelo ${st.model}.`;
      pullBtn.hidden = false;
      pullBtn.disabled = st.pulling;
    }
  }

  async function addFolder() {
    const folder = input.value.trim();
    if (!folder) return;
    try {
      await invoke<string>("docs_add_folder", { folder });
      input.value = "";
    } catch (e) {
      statusEl.textContent = String(e);
      return;
    }
    void load();
  }

  card.querySelector(".docs-add-btn")!.addEventListener("click", () => void addFolder());
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void addFolder();
    }
  });
  reindexBtn.addEventListener("click", async () => {
    await invoke("docs_reindex");
    void load();
  });
  pullBtn.addEventListener("click", async () => {
    pullBtn.disabled = true;
    pullText = "Baixando o modelo…";
    modelEl.textContent = pullText;
    await invoke("docs_pull_model");
  });

  listen<DocsProgress>("lilith://docs-index", (e) => {
    renderProgress(e.payload);
    if (!e.payload.running) void load();
  });
  listen<{ completed?: number; total?: number; status?: string; finished?: boolean; error?: string | null }>(
    "lilith://docs-model",
    (e) => {
      const p = e.payload;
      if (p.finished) {
        pullText = p.error ? `Não deu para baixar: ${p.error}` : "";
        void load();
        return;
      }
      const pct = p.total ? ` ${Math.round(((p.completed ?? 0) / p.total) * 100)}%` : "";
      pullText = `Baixando o modelo…${pct}`;
      modelEl.textContent = pullText;
    },
  );

  void load();
}
