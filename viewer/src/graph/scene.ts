import ForceGraph3D from "3d-force-graph";
import * as THREE from "three";
import type { NodeKind } from "../types";
import {
  DEAD,
  EDGE_COLORS,
  EFFECT_COLORS,
  hslToHex,
  NEUTRAL,
  packageColor,
  packageHue,
  SYNTHETIC_COLORS,
  withAlpha,
} from "./colors";
import type { View, ViewLink, ViewNode } from "./lod";

export type SceneLink = Omit<ViewLink, "source" | "target"> & {
  source: string | ViewNode;
  target: string | ViewNode;
};

export interface Highlight {
  nodes: ReadonlySet<string>;
  links: ReadonlySet<string>;
  color: string;
}

export interface SceneCallbacks {
  onClick(id: string): void;
  onDoubleClick(id: string): void;
  onBackground(): void;
}

export type ColorBy = "effect" | "package";

interface Glow {
  sprite: THREE.Sprite;
  node: ViewNode | null;
  from: ViewNode | null;
  to: ViewNode | null;
  start: number;
  duration: number;
  size: number;
  persistent: boolean;
}

const SYNTHETIC_GEOMETRY: Partial<Record<NodeKind, () => THREE.BufferGeometry>> = {
  table: () => new THREE.CylinderGeometry(4.5, 4.5, 2.2, 24),
  route: () => new THREE.OctahedronGeometry(4.5),
  host: () => new THREE.BoxGeometry(6, 6, 6),
  topic: () => new THREE.TorusGeometry(3.5, 1.2, 10, 24),
  dependency: () => new THREE.TetrahedronGeometry(4.5),
};

export class Scene {
  readonly fg: InstanceType<typeof ForceGraph3D>;
  private nodes = new Map<string, ViewNode>();
  private linkIds = new Map<string, SceneLink>();
  private selected: string | null = null;
  private highlight: Highlight | null = null;
  private heat = new Map<string, number>();
  private heatOn = false;
  colorBy: ColorBy = "effect";
  private readonly glowGroup = new THREE.Group();
  private readonly glows: Glow[] = [];
  private readonly glowTexture = makeGlowTexture();
  private frames = 0;
  private lastFpsAt = performance.now();
  private currentFps = 0;
  private lastClick: { id: string; at: number } | null = null;
  private raf = 0;
  private cheapLinks = false;
  private cheapNodes = false;
  private denseNodes = false;
  private fitPending = false;
  private batchedLinks: { mesh: THREE.LineSegments; links: SceneLink[] } | null = null;
  private batchedNodes: { points: THREE.Points; nodes: ViewNode[] } | null = null;

