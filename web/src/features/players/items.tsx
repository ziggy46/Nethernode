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

import type { ItemStack } from "@/gen/nethernode/v1/players_pb";
import { cn } from "@/lib/utils";

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

/** One inventory slot. Item textures aren't shipped, so items show as an icon by type and their name on hover. */
export function Slot({ item, label, selected }: { item?: ItemStack; label?: string; selected?: boolean }) {
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
      {Icon ? (
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
