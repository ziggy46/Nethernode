const MIB = 1024 * 1024;
const GIB = 1024 * MIB;

/** `512 KB`, `48.2 MB`, `4.70 GB`. */
export function bytes(n: number): string {
  if (n >= GIB) return `${(n / GIB).toFixed(2)} GB`;
  if (n >= MIB) return `${(n / MIB).toFixed(1)} MB`;
  return `${Math.max(0, Math.round(n / 1024))} KB`;
}