  constructor(
    el: HTMLElement,
    private readonly cb: SceneCallbacks,
  ) {
    this.fg = new ForceGraph3D(el, { controlType: "orbit" })
      .backgroundColor("rgba(0,0,0,0)")
      .showNavInfo(false)
      .nodeRelSize(3)
      .nodeResolution(10)
      .nodeOpacity(0.92)
      .nodeId("id")
      .nodeVisibility(() => !this.cheapNodes)
      .linkVisibility(() => !this.cheapLinks)
      .nodeVal((n) => this.val(n as ViewNode))
      .nodeColor((n) => this.nodeColor(n as ViewNode))
      .nodeLabel((n) => this.tooltip(n as ViewNode))
      .nodeThreeObject((n) => this.nodeObject(n as ViewNode))
      .nodeThreeObjectExtend(
        (n) => !(n as ViewNode).synthetic && (this.cheapNodes || !(n as ViewNode).dead),
      )
      .linkColor((l) => this.linkColor(l as SceneLink))
      .linkWidth((l) => this.linkWidth(l as SceneLink))
      .linkOpacity(0.55)
      .linkCurvature((l) => ((l as SceneLink).kind === "requests" ? 0.3 : 0))
      .linkDirectionalArrowLength((l) =>
        this.cheapLinks
          ? 0
          : (l as SceneLink).kind === "requests" || (l as SceneLink).kind === "handles"
            ? 4
            : 0,
      )
      .linkDirectionalArrowRelPos(0.85)
      .linkDirectionalArrowColor((l) => this.linkColor(l as SceneLink))
      .linkThreeObject((l) =>
        !this.cheapLinks && (l as SceneLink).kind === "spawns"
          ? dashedLine(this.linkColor(l as SceneLink))
          : (undefined as unknown as THREE.Object3D),
      )
      .linkPositionUpdate((obj, { start, end }, l) => {
        if (this.cheapLinks || (l as SceneLink).kind !== "spawns") return false;
        const line = obj as THREE.Line;
        const pos = line.geometry.getAttribute("position") as THREE.BufferAttribute;
        pos.setXYZ(0, start.x, start.y, start.z);
        pos.setXYZ(1, end.x, end.y, end.z);
        pos.needsUpdate = true;
        line.computeLineDistances();
        return true;
      })
      .linkDirectionalParticleWidth(2.2)
      .linkDirectionalParticleColor(() => "#ffe29a")
      .onNodeClick((n) => this.handleClick(n as ViewNode))
      .onBackgroundClick(() => this.cb.onBackground())
      .d3AlphaDecay(0.03)
      .d3VelocityDecay(0.35)
      .warmupTicks(30)
      .cooldownTicks(260);
    this.fg.d3Force("charge")?.strength?.(-60);
    this.fg.d3Force("link")?.distance?.((l: SceneLink) => (l.kind === "requests" ? 90 : 45));
    this.fg.onEngineStop(() => {
      if (!this.fitPending) return;
      this.fitPending = false;
      this.fg.zoomToFit(700, 80);
    });
    this.fg.d3Force("cluster", this.clusterForce());
    this.fg.scene().add(this.glowGroup);
    const light = new THREE.PointLight(0x8899ff, 1.5, 0, 0);
    light.position.set(300, 400, 300);
    this.fg.scene().add(light);
    this.tick = this.tick.bind(this);
    this.raf = requestAnimationFrame(this.tick);
    new ResizeObserver(() => this.fg.width(el.clientWidth).height(el.clientHeight)).observe(el);
  }

  setView(view: View): void {
    const prev = this.nodes;
    this.nodes = new Map();
    for (const n of view.nodes) {
      const old = prev.get(n.id);
      if (old) {
        n.x = old.x;
        n.y = old.y;
        n.z = old.z;
      }
      this.nodes.set(n.id, n);
    }
    this.linkIds = new Map();
    const links: SceneLink[] = [];
    for (const l of view.links) {
      const sl: SceneLink = { ...l };
      links.push(sl);
      this.linkIds.set(`${l.source}|${l.target}`, sl);
    }
    this.cheapLinks = links.length > BATCH_LINKS_ABOVE;
    this.cheapNodes = view.nodes.length > BATCH_NODES_ABOVE;
    this.denseNodes = view.nodes.length > DECORATE_BELOW;
    this.fitPending = prev.size === 0;
    if (this.fitPending) {
      if (view.nodes.length < 600) window.setTimeout(() => this.fg.zoomToFit(700, 80), 1500);
      else {
        const r = layoutRadius(view.nodes);
        this.fg.cameraPosition({ x: 0, y: r * 0.6, z: r * 2.6 }, { x: 0, y: 0, z: 0 }, 0);
      }
    }
    this.clearGlows(false);
    this.fg.warmupTicks(this.cheapNodes ? 0 : 30).cooldownTicks(this.cheapNodes ? 150 : 260);
    this.fg.graphData({ nodes: view.nodes, links });
    this.fg.linkWidth(this.fg.linkWidth());
    this.rebuildBatches(view.nodes, links);
  }

