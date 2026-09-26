// Times as a person reads them, in this browser's time zone.

/** "in 3 h", "5 min ago", "just now". */
export function relative(iso: string, now: number = Date.now()): string {
  const diff = Date.parse(iso) - now;
  const future = diff > 0;
  const minutes = Math.round(Math.abs(diff) / 60_000);
  if (minutes < 1) return future ? 'in under a minute' : 'just now';
  let amount: string;
  if (minutes < 60) amount = `${minutes} min`;
  else if (minutes < 48 * 60) amount = `${Math.round(minutes / 60)} h`;
  else amount = `${Math.round(minutes / (24 * 60))} days`;
  return future ? `in ${amount}` : `${amount} ago`;
}

/** "Sat 26 Sep, 07:30 (in 3 h)" */
export function when(iso: string | null | undefined, now: number = Date.now()): string {
  if (!iso) return '—';
  const date = new Date(iso);
  const text = date.toLocaleString(undefined, {
    weekday: 'short',
    day: 'numeric',
    month: 'short',
    hour: '2-digit',
    minute: '2-digit',
  });
  return `${text} (${relative(iso, now)})`;
}

export function bytes(n: number | null | undefined): string {
  if (n === null || n === undefined) return '—';
  return n >= 1024 * 1024 ? `${(n / 1024 / 1024).toFixed(1)} MiB` : `${Math.round(n / 1024)} KiB`;
}

export function duration(ms: number): string {
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 48) return `${hours} h ${minutes % 60} min`;
  return `${Math.floor(hours / 24)} days ${hours % 24} h`;
}
