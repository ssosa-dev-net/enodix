<script lang="ts">
  import type { Role, RoleIcon } from "./workspace";

  interface Props {
    role: Role;
  }

  let { role }: Props = $props();

  // Each icon on a 16 × 16 grid, drawn with strokes in the role's colour.
  const SHAPES: Record<RoleIcon, string> = {
    core: '<circle cx="8" cy="8" r="6"/><circle cx="8" cy="8" r="2.5"/>',
    distribution:
      '<circle cx="8" cy="3.5" r="1.8"/><circle cx="3.5" cy="12.5" r="1.8"/><circle cx="12.5" cy="12.5" r="1.8"/><path d="M7 5 4.5 11M9 5l2.5 6"/>',
    access:
      '<rect x="1.5" y="5" width="13" height="6" rx="1"/><path d="M4 8h.01M6.7 8h.01M9.3 8h.01M12 8h.01"/>',
    router:
      '<circle cx="8" cy="8" r="6.5"/><path d="M8 3.5v9M3.5 8h9M6.5 5 8 3.5 9.5 5M6.5 11 8 12.5 9.5 11"/>',
    firewall:
      '<rect x="1.5" y="2.5" width="13" height="11" rx="1"/><path d="M1.5 6.2h13M1.5 9.8h13M6 2.5v3.7M10 6.2v3.6M6 9.8v3.7"/>',
    load_balancer:
      '<circle cx="3" cy="8" r="1.6"/><path d="M4.6 8h3M7.6 8l5-4.5M7.6 8h5M7.6 8l5 4.5"/>',
    wireless_controller:
      '<rect x="2" y="9" width="12" height="5" rx="1"/><path d="M5 6a4.2 4.2 0 0 1 6 0M6.6 7.4a2 2 0 0 1 2.8 0"/>',
    access_point:
      '<path d="M2.5 6a7.8 7.8 0 0 1 11 0M4.6 8.2a4.8 4.8 0 0 1 6.8 0M6.7 10.4a1.8 1.8 0 0 1 2.6 0"/><circle cx="8" cy="13" r=".6"/>',
    console_server:
      '<rect x="1.5" y="2.5" width="13" height="11" rx="1"/><path d="m4.5 6 2.5 2-2.5 2M8.5 10.5h3"/>',
    server:
      '<rect x="2.5" y="1.5" width="11" height="4" rx=".8"/><rect x="2.5" y="6" width="11" height="4" rx=".8"/><rect x="2.5" y="10.5" width="11" height="4" rx=".8"/><path d="M5 3.5h.01M5 8h.01M5 12.5h.01"/>',
    switch:
      '<rect x="1.5" y="3.5" width="13" height="9" rx="1"/><path d="M4.5 6.5h7l-1.5-1.3M11.5 9.5h-7L6 10.8"/>',
    cloud:
      '<path d="M4.5 12.5a3 3 0 0 1-.4-6 4 4 0 0 1 7.6-.8 3.4 3.4 0 0 1 .3 6.8z"/>',
    storage:
      '<ellipse cx="8" cy="3.5" rx="5.5" ry="2"/><path d="M2.5 3.5v9c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2v-9M2.5 8c0 1.1 2.5 2 5.5 2s5.5-.9 5.5-2"/>',
    generic: '<rect x="2.5" y="2.5" width="11" height="11" rx="2.5"/>',
    unknown:
      '<circle cx="8" cy="8" r="6" stroke-dasharray="2 1.6"/><path d="M6.4 6.4a1.7 1.7 0 1 1 2.3 1.6c-.5.2-.7.6-.7 1.1M8 11.3h.01"/>',
  };
</script>

<svg
  class="role-icon"
  viewBox="0 0 16 16"
  fill="none"
  stroke="currentColor"
  stroke-width="1.4"
  stroke-linecap="round"
  stroke-linejoin="round"
  style:color={role.color}
  data-role-icon={role.icon}
  data-role-color={role.color}
  role="img"
  aria-label="{role.name} role"
>
  <title>{role.name}</title>
  <!-- Fixed markup from SHAPES above, never from a file. -->
  {@html SHAPES[role.icon]}
</svg>

<style>
  .role-icon {
    width: 1em;
    height: 1em;
    flex: none;
    align-self: center;
  }
</style>
