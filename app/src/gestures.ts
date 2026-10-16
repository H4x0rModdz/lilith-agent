/// Gestos da forma 3D (fase 2d): animação procedural leve, só com rotações
/// somadas aos ossos normalizados do VRM. Este módulo é lógica pura (sem
/// three.js) para poder ser testado: ele só diz, a cada quadro, quanto somar em
/// cada osso. Quem aplica (e corrige o espelhamento do VRM 0.x) é o avatar3d.
///
/// Regras:
/// - O CÓDIGO decide quando gesticular (eventos/estados), nunca o modelo de IA.
/// - Gestos entram e saem com easing e se somam (camada) à respiração e ao olhar
///   seguindo o cursor, sem brigar com eles.
/// - Um gesto novo de prioridade maior ou igual cancela o atual com uma saída
///   suave (crossfade); um de prioridade menor espera numa fila curta.
/// - Poses mantidas (pensando, ouvindo) ficam numa camada de baixo, que se
///   recolhe enquanto um gesto de uma vez (aceno, apontar…) está tocando.
///
/// Convenção dos números: radianos, no sistema do VRM 1.0 (modelo de frente
/// para a câmera, +X = esquerda dela = DIREITA da tela). Para o VRM 0.x o avatar3d
/// inverte X e Z (variável `flip`). Gestos de braço são escritos para o braço
/// DIREITO dela (lado esquerdo da tela) e espelhados para o esquerdo.

export type Vec3 = [number, number, number];

/// Lado da TELA (de quem olha), não da Lilith.
export type Side = "left" | "right";

export type Bone =
  | "hips"
  | "spine"
  | "chest"
  | "neck"
  | "head"
  | "leftUpperArm"
  | "leftLowerArm"
  | "leftHand"
  | "rightUpperArm"
  | "rightLowerArm"
  | "rightHand"
  | "leftMiddleProximal"
  | "leftMiddleIntermediate"
  | "leftRingProximal"
  | "leftRingIntermediate"
  | "leftLittleProximal"
  | "leftLittleIntermediate"
  | "rightMiddleProximal"
  | "rightMiddleIntermediate"
  | "rightRingProximal"
  | "rightRingIntermediate"
  | "rightLittleProximal"
  | "rightLittleIntermediate";

/// Deslocamento a somar no quadro: rotações por osso e `lift` (metros, sobe o quadril).
export interface Pose {
  rot: Partial<Record<Bone, Vec3>>;
  lift: number;
}

export type GestureName = "wave" | "nod" | "shake" | "point" | "bounce" | "think" | "listen";

