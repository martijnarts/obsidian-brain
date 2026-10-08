/**
 * Tag helpers over the indexed frontmatter JSON. A note's tags come from the
 * frontmatter `tags` / `tag` key (a list, or a comma- or space-separated
 * string, each entry with or without a leading `#`) plus the `inline_tags`
 * array the parser stores for body `#tags`. Tags compare case-insensitively,
 * as in Obsidian: `#Project` and `#project` are one tag.
 */

function splitTagValue(value: unknown): string[] {
  if (Array.isArray(value)) return value.flatMap(splitTagValue);
  if (typeof value !== 'string') return [];
  return value.split(/[,\s]+/);
}

/**
 * Every distinct tag of one note, without the leading `#`. Of tags that
 * differ only in case, the first spelling is kept.
 */
export function noteTags(frontmatter: Record<string, unknown>): string[] {
  const raw = [
    ...splitTagValue(frontmatter.tags),
    ...splitTagValue(frontmatter.tag),
    ...splitTagValue(frontmatter.inline_tags),
  ];
  const tags = new Map<string, string>();
  for (const t of raw) {
    const tag = t.replace(/^#/, '');
    if (tag && !tags.has(tag.toLowerCase())) tags.set(tag.toLowerCase(), tag);
  }
  return [...tags.values()];
}

/** True when `tag` is `filter` or nested below it (`a` matches `a/b`), ignoring case. */
export function tagMatches(tag: string, filter: string): boolean {
  const t = tag.toLowerCase();
  const f = filter.replace(/^#/, '').toLowerCase();
  return t === f || t.startsWith(f + '/');
}

/** `a/b/c` -> `['a', 'a/b', 'a/b/c']`. */
export function tagWithParents(tag: string): string[] {
  const parts = tag.split('/');
  return parts.map((_, i) => parts.slice(0, i + 1).join('/'));
}

/**
 * Note count per tag, case-insensitively; each tag is reported in the
 * spelling first seen. With `includeParents`, `a/b` also counts toward `a`.
 */
export function countTags(
  frontmatters: Iterable<Record<string, unknown>>,
  includeParents: boolean,
): Map<string, number> {
  const counts = new Map<string, number>();
  const spelling = new Map<string, string>();
  for (const fm of frontmatters) {
    const tags = new Set<string>();
    for (const t of noteTags(fm).flatMap((x) => (includeParents ? tagWithParents(x) : [x]))) {
      const key = t.toLowerCase();
      if (!spelling.has(key)) spelling.set(key, t);
      tags.add(key);
    }
    for (const key of tags) {
      const name = spelling.get(key)!;
      counts.set(name, (counts.get(name) ?? 0) + 1);
    }
  }
  return counts;
}

/** Count desc, then name asc. */
export function sortTagCounts(counts: Map<string, number>): Array<{ tag: string; count: number }> {
  return [...counts]
    .map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag, 'en'));
}
