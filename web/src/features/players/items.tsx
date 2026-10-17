import { queryOptions } from "@tanstack/react-query";
import {
  Apple,
  Axe,
  Book,
  Box,
  FlaskConical,
  Gem,
  Hammer,
  type LucideIcon,
  Pickaxe,
  Shield,
  Shirt,
  Sword,
  Wand,
} from "lucide-react";
import { createContext, useContext, useId } from "react";

import type { ItemStack } from "@/gen/nethernode/v1/players_pb";
import { cn } from "@/lib/utils";

/** How to draw an item, from the server's Minecraft version (`/api/servers/{id}/items`). */
export type ItemIcon =
  | { kind: "flat"; texture: string; tint?: string }
  | { kind: "cube"; top: string; side: string; tint?: string };

export interface ItemIcons {
  version: string;
  icons: Record<string, ItemIcon>;
}

/** Icons for the items in `ids`. The panel downloads the version's textures from Mojang the first time. */
export const itemIconsQuery = (serverId: string, ids: string[]) =>
  queryOptions({
    queryKey: ["items", serverId, ids.slice().sort().join(",")],
    queryFn: async (): Promise<ItemIcons> => {
      const response = await fetch(`/api/servers/${serverId}/items?ids=${encodeURIComponent(ids.join(","))}`);
      if (!response.ok) throw new Error(`item icons: ${response.status}`);
      return (await response.json()) as ItemIcons;
    },
    enabled: ids.length > 0,
    staleTime: Number.POSITIVE_INFINITY,
    retry: 1,
  });

/** Icons for the slots below; slots without one fall back to a symbol. */
export const ItemIconsContext = createContext<ItemIcons | undefined>(undefined);

const textureUrl = (version: string, path: string) =>
  `/api/minecraft/${encodeURIComponent(version)}/textures/${path}.png`;

/**
 * One texture face, cropped to its first 16×16 frame (animated textures are
 * tall strips) and drawn as pixel art.
 */
function Face({
  href,
  transform,
  shade,
  filter,
}: {
  href: string;
  transform: string;
  shade?: number;
  filter?: string;
}) {
  return (
    <g transform={transform} filter={filter}>
      <image
        href={href}
        width="16"
        height="16"
        preserveAspectRatio="xMidYMin slice"
        style={{ imageRendering: "pixelated" }}
      />
      {shade ? <rect width="16" height="16" fill="black" opacity={shade} /> : null}
    </g>
  );
}

/** An item as the game draws it in inventories: flat, or a small isometric cube. */
function GameIcon({ icon, version }: { icon: ItemIcon; version: string }) {
  const tintId = useId();
  // Grass and leaves are grey until the game colours them.
  const tint = icon.tint && (
    <defs>
      <filter id={tintId}>
        <feFlood floodColor={icon.tint} result="tint" />
        <feComposite in="tint" in2="SourceAlpha" operator="in" result="tinted" />
        <feBlend in="SourceGraphic" in2="tinted" mode="multiply" />
      </filter>
    </defs>
  );
  const filter = icon.tint ? `url(#${tintId})` : undefined;
  if (icon.kind === "flat") {
    return (
      <svg viewBox="0 0 16 16" className="size-[78%]" aria-hidden>
        {tint}
        <Face href={textureUrl(version, icon.texture)} transform="" filter={filter} />
      </svg>
    );
  }
  const top = textureUrl(version, icon.top);
  const side = textureUrl(version, icon.side);
  // Leaves are one texture all round, so every face is coloured; a grass
  // block's sides are already green.
  const sideFilter = icon.top === icon.side ? filter : undefined;
  return (
    <svg viewBox="0 0 32 32" className="size-[82%]" aria-hidden>
      {tint}
      <Face href={top} transform="matrix(1,0.5,-1,0.5,16,0)" filter={filter} />
      <Face href={side} transform="matrix(1,0.5,0,1,0,8)" shade={0.25} filter={sideFilter} />
      <Face href={side} transform="matrix(1,-0.5,0,1,16,16)" shade={0.45} filter={sideFilter} />
    </svg>
  );
}

/** `minecraft:diamond_sword` → `Diamond Sword`. */
export function itemName(id: string): string {
  const bare = id.includes(":") ? id.slice(id.indexOf(":") + 1) : id;
  return bare
    .split("_")
    .filter(Boolean)
    .map((w) => w[0]?.toUpperCase() + w.slice(1))
    .join(" ");
}

const ICONS: [RegExp, LucideIcon][] = [
  [/sword/, Sword],
  [/pickaxe/, Pickaxe],
  [/_axe$/, Axe],
  [/shovel|_hoe$|mace/, Hammer],
  [/helmet|chestplate|leggings|boots|elytra/, Shirt],
  [/shield/, Shield],
  [/diamond|emerald|amethyst|quartz|lapis|redstone|netherite_ingot|_ingot|nugget/, Gem],
  [/apple|bread|beef|pork|chicken|mutton|cod|salmon|carrot|potato|stew|pie|cookie|melon|berries/, Apple],
  [/potion/, FlaskConical],
  [/book/, Book],
  [/bow|trident|rod|stick|blaze/, Wand],
];

function iconFor(id: string): LucideIcon {
  return ICONS.find(([pattern]) => pattern.test(id))?.[1] ?? Box;
}

/** One inventory slot: the item's real icon when it's known, otherwise a symbol by type; its name on hover. */
export function Slot({ item, label, selected }: { item?: ItemStack; label?: string; selected?: boolean }) {
  const icons = useContext(ItemIconsContext);
  const game = item && icons ? icons.icons[item.id] : undefined;
  const Icon = item ? iconFor(item.id) : null;
  const title = item
    ? [
        `${item.customName || itemName(item.id)}${Number(item.count) > 1 ? ` ×${item.count}` : ""}`,
        item.customName ? itemName(item.id) : "",
        ...item.enchantments.map((e) => `${itemName(e.id)} ${e.level}`),
        Number(item.damage) > 0 ? `Damage ${item.damage}` : "",
      ]
        .filter(Boolean)
        .join("\n")
    : label;
  return (
    <div
      title={title}
      role="img"
      aria-label={title || "Empty slot"}
      className={cn(
        "relative grid aspect-square place-items-center rounded-xs border border-border bg-console",
        selected && "border-primary",
        item && item.enchantments.length > 0 && "shadow-[inset_0_0_0_1px_var(--color-info)]",
      )}
    >
      {game && icons ? (
        <GameIcon icon={game} version={icons.version} />
      ) : Icon ? (
        <Icon className="size-[55%] text-text-secondary" aria-hidden />
      ) : (
        label && <span className="text-[9px] text-text-muted uppercase">{label}</span>
      )}
      {item && Number(item.count) > 1 && (
        <span className="absolute right-0.5 bottom-0 font-semibold text-[10px] text-text-primary tabular-nums">
          {String(item.count)}
        </span>
      )}
    </div>
  );
}

/** Readable stat names: `minecraft:walk_one_cm` → `Walk one cm`. */
export function statName(key: string): string {
  const name = itemName(key).toLowerCase();
  return name.charAt(0).toUpperCase() + name.slice(1);
}