/// Todos os números ajustáveis num lugar só. Ângulos em radianos (0,1 ≈ 6°),
/// tempos em segundos. Valores conservadores: dá para aumentar aos poucos.
export const TUNING = {
  wave: {
    duration: 2.0,
    easeIn: 0.4,
    easeOut: 0.5,
    /// Braço sobe para o lado e um pouco para a frente.
    upperArm: [-0.15, 0.55, -0.85] as Vec3,
    /// Antebraço dobra para cima (z negativo = sobe).
    lowerArm: [0, 0.3, -1.55] as Vec3,
    /// Gira a mão para a palma ficar de frente (x) — se a palma virar para ela, troque o sinal.
    hand: [-0.9, 0, 0] as Vec3,
    /// Balanço do aceno: ciclos por segundo e amplitude no antebraço e na mão.
    freq: 1.7,
    swing: 0.28,
    handSwing: 0.18,
    headTilt: 0.06,
  },
  nod: {
    duration: 0.9,
    easeIn: 0.12,
    easeOut: 0.25,
    /// Quantos "sins" cabem na duração e a profundidade (cabeça, pescoço).
    count: 2,
    head: 0.14,
    neck: 0.05,
  },
  shake: {
    duration: 1.0,
    easeIn: 0.15,
    easeOut: 0.3,
    freq: 2.2,
    head: 0.13,
    neck: 0.05,
    /// Cabeça um pouco baixa enquanto nega (desconsolada).
    down: 0.04,
  },
  point: {
    duration: 1.7,
    easeIn: 0.3,
    easeOut: 0.45,
    upperArm: [-0.2, 0.55, -0.3] as Vec3,
    lowerArm: [0, 0.5, -0.45] as Vec3,
    hand: [0, 0, -0.15] as Vec3,
    /// Fecha médio, anelar e mindinho (o indicador fica esticado).
    curl: 1.1,
    /// Vira a cabeça para o lado apontado.
    headYaw: 0.22,
  },
  bounce: {
    duration: 0.9,
    easeIn: 0.12,
    easeOut: 0.25,
    hops: 2,
    /// Altura do pulinho (metros).
    lift: 0.012,
    /// Braços abrem um pouco (comemorando).
    arms: 0.12,
    chest: -0.03,
    headTilt: 0.05,
  },
  think: {
    easeIn: 0.6,
    easeOut: 0.5,
    /// Mão perto do queixo: braço para a frente, cotovelo bem dobrado.
    upperArm: [-0.6, 0.35, 0.2] as Vec3,
    lowerArm: [0, 2.1, 0] as Vec3,
    hand: [0, 0, 0.2] as Vec3,
    /// A inclinação de lado ao pensar já vem do avatar3d (tiltZ); aqui só abaixa um pouco.
    headDown: 0.04,
  },
  listen: {
    easeIn: 0.4,
    easeOut: 0.4,
    /// Inclina a cabeça (como quem presta atenção) e o tronco um pouco para a frente.
    headTilt: -0.1,
    lean: 0.04,
    arms: 0.12,
  },
  /// Saída rápida (mas suave) de um gesto cancelado por outro.
  cancelOut: 0.3,
  /// Fila: no máximo N gestos esperando, cada um vale por T segundos.
  queueMax: 2,
  queueTtl: 2.5,
};

/// 0..1 → 0..1 com início e fim suaves (senoide).
export function ease(x: number): number {
  if (x <= 0) return 0;
  if (x >= 1) return 1;
  return 0.5 - 0.5 * Math.cos(Math.PI * x);
}

/// Peso de um gesto de duração fixa no instante `t` (entra, segura, sai).
export function envelope(t: number, duration: number, easeIn: number, easeOut: number): number {
  if (t <= 0 || t >= duration) return 0;
  const a = easeIn > 0 ? ease(t / easeIn) : 1;
  const b = easeOut > 0 ? ease((duration - t) / easeOut) : 1;
  return Math.min(a, b);
}

/// Espelha uma rotação do braço direito para o esquerdo (X igual, Y e Z trocam de sinal).
export function mirror(v: Vec3): Vec3 {
  return [v[0], -v[1], -v[2]];
}

type ArmBone = "UpperArm" | "LowerArm" | "Hand";

/// Põe uma rotação escrita para o braço direito no braço `arm` da Lilith.
function arm(rot: Pose["rot"], which: "left" | "right", bone: ArmBone, v: Vec3) {
  rot[`${which}${bone}` as Bone] = which === "right" ? v : mirror(v);
}

/// O braço da Lilith que fica do lado da tela pedido (ela está de frente: o
/// braço esquerdo dela aparece à direita da tela).
export function armForSide(side: Side): "left" | "right" {
  return side === "right" ? "left" : "right";
}

interface GestureDef {
  priority: number;
  /// Infinity = mantido até soltar (pose de fundo).
  duration: number;
  easeIn: number;
  easeOut: number;
  pose(t: number, side: Side): Pose;
}

const TAU = Math.PI * 2;

