// Records graphology-metrics' PageRank and betweenness through the
// TypeScript graph code. Run from a checkout with node_modules, with ROOT
// pointing at it:
//   node_modules/.bin/tsx generate.mts graphology.json specs.json
// graphology.json keeps the graphs where graphology's index is correct
// (no self-loops, not 128 or 32768 edges). specs.json holds every graph,
// as input for networkx_reference.py.
const ROOT = '/var/lib/coder/work/tmp-checkout/obsidian-brain-rust-port';
const { openDb } = await import(ROOT + '/src/store/db.ts');
const { upsertNode } = await import(ROOT + '/src/store/nodes.ts');
const { insertEdge } = await import(ROOT + '/src/store/edges.ts');
const { KnowledgeGraph } = await import(ROOT + '/src/graph/builder.ts');
const C = await import(ROOT + '/src/graph/centrality.ts');
const fs = await import('node:fs');

type Spec = { name: string; nodes: [string, string, boolean][]; edges: [string, string, string][]; includeStubs: boolean };
const specs: Spec[] = [];
const n = (ids: string[]) => ids.map((id) => [id, id.replace('.md', '').toUpperCase(), false] as [string, string, boolean]);

specs.push({ name: 'small', includeStubs: false, nodes: n(['a.md','b.md','c.md','d.md']),
  edges: [['a.md','b.md','A links to B'],['b.md','c.md','B links to C'],['a.md','c.md','A links to C']] });
specs.push({ name: 'star', includeStubs: false, nodes: n(['hub.md','s1.md','s2.md','s3.md','s4.md','s5.md']),
  edges: ['s1','s2','s3','s4','s5'].map((s) => [s+'.md','hub.md','x']) });
specs.push({ name: 'quirks', includeStubs: true,
  nodes: [...n(['m.md','z.md','k.md','a.md','q.md','b.md','iso1.md','iso2.md','y.md','x.md']), ['_stub/ghost.md','Ghost',true]],
  edges: [['m.md','z.md','1'],['z.md','m.md','2'],['m.md','z.md','3'],['m.md','m.md','self'],['k.md','a.md','4'],
          ['a.md','q.md','5'],['q.md','k.md','6'],['q.md','q.md','self2'],['b.md','a.md','7'],['b.md','m.md','8'],
          ['y.md','x.md','9'],['x.md','_stub/ghost.md','10'],['y.md','_stub/ghost.md','11'],['x.md','x.md','self3']] });
{
  const ids: string[] = []; for (let i = 0; i < 129; i++) ids.push(`p${String(i).padStart(3,'0')}.md`);
  const edges: [string,string,string][] = []; for (let i = 0; i < 128; i++) edges.push([ids[i], ids[i+1], 'p']);
  specs.push({ name: 'path129', includeStubs: false, nodes: n(ids), edges });
}
{
  let seed = 12345; const rnd = () => { seed = (seed * 1103515245 + 12345) % 2147483648; return seed / 2147483648; };
  const ids: string[] = []; for (let i = 0; i < 40; i++) ids.push(`r${i}.md`);
  const edges: [string,string,string][] = [];
  for (let k = 0; k < 90; k++) { const s = ids[Math.floor(rnd()*40)]; const t = ids[Math.floor(rnd()*40)]; edges.push([s, t, `e${k}`]); }
  specs.push({ name: 'random40', includeStubs: false, nodes: n(ids), edges });
}

const out: unknown[] = [];
for (const spec of specs) {
  const db = openDb(':memory:');
  for (const [id, title, stub] of spec.nodes) upsertNode(db, { id, title, content: '', frontmatter: stub ? { _stub: true } : {} });
  for (const [s, t, c] of spec.edges) insertEdge(db, { sourceId: s, targetId: t, context: c });
  const kg = KnowledgeGraph.fromStore(db, { includeStubs: spec.includeStubs });
  const u = kg.toUndirected();
  out.push({
    ...spec,
    pageRank: Object.entries(C.pageRank(u)).map(([id, v]) => [id, String(v)]),
    betweennessTop: C.betweennessCentralityTop(u, 1000).map((r) => ({ ...r, score: String(r.score) })),
    betweennessNormalized: C.betweennessCentralityNormalized(u, 1000).map((r) => ({ ...r, score: String(r.score) })),
  });
  db.close();
}
const exact = new Set(['small', 'star']);
fs.writeFileSync(process.argv[2], JSON.stringify(out.filter((c) => exact.has((c as Spec).name))) + '\n');
fs.writeFileSync(process.argv[3], JSON.stringify(specs) + '\n');
