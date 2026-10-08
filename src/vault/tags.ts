/**
 * Tag helpers over the indexed frontmatter JSON. A note's tags come from the
 * frontmatter `tags` / `tag` key (a list, or a comma- or space-separated
 * string, each entry with or without a leading `#`) plus the `inline_tags`
 * array the parser stores for body `#tags`.
 */

function splitTagValue(value: unknown): string[] {
  if (Array.isArray(value)) return value.flatMap(splitTagValue);
  if (typeof value !== 'string') return [];
  return value.split(/[,\s]+/);
}

/** Every distinct tag of one note, without the leading `#`. */
export function noteTags(frontmatter: Record<string, unknown>): string[] {
  const raw = [
    ...splitTagValue(frontmatter.tags),
    ...splitTagValue(frontmatter.tag),
    ...splitTagValue(frontmatter.inline_tags),
  ];
  const tags = new Set<string>();
  for (const t of raw) {
    const tag = t.replace(/^#/, '');
    if (tag) tags.add(tag);
  }
  return [...tags];
}

/** True when `tag` is `filter` or nested below it (`a` matches `a/b`). */
export function tagMatches(tag: string, filter: string): boolean {
  const f = filter.replace(/^#/, '');
  return tag === f || tag.startsWith(f + '/');
}

/** `a/b/c` -> `['a', 'a/b', 'a/b/c']`. */
export function tagWithParents(tag: string): string[] {
  const parts = tag.split('/');
  return parts.map((_, i) => parts.slice(0, i + 1).join('/'));
}

/** Note count per tag. With `includeParents`, `a/b` also counts toward `a`. */
export function countTags(
  frontmatters: Iterable<Record<string, unknown>>,
  includeParents: boolean,
): Map<string, number> {
  const counts = new Map<string, number>();
  for (const fm of frontmatters) {
    const tags = new Set(noteTags(fm).flatMap((t) => (includeParents ? tagWithParents(t) : [t])));
    for (const t of tags) counts.set(t, (counts.get(t) ?? 0) + 1);
  }
  return counts;
}

/** Count desc, then name asc. */
export function sortTagCounts(counts: Map<string, number>): Array<{ tag: string; count: number }> {
  return [...counts]
    .map(([tag, count]) => ({ tag, count }))
    .sort((a, b) => b.count - a.count || a.tag.localeCompare(b.tag, 'en'));
}
