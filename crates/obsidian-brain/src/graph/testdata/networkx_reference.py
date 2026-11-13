"""Computes reference PageRank and betweenness scores with networkx.

Reads specs.json, which generate.mts writes (every test graph), rebuilds
each graph the way KnowledgeGraph.from_store and to_undirected do, and
writes networkx.json. Run with a Python that has networkx, scipy and
numpy:

    python3 networkx_reference.py specs.json networkx.json
"""

import json
import sys

import networkx as nx


def build(spec):
    stubs = {n[0] for n in spec["nodes"] if n[2]}
    # SELECT id FROM nodes returns ids in index (binary) order.
    ids = sorted(n[0] for n in spec["nodes"])
    keep = [i for i in ids if spec["includeStubs"] or i not in stubs]
    g = nx.Graph()
    g.add_nodes_from(keep)
    for source in keep:
        for s, t, _ in spec["edges"]:
            if s == source and t in g:
                g.add_edge(s, t)
    return keep, g


def reference(spec):
    order, g = build(spec)
    isolated = [n for n in order if g.degree(n) == 0]
    connected = [n for n in order if g.degree(n) > 0]
    page_rank = [[n, 0.0] for n in isolated]
    if connected:
        pr = nx.pagerank(g.subgraph(connected), alpha=0.85, max_iter=1000, tol=1e-6)
        page_rank += [[n, pr[n]] for n in connected]
    # normalized=True scales by 1 / ((n - 1)(n - 2)), as graphology does.
    bc = nx.betweenness_centrality(g, normalized=True)
    return {
        "name": spec["name"],
        "includeStubs": spec["includeStubs"],
        "nodes": spec["nodes"],
        "edges": spec["edges"],
        "pageRank": page_rank,
        "betweenness": [[n, bc[n]] for n in order],
    }


specs = json.load(open(sys.argv[1]))
out = [reference(s) for s in specs]
json.dump(out, open(sys.argv[2], "w"))
open(sys.argv[2], "a").write("\n")
