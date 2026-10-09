// Offline import of CC0 packs into assets/models/. Not part of the build.
//
//   cd tools/assets && npm install
//   node import_assets.mjs <dir with the unzipped Kenney packs> <out dir, e.g. ../../assets/models>
//
// Source dir layout: <src>/car-kit, <src>/mini-characters, <src>/nature-kit
// (each is an unzipped Kenney zip; URLs are in assets/LICENSES.md).
//
// Per model: drop skins and animations (M1 has no skeletal animation, system-design 13.3),
// keep only POSITION/NORMAL/TEXCOORD_0/COLOR_0, bake world transforms, merge primitives per
// material, decimate to the triangle budget, then normalise to +Y up, +Z front, origin at the
// ground-centre of the bounding box, units = metres.
import fs from 'node:fs';
import path from 'node:path';
import { NodeIO, getBounds } from '@gltf-transform/core';
import { ALL_EXTENSIONS } from '@gltf-transform/extensions';
import { flatten, join, weld, simplify, prune, dedup, transformPrimitive } from '@gltf-transform/functions';
import { MeshoptSimplifier } from 'meshoptimizer';

const [src, out] = process.argv.slice(2);
if (!src || !out) throw new Error('usage: import_assets.mjs <src> <out>');

// Triangle budgets from the T-015 spec.
const BUDGET = { vehicles: 1500, people: 800, trees: 1500, props: 1500 };
// Uniform scale per category (keeps relative sizes inside a pack). Kenney cars are stubby
// cartoon proportions at ~1.5 u wide, so x1.5 gives ~2.2 m wide, 3.8 m long sedans.
const SCALE = { vehicles: 1.5, people: 2.4, trees: 6, props: 4 };

const CAR = 'car-kit/Models/GLB format';
const CHR = 'mini-characters/Models/GLB format';
const NAT = 'nature-kit/Models/GLTF format';
const car = (n) => ({ cat: 'vehicles', file: `${CAR}/${n}.glb`, name: n, scale: SCALE.vehicles });
const chr = (n) => ({ cat: 'people', file: `${CHR}/${n}.glb`, name: n, scale: SCALE.people });
const nat = (cat, n, scale = SCALE[cat]) => ({ cat, file: `${NAT}/${n}.glb`, name: n, scale });

const MODELS = [
  ...['sedan', 'sedan-sports', 'hatchback-sports', 'suv', 'suv-luxury', 'van', 'taxi', 'police',
    'ambulance', 'delivery', 'truck', 'garbage-truck'].map(car),
  ...['female-b', 'female-c', 'female-d', 'female-e', 'male-a', 'male-b', 'male-c', 'male-d']
    .map((n) => chr(`character-${n}`)),
  ...['tree_default', 'tree_detailed', 'tree_oak', 'tree_fat', 'tree_thin', 'tree_pineTallB_detailed',
    'tree_pineRoundA', 'tree_palmDetailedTall'].map((n) => nat('trees', n)),
  { cat: 'props', file: `${CAR}/cone.glb`, name: 'cone', scale: SCALE.vehicles },
  { cat: 'props', file: `${CAR}/box.glb`, name: 'box', scale: SCALE.vehicles },
  ...['plant_bushDetailed', 'rock_largeA', 'log', 'stump_round', 'sign'].map((n) => nat('props', n)),
];

const io = new NodeIO().registerExtensions(ALL_EXTENSIONS);
await MeshoptSimplifier.ready;

const KEEP = new Set(['POSITION', 'NORMAL', 'TEXCOORD_0', 'COLOR_0']);
const triCount = (doc) => {
  let t = 0;
  for (const m of doc.getRoot().listMeshes()) for (const p of m.listPrimitives()) {
    const i = p.getIndices();
    t += (i ? i.getCount() : p.getAttribute('POSITION').getCount()) / 3;
  }
  return t;
};

// Column-major 4x4 helpers (gltf convention).
const mul = (a, b) => {
  const r = new Array(16).fill(0);
  for (let c = 0; c < 4; c++) for (let rr = 0; rr < 4; rr++)
    for (let k = 0; k < 4; k++) r[c * 4 + rr] += a[k * 4 + rr] * b[c * 4 + k];
  return r;
};
const IDENT = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1];

