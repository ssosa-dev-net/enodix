<script lang="ts">
  // The handle on a side panel's inner edge: drag it to change the panel's
  // width (saved when the drag ends), double-click it for the default
  // width, or use the arrow keys when it has focus. Its button folds the
  // panel away.

  interface Props {
    side: "left" | "right";
    /** The width shown now, in pixels. */
    width: number;
    min: number;
    max: number;
    /** While dragging: the width to show, not saved yet. */
    onlive: (width: number | null) => void;
    /** The width to keep. */
    oncommit: (width: number) => void;
    onreset: () => void;
    onhide: () => void;
  }

  let { side, width, min, max, onlive, oncommit, onreset, onhide }: Props =
    $props();

  let drag: { x: number; width: number; last: number } | null = null;
  const KEY_STEP = 16;

  const clamp = (value: number) =>
    Math.round(Math.min(Math.max(value, min), Math.max(min, max)));

  function onpointerdown(event: PointerEvent) {
    if (event.button !== 0 || event.target !== event.currentTarget) return;
    event.preventDefault();
    (event.currentTarget as HTMLElement).setPointerCapture(event.pointerId);
    drag = { x: event.clientX, width, last: width };
  }

  function onpointermove(event: PointerEvent) {
    if (!drag) return;
    const moved = event.clientX - drag.x;
    drag.last = clamp(drag.width + (side === "left" ? moved : -moved));
    onlive(drag.last);
  }

  function onpointerup() {
    if (!drag) return;
    const { last, width: start } = drag;
    drag = null;
    onlive(null);
    if (last !== start) oncommit(last);
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
    event.preventDefault();
    const wider = (event.key === "ArrowRight") === (side === "left");
    oncommit(clamp(width + (wider ? KEY_STEP : -KEY_STEP)));
  }
</script>

<!-- A focusable separator with a value is ARIA's window splitter, which
     takes pointer and arrow-key input; Svelte's check does not know it. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="resizer {side}"
  role="separator"
  aria-orientation="vertical"
  aria-label={side === "left"
    ? "Resize the workspace panel"
    : "Resize the details panel"}
  aria-valuenow={width}
  aria-valuemin={min}
  aria-valuemax={max}
  tabindex="0"
  title="Drag to resize, double-click for the default width"
  data-panel-resizer={side}
  {onpointerdown}
  {onpointermove}
  {onpointerup}
  onpointercancel={onpointerup}
  ondblclick={onreset}
  {onkeydown}
>
  <button
    class="hide"
    data-hide-panel={side}
    title={side === "left"
      ? "Hide the workspace panel (Ctrl+Shift+B)"
      : "Hide the details panel (Ctrl+Shift+D)"}
    aria-label={side === "left"
      ? "Hide the workspace panel"
      : "Hide the details panel"}
    onmousedown={(event) => event.preventDefault()}
    onclick={onhide}>{side === "left" ? "‹" : "›"}</button
  >
</div>

<style>
  .resizer {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 7px;
    margin-left: -3px;
    cursor: col-resize;
    z-index: 5;
    touch-action: none;
  }

  .resizer:hover,
  .resizer:focus-visible {
    background: color-mix(in srgb, var(--accent) 45%, transparent);
    outline: none;
  }

  .hide {
    position: absolute;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    width: 14px;
    height: 36px;
    padding: 0;
    font-size: var(--text-xs);
    line-height: 1;
    border-radius: var(--radius-sm);
    opacity: 0;
    cursor: pointer;
  }

  .resizer:hover .hide,
  .resizer:focus-visible .hide,
  .hide:focus-visible {
    opacity: 1;
  }
</style>