  /** Above the thresholds, nodes and links are drawn as one Points cloud and one LineSegments instead of one object each. */
  private rebuildBatches(nodes: ViewNode[], links: SceneLink[]): void {
    if (this.batchedLinks) {
      this.fg.scene().remove(this.batchedLinks.mesh);
      this.batchedLinks.mesh.geometry.dispose();
      (this.batchedLinks.mesh.material as THREE.Material).dispose();
      this.batchedLinks = null;
    }
    if (this.batchedNodes) {
      this.fg.scene().remove(this.batchedNodes.points);
      this.batchedNodes.points.geometry.dispose();
      (this.batchedNodes.points.material as THREE.Material).dispose();
      this.batchedNodes = null;
    }
    if (this.cheapLinks) {
      const geo = new THREE.BufferGeometry();
      geo.setAttribute(
        "position",
        new THREE.BufferAttribute(new Float32Array(links.length * 6), 3),
      );
      geo.setAttribute("color", new THREE.BufferAttribute(new Float32Array(links.length * 6), 3));
      const mesh = new THREE.LineSegments(
        geo,
        new THREE.LineBasicMaterial({
          vertexColors: true,
          transparent: true,
          opacity: links.length > 8000 ? 0.12 : 0.3,
          depthWrite: false,
        }),
      );
      mesh.frustumCulled = false;
      this.fg.scene().add(mesh);
      this.batchedLinks = { mesh, links };
    }
    if (this.cheapNodes) {
      const geo = new THREE.BufferGeometry();
      geo.setAttribute(
        "position",
        new THREE.BufferAttribute(new Float32Array(nodes.length * 3), 3),
      );
      geo.setAttribute("color", new THREE.BufferAttribute(new Float32Array(nodes.length * 3), 3));
      const sizes = new Float32Array(nodes.length);
      nodes.forEach((n, i) => {
        sizes[i] = Math.cbrt(this.val(n)) * 9;
      });
      geo.setAttribute("size", new THREE.BufferAttribute(sizes, 1));
      const points = new THREE.Points(geo, pointsMaterial(this.glowTexture));
      points.frustumCulled = false;
      this.fg.scene().add(points);
      this.batchedNodes = { points, nodes };
    }
    this.restyleBatches();
  }

  private restyleBatches(): void {
    if (this.batchedLinks) {
      const colors = this.batchedLinks.mesh.geometry.getAttribute("color") as THREE.BufferAttribute;
      this.batchedLinks.links.forEach((l, i) => {
        const [r, g, b] = toRgb(this.linkColor(l));
        colors.setXYZ(i * 2, r, g, b);
        colors.setXYZ(i * 2 + 1, r, g, b);
      });
      colors.needsUpdate = true;
    }
    if (this.batchedNodes) {
      const colors = this.batchedNodes.points.geometry.getAttribute(
        "color",
      ) as THREE.BufferAttribute;
      this.batchedNodes.nodes.forEach((n, i) => {
        const [r, g, b] = toRgb(this.nodeColor(n));
        colors.setXYZ(i, r, g, b);
      });
      colors.needsUpdate = true;
    }
  }

  private updateBatches(): void {
    if (this.batchedLinks) {
      const pos = this.batchedLinks.mesh.geometry.getAttribute("position") as THREE.BufferAttribute;
      this.batchedLinks.links.forEach((l, i) => {
        const a = l.source as ViewNode;
        const b = l.target as ViewNode;
        if (typeof a === "string" || typeof b === "string") return;
        pos.setXYZ(i * 2, a.x ?? 0, a.y ?? 0, a.z ?? 0);
        pos.setXYZ(i * 2 + 1, b.x ?? 0, b.y ?? 0, b.z ?? 0);
      });
      pos.needsUpdate = true;
    }
    if (this.batchedNodes) {
      const pos = this.batchedNodes.points.geometry.getAttribute(
        "position",
      ) as THREE.BufferAttribute;
      this.batchedNodes.nodes.forEach((n, i) => {
        pos.setXYZ(i, n.x ?? 0, n.y ?? 0, n.z ?? 0);
      });
      pos.needsUpdate = true;
    }
  }

  drawCalls(): number {
    return this.fg.renderer().info.render.calls;
  }

  get batched(): { nodes: boolean; links: boolean } {
    return { nodes: this.cheapNodes, links: this.cheapLinks };
  }

  has(id: string): boolean {
    return this.nodes.has(id);
  }

  node(id: string): ViewNode | undefined {
    return this.nodes.get(id);
  }

  restyle(): void {
    this.fg.nodeColor(this.fg.nodeColor());
    this.fg.linkColor(this.fg.linkColor());
    this.fg.nodeThreeObject(this.fg.nodeThreeObject());
    this.restyleBatches();
  }

  setSelected(id: string | null): void {
    this.selected = id;
    this.restyle();
  }

  setHighlight(h: Highlight | null): void {
    this.highlight = h;
    this.restyle();
  }

  setHeat(values: Map<string, number> | null): void {
    this.heat = values ?? new Map();
    this.heatOn = values !== null && values.size > 0;
    this.fg.nodeVal(this.fg.nodeVal());
    this.restyle();
  }

