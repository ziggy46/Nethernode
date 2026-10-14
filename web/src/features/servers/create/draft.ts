import type { Platform } from "@/gen/nethernode/v1/catalog_pb";
import { CardTheme } from "@/gen/nethernode/v1/servers_pb";

export type Software = "vanilla" | "paper" | "purpur" | "fabric" | "quilt" | "forge" | "neoforge" | "modpack";

export interface PackChoice {
  platform: Platform;
  /** Project ID, or (CurseForge without a key) the slug. */
  project: string;
  title: string;
  iconUrl: string;
  /** Empty: the newest. */
  versionId: string;
  versionName: string;
  /** The pack's Minecraft version, when known. */
  mcVersion: string;
  /** The pack's mod loader, when known. */
  loader: string;
}

export type NetworkMode = "default" | "hostname" | "port";

export interface Draft {
  software: Software;
  mcVersion: string;
  /** Empty: let the image pick the newest build. */
  loaderVersion: string;
  snapshots: boolean;
  pack?: PackChoice;
  memoryMb: number;
  network: NetworkMode;
  hostname: string;
  port: string;
  name: string;
  theme: CardTheme;
  sleep: boolean;
  startNow: boolean;
  eula: boolean;
}

export const newDraft = (preset?: Partial<Draft>): Draft => ({
  software: "paper",
  mcVersion: "",
  loaderVersion: "",
  snapshots: false,
  memoryMb: 4096,
  network: "default",
  hostname: "",
  port: "",
  name: "",
  theme: CardTheme.NETHER,
  sleep: true,
  startNow: true,
  eula: false,
  ...preset,
});

export const SOFTWARE: { id: Software; name: string; blurb: string; memoryMb: number }[] = [
  { id: "vanilla", name: "Vanilla", blurb: "Mojang's own server, no mods or plugins.", memoryMb: 2048 },
  { id: "paper", name: "Paper", blurb: "Fast, with Bukkit and Paper plugins.", memoryMb: 4096 },
  { id: "fabric", name: "Fabric", blurb: "Lightweight mods, quick to update.", memoryMb: 4096 },
  { id: "forge", name: "Forge", blurb: "The classic loader for big mods.", memoryMb: 6144 },
  { id: "neoforge", name: "NeoForge", blurb: "Forge's modern successor.", memoryMb: 6144 },
  { id: "modpack", name: "Modpack", blurb: "A whole pack from Modrinth or CurseForge.", memoryMb: 6144 },
];

export const MORE_SOFTWARE: { id: Software; name: string; blurb: string; memoryMb: number }[] = [
  { id: "purpur", name: "Purpur", blurb: "Paper with extra options.", memoryMb: 4096 },
  { id: "quilt", name: "Quilt", blurb: "A Fabric fork; runs most Fabric mods.", memoryMb: 4096 },
];

/**
 * The DNS-safe slug the server gets from its name (as the server makes it;
 * a number is added if it's taken).
 */
export function slugPreview(name: string): string {
  let slug = "";
  for (const c of name.toLowerCase()) {
    if (/^[a-z0-9]$/.test(c)) slug += c;
    else if (slug && !slug.endsWith("-")) slug += "-";
    if (slug.length >= 32) break;
  }
  slug = slug.replace(/-+$/, "");
  return slug || "server";
}

/** A CurseForge slug from a slug or a modpack page URL. */
export function curseForgeSlug(input: string): string {
  const trimmed = input.trim();
  const match = trimmed.match(/curseforge\.com\/minecraft\/modpacks\/([a-z0-9_-]+)/i);
  return (match?.[1] ?? trimmed).toLowerCase();
}
