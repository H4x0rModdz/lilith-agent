/// Prévia de uma ação com muitos arquivos (organizar uma pasta, limpar
/// duplicados, desfazer): o backend calcula o plano e manda um resumo pronto;
/// aqui só desenhamos — título, resumo, grupos com alguns nomes e as notas.

export interface PreviewGroup {
  label: string;
  items: (string | null)[];
  more?: number;
}

export interface Preview {
  kind: string;
  title: string;
  summary: string;
  groups: PreviewGroup[];
  notes?: string[];
  /// Texto do botão de confirmar ("Organizar", "Mandar para a Lixeira"…).
  confirm?: string;
  /// Mantém o botão "Sempre" (ex.: ler o que foi copiado); mexer em arquivos
  /// é conferido a cada vez.
  allow_always?: boolean;
}

export function renderPreview(p: Preview): HTMLElement {
  const box = document.createElement("div");
  box.className = "preview";
  const summary = document.createElement("p");
  summary.className = "preview-summary";
  summary.textContent = p.summary;
  box.append(summary);

  for (const g of p.groups) {
    const group = document.createElement("details");
    group.className = "preview-group";
    group.open = p.groups.length <= 3;
    const head = document.createElement("summary");
    head.textContent = g.label;
    group.append(head);
    const list = document.createElement("ul");
    for (const item of g.items) {
      if (!item) continue;
      const li = document.createElement("li");
      li.textContent = item;
      list.append(li);
    }
    if (g.more && g.more > 0) {
      const li = document.createElement("li");
      li.className = "preview-more";
      li.textContent = `… e mais ${g.more}`;
      list.append(li);
    }
    group.append(list);
    box.append(group);
  }

  for (const n of p.notes ?? []) {
    const note = document.createElement("p");
    note.className = "preview-note";
    note.textContent = n;
    box.append(note);
  }
  return box;
}