  setColorBy(mode: ColorBy): void {
    this.colorBy = mode;
    this.restyle();
  }

  focus(id: string, ms = 900): void {
    const n = this.nodes.get(id);
    if (!n || n.x === undefined) return;
    const dist = 140;
    const len = Math.hypot(n.x, n.y ?? 0, n.z ?? 0) || 1;
    const ratio = 1 + dist / len;
    this.fg.cameraPosition(
      { x: n.x * ratio, y: (n.y ?? 0) * ratio, z: (n.z ?? 0) * ratio },
      { x: n.x, y: n.y ?? 0, z: n.z ?? 0 },
      ms,
    );
  }

  zoomToFit(): void {
    this.fg.zoomToFit(600, 40);
  }

  flash(id: string, color = "#ffffff", size = 14, duration = 900): void {
    const node = this.nodes.get(id);
    if (!node) return;
    this.addGlow({ node, from: null, to: null, color, size, duration, persistent: false });
  }

  travel(fromId: string, toId: string, color = "#ffe29a", duration = 700, size = 7): void {
    const from = this.nodes.get(fromId);
    const to = this.nodes.get(toId);
    if (!from || !to || from === to) return;
    this.addGlow({ node: null, from, to, color, size, duration, persistent: false });
    const link = this.linkIds.get(`${fromId}|${toId}`) ?? this.linkIds.get(`${toId}|${fromId}`);
    if (link && !this.cheapLinks) this.fg.emitParticle(link);
  }

  halo(id: string, color: string, size = 18): void {
    const node = this.nodes.get(id);
    if (!node) return;
    this.addGlow({
      node,
      from: null,
      to: null,
      color,
      size,
      duration: Number.POSITIVE_INFINITY,
      persistent: true,
    });
  }

  clearGlows(persistentOnly: boolean): void {
    for (let i = this.glows.length - 1; i >= 0; i--) {
      const g = this.glows[i]!;
      if (persistentOnly && !g.persistent) continue;
      this.glowGroup.remove(g.sprite);
      g.sprite.material.dispose();
      this.glows.splice(i, 1);
    }
  }

  fps(): number {
    return this.currentFps;
  }

  dispose(): void {
    cancelAnimationFrame(this.raf);
    this.fg._destructor();
  }

  private addGlow(o: {
    node: ViewNode | null;
    from: ViewNode | null;
    to: ViewNode | null;
    color: string;
    size: number;
    duration: number;
    persistent: boolean;
  }): void {
    const material = new THREE.SpriteMaterial({
      map: this.glowTexture,
      color: new THREE.Color(o.color),
      transparent: true,
      blending: THREE.AdditiveBlending,
      depthWrite: false,
    });
    const sprite = new THREE.Sprite(material);
    sprite.scale.set(o.size, o.size, 1);
    this.glowGroup.add(sprite);
    this.glows.push({
      sprite,
      node: o.node,
      from: o.from,
      to: o.to,
      start: performance.now(),
      duration: o.duration,
      size: o.size,
      persistent: o.persistent,
    });
    if (this.glows.length > 400) {
      const g = this.glows.shift()!;
      this.glowGroup.remove(g.sprite);
      g.sprite.material.dispose();
    }
  }

  private tick(): void {
    this.raf = requestAnimationFrame(this.tick);
    const now = performance.now();
    this.frames++;
    this.updateBatches();
    if (now - this.lastFpsAt >= 1000) {
      this.currentFps = Math.round((this.frames * 1000) / (now - this.lastFpsAt));
      this.frames = 0;
      this.lastFpsAt = now;
    }
    for (let i = this.glows.length - 1; i >= 0; i--) {
      const g = this.glows[i]!;
      const t = (now - g.start) / g.duration;
      if (t >= 1 && !g.persistent) {
        this.glowGroup.remove(g.sprite);
        g.sprite.material.dispose();
        this.glows.splice(i, 1);
        continue;
      }
      if (g.node) {
        g.sprite.position.set(g.node.x ?? 0, g.node.y ?? 0, g.node.z ?? 0);
        if (g.persistent) {
          const pulse = 1 + 0.15 * Math.sin(now / 350);
          g.sprite.scale.set(g.size * pulse, g.size * pulse, 1);
          g.sprite.material.opacity = 0.75;
        } else {
          const s = g.size * (1 + t * 1.8);
          g.sprite.scale.set(s, s, 1);
          g.sprite.material.opacity = 1 - t;
        }
      } else if (g.from && g.to) {
        const e = t < 0.5 ? 2 * t * t : 1 - (-2 * t + 2) ** 2 / 2;
        g.sprite.position.set(
          (g.from.x ?? 0) + ((g.to.x ?? 0) - (g.from.x ?? 0)) * e,
          (g.from.y ?? 0) + ((g.to.y ?? 0) - (g.from.y ?? 0)) * e,
          (g.from.z ?? 0) + ((g.to.z ?? 0) - (g.from.z ?? 0)) * e,
        );
        g.sprite.material.opacity = 0.95;
      }
    }
  }

