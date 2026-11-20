// How many transfers each host's Files tab has running or waiting (#106),
// so that closing the tab, switching workspace or closing the app asks
// first. Each FilesTab keeps its own host's count.

const active = $state<Record<string, number>>({});

/** Sets `host`'s count (0 forgets it). */
export function setActiveTransfers(host: string, count: number) {
  if (count > 0) active[host] = count;
  else delete active[host];
}

/** `host`'s running and waiting transfers, or every host's. */
export function activeTransfers(host?: string): number {
  if (host !== undefined) return active[host] ?? 0;
  return Object.values(active).reduce((sum, count) => sum + count, 0);
}

/** "1 transfer", "3 transfers". */
export function transfersText(count: number): string {
  return count === 1 ? "1 transfer" : `${count} transfers`;
}
