// No writing suggestions in any field (#115). While text is typed into a
// field, WebView2 (Edge's text predictions) connects to Microsoft's
// servers, to suggest how the text goes on; in a terminal that would be
// whatever is typed to a device. `writingsuggestions="false"` on the field
// itself stops it (on an ancestor it did not always). Every field and
// editable element gets it, those made later too (xterm.js makes its own
// textarea), in every window.

const EDITABLE = "input, textarea, [contenteditable]";

function mark(element: Element) {
  if (element.getAttribute("writingsuggestions") !== "false")
    element.setAttribute("writingsuggestions", "false");
}

function markWithin(node: Node) {
  if (!(node instanceof Element)) return;
  if (node.matches(EDITABLE)) mark(node);
  if (node.firstElementChild) node.querySelectorAll(EDITABLE).forEach(mark);
}

/** Marks the document's fields now and as they are added. */
export function noWritingSuggestions(
  root: HTMLElement = document.documentElement,
) {
  mark(root);
  markWithin(root);
  new MutationObserver((changes) => {
    for (const change of changes) change.addedNodes.forEach(markWithin);
  }).observe(root, { childList: true, subtree: true });
}