export const GESTURES: Record<GestureName, GestureDef> = {
  /// Acena com a mão do lado `side` da tela (padrão: esquerda = mão direita dela).
  wave: {
    priority: 3,
    duration: TUNING.wave.duration,
    easeIn: TUNING.wave.easeIn,
    easeOut: TUNING.wave.easeOut,
    pose(t, side) {
      const c = TUNING.wave;
      const s = Math.sin(TAU * c.freq * t);
      const s2 = Math.sin(TAU * c.freq * t - 0.9);
      const which = armForSide(side);
      const rot: Pose["rot"] = {};
      arm(rot, which, "UpperArm", c.upperArm);
      arm(rot, which, "LowerArm", [c.lowerArm[0], c.lowerArm[1], c.lowerArm[2] + s * c.swing]);
      arm(rot, which, "Hand", [c.hand[0], c.hand[1], c.hand[2] + s2 * c.handSwing]);
      // Inclina a cabeça para o lado da mão que acena (z positivo = topo da
      // cabeça para a esquerda da tela).
      rot.head = [0, 0, side === "left" ? c.headTilt : -c.headTilt];
      return { rot, lift: 0 };
    },
  },
  nod: {
    priority: 2,
    duration: TUNING.nod.duration,
    easeIn: TUNING.nod.easeIn,
    easeOut: TUNING.nod.easeOut,
    pose(t) {
      const c = TUNING.nod;
      // 0 → baixa → volta, `count` vezes (x positivo = cabeça para baixo).
      const d = 0.5 - 0.5 * Math.cos((TAU * c.count * t) / c.duration);
      return { rot: { head: [c.head * d, 0, 0], neck: [c.neck * d, 0, 0] }, lift: 0 };
    },
  },
  shake: {
    priority: 2,
    duration: TUNING.shake.duration,
    easeIn: TUNING.shake.easeIn,
    easeOut: TUNING.shake.easeOut,
    pose(t) {
      const c = TUNING.shake;
      const s = Math.sin(TAU * c.freq * t);
      return { rot: { head: [c.down, c.head * s, 0], neck: [0, c.neck * s, 0] }, lift: 0 };
    },
  },
  /// Aponta para o lado `side` da tela (onde a ação na janela acontece).
  point: {
    priority: 3,
    duration: TUNING.point.duration,
    easeIn: TUNING.point.easeIn,
    easeOut: TUNING.point.easeOut,
    pose(_t, side) {
      const c = TUNING.point;
      const which = armForSide(side);
      const rot: Pose["rot"] = {};
      arm(rot, which, "UpperArm", c.upperArm);
      arm(rot, which, "LowerArm", c.lowerArm);
      arm(rot, which, "Hand", c.hand);
      // Dedos: dobrar = z positivo na mão direita, negativo na esquerda.
      const k = which === "right" ? c.curl : -c.curl;
      for (const f of ["Middle", "Ring", "Little"]) {
        rot[`${which}${f}Proximal` as Bone] = [0, 0, k];
        rot[`${which}${f}Intermediate` as Bone] = [0, 0, k];
      }
      // y positivo = olha para a direita da tela.
      rot.head = [0, side === "right" ? c.headYaw : -c.headYaw, 0];
      return { rot, lift: 0 };
    },
  },
  /// Pulinho feliz.
  bounce: {
    priority: 2,
    duration: TUNING.bounce.duration,
    easeIn: TUNING.bounce.easeIn,
    easeOut: TUNING.bounce.easeOut,
    pose(t) {
      const c = TUNING.bounce;
      const p = (t / c.duration) * c.hops;
      const hop = Math.abs(Math.sin(Math.PI * p));
      const rot: Pose["rot"] = {};
      arm(rot, "right", "UpperArm", [0, 0, -c.arms]);
      arm(rot, "left", "UpperArm", [0, 0, -c.arms]);
      rot.chest = [c.chest * hop, 0, 0];
      rot.head = [0, 0, c.headTilt * Math.sin(TAU * 0.5 * p)];
      return { rot, lift: c.lift * hop };
    },
  },
  /// Pose mantida enquanto ela pensa: mão perto do queixo.
  think: {
    priority: 0,
    duration: Infinity,
    easeIn: TUNING.think.easeIn,
    easeOut: TUNING.think.easeOut,
    pose() {
      const c = TUNING.think;
      const rot: Pose["rot"] = {};
      arm(rot, "right", "UpperArm", c.upperArm);
      arm(rot, "right", "LowerArm", c.lowerArm);
      arm(rot, "right", "Hand", c.hand);
      rot.head = [c.headDown, 0, 0];
      return { rot, lift: 0 };
    },
  },
  /// Pose mantida enquanto ouve a voz do usuário.
  listen: {
    priority: 0,
    duration: Infinity,
    easeIn: TUNING.listen.easeIn,
    easeOut: TUNING.listen.easeOut,
    pose(t) {
      const c = TUNING.listen;
      const rot: Pose["rot"] = {};
      // Braços um pouco para dentro/frente (atenta), cabeça inclinada.
      arm(rot, "right", "UpperArm", [-0.1, c.arms, 0.05]);
      arm(rot, "left", "UpperArm", [-0.1, c.arms, 0.05]);
      rot.head = [0, 0, c.headTilt + Math.sin(t * 0.8) * 0.01];
      rot.chest = [c.lean, 0, 0];
      return { rot, lift: 0 };
    },
  },
};

