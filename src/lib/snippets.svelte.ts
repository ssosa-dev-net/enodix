// Snippets as the interface shows them: the Snippets view's filter and its
// grouping. The snippets themselves come with the workspace; the core saves
// them and fills in their variables (`snippet_expand`). They are edited in a
// tab (SnippetEditor).

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

/**
 * The variables in snippet text, once each (ignoring case), in the order
 * they first appear, as first written: `{{name}}`, blanks around the name
 * ignored; an unclosed `{{`, `{{}}` or a name holding a brace or a line
 * break is not one. As the core reads them (`template.rs`).
 */
export function variablesOf(text: string): string[] {
  const names: string[] = [];
  let rest = text;
  for (let start = rest.indexOf("{{"); start >= 0; start = rest.indexOf("{{")) {
    const after = rest.slice(start + 2);
    const end = after.indexOf("}}");
    const name = end < 0 ? "" : after.slice(0, end).trim();
    if (name !== "" && !/[{}\r\n]/.test(name)) {
      if (!names.some((seen) => seen.toLowerCase() === name.toLowerCase()))
        names.push(name);
      rest = after.slice(end + 2);
    } else rest = rest.slice(start + 1);
  }
  return names;
}
