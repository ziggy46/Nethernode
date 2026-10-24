import type { Platform } from "@/gen/nethernode/v1/catalog_pb";
import { CardTheme } from "@/gen/nethernode/v1/servers_pb";

export type Software =
  | "vanilla"
  | "paper"
  | "purpur"
  | "fabric"
  | "quilt"
  | "forge"
  | "neoforge"
  | "modpack"
  | "bedrock";

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

/**
 * How players reach the server: a name on one of the admin's domains, a
 * full hostname of its own, a port of its own, or nothing yet.
 */
export type NetworkMode = "domain" | "hostname" | "port" | "later";

export interface Draft {
  software: Software;
  mcVersion: string;
  /** Empty: let the image pick the newest build. */
  loaderVersion: string;
  snapshots: boolean;
  pack?: PackChoice;
  memoryMb: number;
  network: NetworkMode;
  /** For "domain": the part before the domain; empty uses the server's name. */
  subdomain: string;
  /** For "domain": which domain; empty is the default (first) one. */
  domain: string;
  /** For "domain": the domain itself, with nothing in front. */
  apex: boolean;
  hostname: string;
  port: string;
  name: string;
  theme: CardTheme;
  sleep: boolean;
  startNow: boolean;
  eula: boolean;
  /** Which machine runs it ("local" for this one). */
  nodeId: string;
}

export const newDraft = (preset?: Partial<Draft>): Draft => ({
  software: "paper",
  mcVersion: "",
  loaderVersion: "",
  snapshots: false,
  memoryMb: 4096,
  network: "domain",
  subdomain: "",
  domain: "",
  apex: false,
  hostname: "",
  port: "",
  name: "",
  theme: CardTheme.NETHER,
  sleep: true,
  startNow: true,
  eula: false,
  nodeId: "local",
  ...preset,
});

export const SOFTWARE: { id: Software; name: string; blurb: string; memoryMb: number }[] = [
  { id: "vanilla", name: "Vanilla", blurb: "Mojang's own server, no mods or plugins.", memoryMb: 2048 },
  { id: "paper", name: "Paper", blurb: "Fast, with Bukkit and Paper plugins.", memoryMb: 4096 },
  { id: "fabric", name: "Fabric", blurb: "Lightweight mods, quick to update.", memoryMb: 4096 },
  { id: "forge", name: "Forge", blurb: "The classic loader for big mods.", memoryMb: 6144 },
  { id: "neoforge", name: "NeoForge", blurb: "Forge's modern successor.", memoryMb: 6144 },
  { id: "modpack", name: "Modpack", blurb: "A whole pack from Modrinth or CurseForge.", memoryMb: 6144 },
  {
    id: "bedrock",
    name: "Bedrock",
    blurb: "For Windows, phones and tablets. No Java mods.",
    memoryMb: 2048,
  },
];

export const MORE_SOFTWARE: { id: Software; name: string; blurb: string; memoryMb: number }[] = [
  { id: "purpur", name: "Purpur", blurb: "Paper with extra options.", memoryMb: 4096 },
  { id: "quilt", name: "Quilt", blurb: "A Fabric fork; runs most Fabric mods.", memoryMb: 4096 },
];

/** A kind of server software, from either list. */
export function softwareInfo(id: Software) {
  return [...SOFTWARE, ...MORE_SOFTWARE].find((s) => s.id === id);
}

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

/** The name players use for a "domain" address, or null without a domain. */
export function domainAddress(draft: Draft, domains: string[]): string | null {
  const domain = draft.domain || domains[0];
  if (!domain) return null;
  if (draft.apex) return domain;
  const sub =
    draft.subdomain.trim().toLowerCase() || slugPreview(draft.name || draft.pack?.title || "server");
  return `${sub}.${domain}`;
}

/** Why a subdomain someone typed won't work, if it won't. Empty is fine. */
export function subdomainError(sub: string): string | undefined {
  const value = sub.trim().toLowerCase();
  if (!value) return undefined;
  const label = /^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/;
  return value.split(".").every((l) => label.test(l))
    ? undefined
    : "Use letters, numbers, and hyphens (not at the start or end).";
}

/** A CurseForge slug from a slug or a modpack page URL. */
export function curseForgeSlug(input: string): string {
  const trimmed = input.trim();
  const match = trimmed.match(/curseforge\.com\/minecraft\/modpacks\/([a-z0-9_-]+)/i);
  return (match?.[1] ?? trimmed).toLowerCase();
}