  private handleClick(n: ViewNode): void {
    const now = performance.now();
    if (this.lastClick && this.lastClick.id === n.id && now - this.lastClick.at < 350) {
      this.lastClick = null;
      this.cb.onDoubleClick(n.id);
      return;
    }
    this.lastClick = { id: n.id, at: now };
    this.cb.onClick(n.id);
  }

  private val(n: ViewNode): number {
    const heat = this.heatOn ? (this.heat.get(n.id) ?? 0) : 0;
    return n.val * (1 + heat * 2.5);
  }

  private baseColor(n: ViewNode): string {
    if (n.synthetic) return SYNTHETIC_COLORS[n.kind] ?? NEUTRAL;
    if (n.dead) return DEAD;
    if (this.colorBy === "package") return packageColor(n.pkgIndex);
    if (n.effect) return EFFECT_COLORS[n.effect];
    return packageColor(n.pkgIndex, 58, 28);
  }

  private nodeColor(n: ViewNode): string {
    if (this.selected === n.id) return "#ffffff";
    if (this.heatOn) {
      const h = this.heat.get(n.id);
      if (h !== undefined && h > 0) return heatColor(h);
      if (!this.highlight) return withAlpha(toHex(this.baseColor(n)), 0.25);
    }
    if (this.highlight) {
      if (this.highlight.nodes.has(n.id)) return this.highlight.color;
      return withAlpha(toHex(this.baseColor(n)), 0.12);
    }
    const c = this.baseColor(n);
    return n.dead ? withAlpha(toHex(c), 0.35) : c;
  }

  private linkColor(l: SceneLink): string {
    const key = `${endId(l.source)}|${endId(l.target)}`;
    const base = EDGE_COLORS[l.kind];
    if (this.highlight) {
      if (this.highlight.links.has(key)) return this.highlight.color;
      return withAlpha(base, 0.05);
    }
    const sel = this.selected;
    if (sel && (endId(l.source) === sel || endId(l.target) === sel)) return "#ffffff";
    const dim = l.confidence === "name" || l.confidence === "heuristic" ? 0.35 : 0.8;
    return withAlpha(base, l.kind === "spawns" ? 1 : dim);
  }

  private linkWidth(l: SceneLink): number {
    if (this.cheapLinks) return 0;
    const w = 1 + Math.log2(l.weight);
    switch (l.kind) {
      case "requests":
        return 1.6 * w;
      case "handles":
        return 1.1 * w;
      case "reads":
      case "writes":
        return 0.8 * w;
      case "spawns":
        return 0;
      default:
        return l.weight > 1 ? 0.5 * w : 0;
    }
  }

  private tooltip(n: ViewNode): string {
    const parts = [
      `<b>${esc(n.label)}</b> <span class="muted">${n.kind}${n.aggregate ? ` · ${n.members.length} nodes` : ""}</span>`,
    ];
    if (n.effects.length) parts.push(`<span class="muted">${n.effects.join(", ")}</span>`);
    if (n.entrypoint) parts.push(`<span class="muted">entrypoint: ${n.entrypoint}</span>`);
    if (n.dead) parts.push(`<span class="muted">dead</span>`);
    return `<div class="tip">${parts.join("<br>")}</div>`;
  }

