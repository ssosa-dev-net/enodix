// Snippets as the interface shows them: the editor dialog's state and the
// panel's grouping. The snippets themselves come with the workspace; the
// core saves them and fills in their variables (`snippet_expand`).

import type { Snippet } from "./workspace";

export const snippets = $state({
  /** The snippet in the editor (`previous`: its name; null for a new one). */
  editing: null as { previous: string | null; snippet: Snippet } | null,
});

export function newSnippet() {
  snippets.editing = {
    previous: null,
    snippet: { name: "", group: null, text: "" },
  };
}

export function editSnippet(snippet: Snippet) {
  snippets.editing = { previous: snippet.name, snippet: { ...snippet } };
}

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