interface Instance {
  name: GestureName;
  def: GestureDef;
  side: Side;
  t: number;
  /// Tempo (em `t`) em que foi cancelado/solto, e o peso que tinha.
  cancelAt: number | null;
  cancelWeight: number;
  cancelOut: number;
}

function weight(i: Instance): number {
  if (i.cancelAt !== null) {
    const k = i.cancelOut > 0 ? (i.t - i.cancelAt) / i.cancelOut : 1;
    return i.cancelWeight * (1 - ease(k));
  }
  if (i.def.duration === Infinity) return i.def.easeIn > 0 ? ease(i.t / i.def.easeIn) : 1;
  return envelope(i.t, i.def.duration, i.def.easeIn, i.def.easeOut);
}

function done(i: Instance): boolean {
  if (i.cancelAt !== null) return i.t - i.cancelAt >= i.cancelOut;
  return i.t >= i.def.duration;
}

function cancel(i: Instance, out: number) {
  if (i.cancelAt !== null) return;
  i.cancelWeight = weight(i);
  i.cancelAt = i.t;
  i.cancelOut = out;
}

function addPose(into: Pose, p: Pose, w: number) {
  if (w <= 0) return;
  for (const [bone, v] of Object.entries(p.rot) as [Bone, Vec3][]) {
    const cur = into.rot[bone] ?? (into.rot[bone] = [0, 0, 0]);
    cur[0] += v[0] * w;
    cur[1] += v[1] * w;
    cur[2] += v[2] * w;
  }
  into.lift += p.lift * w;
}

/// Toca os gestos: um de cada vez (com crossfade), uma fila curta e as poses mantidas.
export class GesturePlayer {
  enabled = true;
  private active: Instance | null = null;
  private fading: Instance[] = [];
  private queue: { name: GestureName; side: Side; expires: number }[] = [];
  private holds = new Map<GestureName, Instance>();
  private released: Instance[] = [];
  private now = 0;

  /// Toca um gesto de uma vez. Devolve se começou (ou entrou na fila).
  play(name: GestureName, side: Side = "left"): boolean {
    if (!this.enabled) return false;
    const def = GESTURES[name];
    if (def.duration === Infinity) return false;
    const cur = this.active;
    if (cur && cur.cancelAt === null) {
      // O mesmo gesto já tocando: não reinicia (evita tremedeira).
      if (cur.name === name && cur.side === side) return false;
      if (def.priority < cur.def.priority) {
        if (this.queue.some((q) => q.name === name)) return false;
        if (this.queue.length >= TUNING.queueMax) this.queue.shift();
        this.queue.push({ name, side, expires: this.now + TUNING.queueTtl });
        return true;
      }
      cancel(cur, Math.min(cur.def.easeOut, TUNING.cancelOut));
      this.fading.push(cur);
    }
    this.start(name, side);
    return true;
  }

  /// Liga/desliga uma pose mantida (pensando, ouvindo).
  hold(name: GestureName, on: boolean) {
    const def = GESTURES[name];
    if (def.duration !== Infinity) return;
    const cur = this.holds.get(name);
    if (on && this.enabled) {
      if (cur) return;
      this.holds.set(name, { name, def, side: "left", t: 0, cancelAt: null, cancelWeight: 0, cancelOut: 0 });
    } else if (cur) {
      this.holds.delete(name);
      cancel(cur, def.easeOut);
      this.released.push(cur);
    }
  }