  private nodeObject(n: ViewNode): THREE.Object3D {
    if (n.synthetic) {
      const geo = SYNTHETIC_GEOMETRY[n.kind]?.() ?? new THREE.SphereGeometry(3);
      const color = new THREE.Color(this.nodeColor(n));
      const mesh = new THREE.Mesh(
        geo,
        new THREE.MeshLambertMaterial({
          color,
          emissive: color,
          emissiveIntensity: 0.35,
          transparent: true,
          opacity: this.highlight && !this.highlight.nodes.has(n.id) ? 0.2 : 0.95,
        }),
      );
      const ring = new THREE.Mesh(
        new THREE.RingGeometry(6.5, 7.3, 32),
        new THREE.MeshBasicMaterial({
          color,
          side: THREE.DoubleSide,
          transparent: true,
          opacity: 0.6,
        }),
      );
      ring.rotation.x = Math.PI / 2;
      const group = new THREE.Group();
      group.add(mesh, ring);
      if (!this.denseNodes) {
        const label = this.label(n.label, "#dfe6ff", 5);
        label.position.y = 9;
        group.add(label);
      }
      return group;
    }
    if (this.cheapNodes || this.denseNodes) return undefined as unknown as THREE.Object3D;
    if (n.dead) {
      const r = Math.cbrt(this.val(n)) * 3;
      return new THREE.Mesh(
        new THREE.SphereGeometry(r, 8, 6),
        new THREE.MeshLambertMaterial({
          color: new THREE.Color(DEAD),
          transparent: true,
          opacity: 0.3,
          wireframe: true,
        }),
      );
    }
    const group = new THREE.Group();
    if (n.entrypoint) {
      const r = Math.cbrt(this.val(n)) * 3;
      const halo = new THREE.Mesh(
        new THREE.RingGeometry(r + 1.6, r + 2.4, 32),
        new THREE.MeshBasicMaterial({
          color: 0xffe29a,
          side: THREE.DoubleSide,
          transparent: true,
          opacity: 0.85,
        }),
      );
      group.add(halo);
    }
    if (n.aggregate) {
      const r = Math.cbrt(this.val(n)) * 3;
      const label = this.label(n.label, packageColor(n.pkgIndex, 80), n.kind === "package" ? 8 : 5);
      label.position.y = r + 6;
      group.add(label);
      if (n.kind === "package") {
        const ring = new THREE.Mesh(
          new THREE.RingGeometry(r + 2, r + 2.6, 48),
          new THREE.MeshBasicMaterial({
            color: hslToHex(packageHue(n.pkgIndex), 70, 60),
            side: THREE.DoubleSide,
            transparent: true,
            opacity: 0.5,
          }),
        );
        ring.rotation.x = Math.PI / 2;
        group.add(ring);
      }
    }
    return group;
  }

  private label(text: string, color: string, size: number): THREE.Sprite {
    const canvas = document.createElement("canvas");
    const ctx = canvas.getContext("2d")!;
    const font = "600 48px Inter, system-ui, sans-serif";
    ctx.font = font;
    const w = Math.ceil(ctx.measureText(text).width) + 16;
    canvas.width = Math.max(2, w);
    canvas.height = 64;
    ctx.font = font;
    ctx.fillStyle = color;
    ctx.textBaseline = "middle";
    ctx.fillText(text, 8, 32);
    const tex = new THREE.CanvasTexture(canvas);
    tex.minFilter = THREE.LinearFilter;
    const sprite = new THREE.Sprite(
      new THREE.SpriteMaterial({ map: tex, transparent: true, depthWrite: false }),
    );
    sprite.scale.set((size * canvas.width) / canvas.height, size, 1);
    return sprite;
  }

  private clusterForce() {
    let nodes: ViewNode[] = [];
    const force = (alpha: number) => {
      const pkgCount = packageCount(nodes);
      const radius = layoutRadius(nodes);
      for (const n of nodes) {
        const c = centroid(n, pkgCount, radius);
        if (!c) continue;
        const k = (n.synthetic ? 0.14 : n.aggregate ? 0.06 : 0.1) * alpha;
        const f = n as ForceNode;
        f.vx = (f.vx ?? 0) + (c[0] - (n.x ?? 0)) * k;
        f.vy = (f.vy ?? 0) + (c[1] - (n.y ?? 0)) * k;
        f.vz = (f.vz ?? 0) + (c[2] - (n.z ?? 0)) * k;
      }
    };
    force.initialize = (ns: object[]) => {
      nodes = ns as ViewNode[];
    };
    return force;
  }
}

type ForceNode = ViewNode & { vx?: number; vy?: number; vz?: number };

const BATCH_LINKS_ABOVE = 1500;

