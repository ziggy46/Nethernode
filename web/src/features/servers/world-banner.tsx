import { CardTheme } from "@/gen/nethernode/v1/servers_pb";

type Theme = "nether" | "overworld" | "end" | "sky";

export const THEME_NAMES: Record<Exclude<CardTheme, CardTheme.UNSPECIFIED>, string> = {
  [CardTheme.NETHER]: "Nether",
  [CardTheme.OVERWORLD]: "Overworld",
  [CardTheme.END]: "The End",
  [CardTheme.SKY]: "Sky islands",
};

function themeKey(theme: CardTheme): Theme {
  switch (theme) {
    case CardTheme.OVERWORLD:
      return "overworld";
    case CardTheme.END:
      return "end";
    case CardTheme.SKY:
      return "sky";
    default:
      return "nether";
  }
}

/**
 * Geometric world artwork from the UI reference. Colors come from the
 * `--world-*` tokens. Purely decorative.
 */
export function WorldBanner({ theme }: { theme: CardTheme }) {
  const key = themeKey(theme);
  const c = (i: number) => `var(--world-${key}-${i})`;
  return (
    <svg viewBox="0 0 500 140" preserveAspectRatio="xMidYMid slice" aria-hidden className="block size-full">
      <rect width="500" height="140" fill={c(0)} />
      <path d="M0 70h40V46h60V64h42V31h54V54h72V24h50V60h51V39h65V65h66v75H0Z" fill={c(1)} />
      <path d="M0 103h50V85h62v22h70V71h61v23h53V77h56v25h77V82h71v58H0Z" fill={c(2)} />
      <path d="m70 80 57-27 57 27-57 28Z" fill={c(3)} />
      <path d="m70 80 57 28v40l-57-28Z" fill={c(1)} />
      <path d="m127 108 57-28v40l-57 28Z" fill={c(2)} />
      <path d="m293 99 80-38 79 38-79 38Z" fill={c(3)} />
      <path d="m293 99 80 38v20l-80-20Z" fill={c(1)} />
      <path d="m373 137 79-38v20l-79 38Z" fill={c(2)} />
      {key === "nether" && (
        <>
          <path d="m210 58 16-7 16 7-16 8Z" fill={c(5)} />
          <path d="m210 58 16 8v74h-16Z" fill={c(4)} />
          <path d="m226 66 16-8v82h-16Z" fill={c(6)} />
          <path d="M0 124h88v16H0m250 6 50-24h59l-50 24Z" fill={c(4)} opacity={0.65} />
          <rect x="421" y="22" width="3" height="3" fill={c(5)} />
          <rect x="66" y="24" width="2" height="2" fill={c(5)} />
        </>
      )}
      {key === "overworld" && (
        <>
          <path d="m119 61 8-5 8 5v26h-16Z" fill={c(6)} />
          <path d="m96 55 31-15 31 15-31 15Z" fill={c(7)} />
          <path d="m96 55 31 15v13L96 68Z" fill={c(8)} />
          <path d="m127 70 31-15v13l-31 15Z" fill={c(9)} />
        </>
      )}
      {key === "end" && (
        <>
          <path d="M350 72V18l12-6 12 6v55l-12 7Z" fill={c(6)} />
          <path d="m350 18 12-6 12 6-12 6Z" fill={c(4)} />
          <circle cx="83" cy="22" r="2" fill={c(4)} />
          <circle cx="228" cy="35" r="1.5" fill={c(4)} />
        </>
      )}
      {key === "sky" && (
        <path d="M40 27h45v9H40m8-17h20v8H48m269-3h64v10h-64m12-10h36v10h-36" fill={c(6)} opacity={0.3} />
      )}
      <path d="m373 69 18-8 17 8-17 8Z" fill={c(4)} opacity={0.4} />
    </svg>
  );
}