  /// Desliga tudo com saída suave (ex.: o usuário desligou os gestos).
  stopAll() {
    this.queue = [];
    if (this.active) {
      cancel(this.active, TUNING.cancelOut);
      this.fading.push(this.active);
      this.active = null;
    }
    for (const name of [...this.holds.keys()]) this.hold(name, false);
  }

  /// Zera na hora, sem transição (janela escondida).
  reset() {
    this.active = null;
    this.fading = [];
    this.queue = [];
    this.holds.clear();
    this.released = [];
  }

  /// Nome do gesto de uma vez tocando agora (para testes e depuração).
  get current(): GestureName | null {
    return this.active && this.active.cancelAt === null ? this.active.name : null;
  }

  get pending(): number {
    return this.queue.length;
  }

  /// Avança `dt` segundos e devolve o quanto somar em cada osso neste quadro.
  update(dt: number): Pose {
    dt = Math.max(0, Math.min(dt, 0.1));
    this.now += dt;
    const all = [this.active, ...this.fading, ...this.holds.values(), ...this.released];
    for (const i of all) if (i) i.t += dt;

    this.fading = this.fading.filter((i) => !done(i));
    this.released = this.released.filter((i) => !done(i));
    this.queue = this.queue.filter((q) => q.expires > this.now);

    const a = this.active;
    if (a && done(a)) this.active = null;
    // Próximo da fila: entra quando o atual começa a sair (emenda suave).
    if (this.queue.length > 0) {
      const cur = this.active;
      const leaving = !cur || (cur.def.duration - cur.t <= cur.def.easeOut);
      if (leaving) {
        if (cur) this.fading.push(cur);
        const next = this.queue.shift()!;
        this.start(next.name, next.side);
      }
    }

    const out: Pose = { rot: {}, lift: 0 };
    let oneShot = 0;
    for (const i of [this.active, ...this.fading]) {
      if (!i) continue;
      const w = weight(i);
      oneShot += w;
      addPose(out, i.def.pose(i.t, i.side), w);
    }
    // Poses mantidas recolhem enquanto um gesto de uma vez toca.
    const duck = 1 - Math.min(1, oneShot);
    for (const i of [...this.holds.values(), ...this.released]) {
      addPose(out, i.def.pose(i.t, i.side), weight(i) * duck);
    }
    return out;
  }

  private start(name: GestureName, side: Side) {
    this.active = { name, def: GESTURES[name], side, t: 0, cancelAt: null, cancelWeight: 0, cancelOut: 0 };
  }
}

/// Palavras de oi/tchau no começo da resposta (decide o aceno).
const GREETING = /^\s*(oi+|olá|ola|oie|eae|e aí|e ai|hey|hello|bom dia|boa tarde|boa noite|tchau|até mais|até logo|ate mais|ate logo|falou|bye)\b/i;

export function isGreeting(text: string): boolean {
  return text.length <= 200 && GREETING.test(text);
}

export type Emotion = "neutral" | "happy" | "concerned" | string;

/// Gesto para uma resposta pronta (determinístico: texto, emoção e se fez algo).
export function gestureForReply(reply: { reply?: string; emotion?: Emotion; action?: string | null }): GestureName | null {
  if (isGreeting(reply.reply ?? "")) return "wave";
  if (reply.emotion === "concerned") return "shake";
  if (reply.emotion === "happy") return "bounce";
  if (reply.action) return "nod";
  return null;
}

/// Passos do agente em que o cursor dela vai até uma janela (vale apontar).
export function isPointingStep(action: string): boolean {
  if (action === "cursor.point") return true;
  return action.startsWith("window.") && action !== "window.list" && action !== "window.read";
}

/// Lado da tela onde está `x` em relação ao centro da janela do avatar
/// (com uma zona morta no meio: nesse caso não aponta).
export function sideOf(x: number, avatarCenterX: number, deadZone = 80): Side | null {
  const d = x - avatarCenterX;
  if (Math.abs(d) < deadZone) return null;
  return d > 0 ? "right" : "left";
}