function packageCount(nodes: ViewNode[]): number {
  return Math.max(1, new Set(nodes.map((n) => n.pkg).filter(Boolean)).size);
}

function layoutRadius(nodes: ViewNode[]): number {
  return 40 + 9 * Math.sqrt(nodes.length) + 6 * packageCount(nodes);
}
const BATCH_NODES_ABOVE = 4000;
const DECORATE_BELOW = 250;

const RGBA = /^rgba?\(\s*(\d+)\s*,\s*(\d+)\s*,\s*(\d+)\s*(?:,\s*([\d.]+))?\s*\)$/;

/** Linear-ish RGB triple; alpha is folded into brightness since batches draw additively on a dark background. */
function toRgb(color: string): [number, number, number] {
  const m = RGBA.exec(color);
  if (m) {
    const a = m[4] === undefined ? 1 : Number(m[4]);
    return [(Number(m[1]) / 255) * a, (Number(m[2]) / 255) * a, (Number(m[3]) / 255) * a];
  }
  const c = new THREE.Color(color);
  return [c.r, c.g, c.b];
}

function pointsMaterial(map: THREE.Texture): THREE.ShaderMaterial {
  return new THREE.ShaderMaterial({
    uniforms: { map: { value: map } },
    vertexShader: `
      attribute float size;
      varying vec3 vColor;
      void main() {
        vColor = color;
        vec4 mv = modelViewMatrix * vec4(position, 1.0);
        gl_PointSize = size * (220.0 / max(1.0, -mv.z));
        gl_Position = projectionMatrix * mv;
      }`,
    fragmentShader: `
      uniform sampler2D map;
      varying vec3 vColor;
      void main() {
        vec4 t = texture2D(map, gl_PointCoord);
        gl_FragColor = vec4(vColor, 1.0) * t;
      }`,
    vertexColors: true,
    transparent: true,
    depthWrite: false,
    blending: THREE.AdditiveBlending,
  });
}

const SYNTHETIC_ANGLE: Partial<Record<NodeKind, number>> = {
  route: 0,
  host: 1.26,
  topic: 2.51,
  dependency: 3.77,
  table: 5.03,
};

function centroid(n: ViewNode, pkgCount: number, radius: number): [number, number, number] | null {
  if (n.synthetic) {
    const a = SYNTHETIC_ANGLE[n.kind] ?? 0;
    const r = radius * 1.5;
    return [Math.cos(a) * r, n.kind === "table" ? -radius * 0.6 : radius * 0.3, Math.sin(a) * r];
  }
  if (n.pkgIndex < 0) return null;
  const a = (n.pkgIndex / pkgCount) * Math.PI * 2;
  const y = ((n.pkgIndex % 3) - 1) * radius * 0.25;
  return [Math.cos(a) * radius, y, Math.sin(a) * radius];
}

function endId(e: string | ViewNode): string {
  return typeof e === "string" ? e : e.id;
}

function toHex(color: string): string {
  if (color.startsWith("#")) return color;
  return `#${new THREE.Color(color).getHexString()}`;
}

function heatColor(h: number): string {
  const hue = 60 - h * 60;
  return `hsl(${hue.toFixed(1)}, 95%, ${(55 + h * 15).toFixed(1)}%)`;
}

function dashedLine(color: string): THREE.Line {
  const geo = new THREE.BufferGeometry();
  geo.setAttribute("position", new THREE.BufferAttribute(new Float32Array(6), 3));
  return new THREE.Line(
    geo,
    new THREE.LineDashedMaterial({
      color: new THREE.Color(toHex(color)),
      dashSize: 3,
      gapSize: 2,
      transparent: true,
      opacity: 0.95,
    }),
  );
}

function makeGlowTexture(): THREE.Texture {
  const c = document.createElement("canvas");
  c.width = c.height = 64;
  const ctx = c.getContext("2d")!;
  const g = ctx.createRadialGradient(32, 32, 0, 32, 32, 32);
  g.addColorStop(0, "rgba(255,255,255,1)");
  g.addColorStop(0.3, "rgba(255,255,255,0.6)");
  g.addColorStop(1, "rgba(255,255,255,0)");
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, 64, 64);
  return new THREE.CanvasTexture(c);
}

function esc(s: string): string {
  return s.replace(
    /[&<>"]/g,
    (ch) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[ch]!,
  );
}
