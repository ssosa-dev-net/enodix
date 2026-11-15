// Snippets as the interface shows them: the Snippets view's filter and its
// grouping. The snippets themselves come with the workspace; the core saves
// them and fills in their variables (`snippet_expand`). The view's form is
// its own state (SnippetsView).

import type { Snippet } from "./workspace";

export const snippets = $state({
  /** The Snippets view's filter, kept while other views are shown. */
  query: "",
});

/** The snippets by group, in the order each group first appears; the
 * snippets without a group first. */
export function bySnippetGroup(
  list: Snippet[],
): { group: string | null; snippets: Snippet[] }[] {
  const groups = new Map<string | null, Snippet[]>([[null, []]]);
  for (const snippet of list) {
    const group = groups.get(snippet.group);
    if (group) group.push(snippet);
    else groups.set(snippet.group, [snippet]);
  }
  return [...groups]
    .filter(([, members]) => members.length > 0)
    .map(([group, members]) => ({ group, snippets: members }));
}
