/** A timestamp as people read it: "3 Oct 2026, 14:05". */
export function when(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' });
}

/** A fraction as a percentage: 0.4231 -> "42%". */
export function percent(fraction: number | null | undefined): string {
  return fraction === null || fraction === undefined ? 'unknown' : `${Math.round(fraction * 100)}%`;
}