// Skinned packs ship in a T-pose bind pose. Pose the skeleton from frame 0 of the "static"
// and then "idle" animations and bake the result into the vertices (linear blend skinning).
function bakeRestPose(root) {
  const skins = root.listSkins();
  if (!skins.length) return;
  // "static" is the T-pose; "idle" frame 0 lowers the arms.
  const anims = ['static', 'idle'].map((n) => root.listAnimations().find((a) => a.getName() === n)).filter(Boolean);
  for (const anim of anims) for (const ch of anim.listChannels()) {
    const n = ch.getTargetNode(), v = ch.getSampler().getOutput().getElement(0, []);
    const path = ch.getTargetPath();
    if (path === 'translation') n.setTranslation(v);
    else if (path === 'rotation') n.setRotation(v);
    else if (path === 'scale') n.setScale(v);
  }
  for (const node of root.listNodes()) {
    const skin = node.getSkin(), mesh = node.getMesh();
    if (!skin || !mesh) continue;
    const js = skin.listJoints(), ibm = skin.getInverseBindMatrices();
    const jm = js.map((j, i) => mul(j.getWorldMatrix(), ibm.getElement(i, [])));
    for (const p of mesh.listPrimitives()) {
      const pos = p.getAttribute('POSITION'), nor = p.getAttribute('NORMAL');
      const ji = p.getAttribute('JOINTS_0'), jw = p.getAttribute('WEIGHTS_0');
      for (let i = 0; i < pos.getCount(); i++) {
        const v = pos.getElement(i, []), n = nor ? nor.getElement(i, []) : null;
        const idx = ji.getElement(i, []), w = jw.getElement(i, []);
        const op = [0, 0, 0], on = [0, 0, 0];
        for (let k = 0; k < 4; k++) {
          if (!w[k]) continue;
          const m = jm[idx[k]];
          for (let r = 0; r < 3; r++) {
            op[r] += w[k] * (m[r] * v[0] + m[4 + r] * v[1] + m[8 + r] * v[2] + m[12 + r]);
            if (n) on[r] += w[k] * (m[r] * n[0] + m[4 + r] * n[1] + m[8 + r] * n[2]);
          }
        }
        pos.setElement(i, op);
        if (n) { const l = Math.hypot(...on) || 1; nor.setElement(i, on.map((x) => x / l)); }
      }
    }
    node.setMatrix(IDENT);
  }
}

const rows = [];
for (const m of MODELS) {
  const doc = await io.read(path.join(src, m.file));
  const root = doc.getRoot();

  bakeRestPose(root);
  root.listAnimations().forEach((a) => a.dispose());
  // Remove skeleton bones before dropping skins so they do not pollute bounds.
  const joints = new Set();
  root.listSkins().forEach((s) => s.listJoints().forEach((j) => joints.add(j)));
  root.listSkins().forEach((s) => s.dispose());
  for (const j of joints) j.dispose();
  for (const mesh of root.listMeshes()) for (const p of mesh.listPrimitives()) {
    for (const sem of p.listSemantics()) {
      if (!KEEP.has(sem)) { p.setAttribute(sem, null); continue; }
      // Some packs share one accessor between primitives; give each its own so the
      // in-place transform below is applied exactly once.
      p.setAttribute(sem, p.getAttribute(sem).clone());
    }
  }

  await doc.transform(flatten(), prune());
  const scene = root.listScenes()[0];
  const b = getBounds(scene);
  const s = m.scale;
  const cx = (b.min[0] + b.max[0]) / 2, cz = (b.min[2] + b.max[2]) / 2;
  // normalise = scale then move bbox ground-centre to the origin.
  const norm = [s, 0, 0, 0, 0, s, 0, 0, 0, 0, s, 0, -cx * s, -b.min[1] * s, -cz * s, 1];
  for (const node of scene.listChildren()) {
    const mesh = node.getMesh();
    if (!mesh) continue;
    const M = mul(norm, node.getWorldMatrix());
    mesh.listPrimitives().forEach((p) => transformPrimitive(p, M));
    node.setMatrix(IDENT);
  }

  await doc.transform(join({ keepNamed: false }), weld(), dedup(), prune());
  // Decimate to the budget, leaving a little headroom.
  const budget = BUDGET[m.cat];
  let tris = triCount(doc);
  const before = tris;
  for (let ratio = (budget * 0.95) / tris; tris > budget && ratio > 0.05; ratio *= 0.9) {
    await doc.transform(simplify({ simplifier: MeshoptSimplifier, ratio, error: 0.02 }));
    tris = triCount(doc);
  }
  if (tris > budget) throw new Error(`${m.name}: ${tris} tris over budget ${budget}`);

  const nb = getBounds(root.listScenes()[0]);
  const dir = path.join(out, m.cat);
  fs.mkdirSync(dir, { recursive: true });
  const target = path.join(dir, `${m.name}.glb`);
  await io.write(target, doc);
  const size = nb.max.map((v, i) => +(v - nb.min[i]).toFixed(2));
  rows.push({ ...m, before, tris, size, min: nb.min.map((v) => +v.toFixed(2)), bytes: fs.statSync(target).size });
  console.log(`${m.cat}/${m.name}.glb  ${before} -> ${tris} tris  size(x,y,z)=${size.join('x')}  ${(rows.at(-1).bytes / 1024).toFixed(0)}K`);
}
fs.writeFileSync(path.join(out, 'import-report.json'), JSON.stringify(rows, null, 1));
