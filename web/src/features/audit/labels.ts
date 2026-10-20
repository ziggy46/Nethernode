/** Plain-language names for audit actions. */
const LABELS: Record<string, string> = {
  "auth.setup": "Created the owner account",
  "auth.login": "Signed in",
  "auth.login_failed": "Failed to sign in",
  "auth.logout": "Signed out",
  "auth.change_password": "Changed their password",
  "auth.totp_enabled": "Turned on two-factor sign-in",
  "auth.totp_disabled": "Turned off two-factor sign-in",
  "auth.reset_password": "Reset an owner password from the command line",
  "user.create": "Added a user",
  "user.role": "Changed a role",
  "user.reset_password": "Reset a password",
  "user.reset_totp": "Turned off someone's two-factor sign-in",
  "user.revoke_sessions": "Signed someone out everywhere",
  "user.delete": "Deleted a user",
  "server.create": "Created a server",
  "server.delete": "Deleted a server",
  "server.start": "Started a server",
  "server.stop": "Stopped a server",
  "server.restart": "Restarted a server",
  "server.kill": "Force-stopped a server",
  "server.update": "Changed server settings",
  "server.allowlist_enabled": "Changed the allowlist setting",
  "server.properties": "Changed game settings",
  "server.move": "Moved a server to another machine",
  "server.copy": "Copied a server",
  "server.template_save": "Saved a server as a template",
  "server.template_delete": "Deleted a server template",
  "server.create_from_template": "Created a server from a template",
  "node.create": "Added a node",
  "node.join": "A node's agent joined",
  "node.join_failed": "An agent failed to join",
  "node.join_code": "Made a new join code",
  "node.rename": "Renamed a node",
  "node.delete": "Removed a node",
  "node.update": "Updated a node",
  "node.auto_update": "Changed automatic node updates",
  "player.ban": "Banned a player",
  "player.unban": "Unbanned a player",
  "player.ban_expired": "A ban expired",
  "player.kick": "Kicked a player",
  "player.op": "Made a player an operator",
  "player.deop": "Removed an operator",
  "player.allowlist_add": "Allowlisted a player",
  "player.allowlist_remove": "Removed a player from the allowlist",
  "player.allowlist_import": "Imported an allowlist",
  "player.gamemode": "Changed a game mode",
  "player.give": "Gave a player items",
  "player.teleport": "Teleported a player",
  "player.notes": "Edited player notes",
  "file.write": "Edited a file",
  "file.upload": "Uploaded a file",
  "file.delete": "Deleted files",
  "file.restore": "Restored files from the trash",
  "file.purge": "Emptied the trash",
  "file.extract": "Extracted an archive",
  "file.compress": "Compressed files",
  "file.rename": "Renamed a file",
  "file.mkdir": "Created a folder",
  "file.copy": "Copied files",
  "mod.install": "Installed a mod",
  "mod.update": "Updated a mod",
  "mod.enable": "Turned on a mod",
  "mod.disable": "Turned off a mod",
  "mod.delete": "Removed a mod",
  "modpack.update": "Updated a modpack",
  "backup.create": "Backed up a server",
  "backup.restore": "Restored a backup",
  "backup.delete": "Deleted a backup",
  "backup.prune": "Removed old backups",
  "schedule.create": "Added a schedule",
  "schedule.update": "Changed a schedule",
  "schedule.delete": "Deleted a schedule",
  "schedule.run_now": "Ran a schedule by hand",
  "schedule.run": "Ran a schedule",
  "proxy.listener.upsert": "Saved a proxy listener",
  "proxy.server_port.add": "Gave a server its own port",
  "proxy.listener.delete": "Deleted a proxy listener",
  "proxy.route.upsert": "Saved a route",
  "proxy.route.delete": "Deleted a route",
  "proxy.domains": "Changed the domains",
  "settings.curseforge_key": "Changed the CurseForge key",
  "settings.cache_clear": "Cleared downloaded files",
  "settings.notifications": "Changed Discord notifications",
  "settings.install_count": "Changed whether this install is counted",
  "settings.update_check": "Changed checking for new versions",
  "settings.update_channel": "Changed the update channel",
  "settings.update_panel": "Updated NetherNode",
};

export function actionLabel(action: string): string {
  return LABELS[action] ?? action;
}

export const CATEGORIES: { value: string; label: string }[] = [
  { value: "", label: "Everything" },
  { value: "auth", label: "Sign-ins" },
  { value: "user", label: "Users" },
  { value: "server", label: "Servers" },
  { value: "player", label: "Players" },
  { value: "file", label: "Files" },
  { value: "mod", label: "Mods" },
  { value: "backup", label: "Backups" },
  { value: "schedule", label: "Schedules" },
  { value: "proxy", label: "Proxy" },
  { value: "node", label: "Nodes" },
  { value: "settings", label: "Settings" },
];

/** Detail keys not worth repeating (they're shown elsewhere in the row). */
const HIDDEN = new Set(["name", "username", "server_id", "schedule_id"]);

/** "reason: griefing · expires_at: …" from an entry's details. */
export function detailPairs(json: string): [string, string][] {
  if (!json) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    return [];
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return [];
  return Object.entries(parsed as Record<string, unknown>)
    .filter(([key, value]) => !HIDDEN.has(key) && value !== null && value !== "" && value !== undefined)
    .map(([key, value]) => {
      const text =
        typeof value === "string"
          ? value
          : Array.isArray(value)
            ? value.map((v) => (typeof v === "string" ? v : JSON.stringify(v))).join(", ")
            : JSON.stringify(value);
      return [key.replaceAll("_", " "), text.length > 160 ? `${text.slice(0, 160)}…` : text];
    });
}

/** Dashboard wording: "Nether SMP started". */
export function activityTitle(action: string, server: string): string {
  const s = server || "A server";
  switch (action) {
    case "server.create":
      return `${s} was created`;
    case "server.delete":
      return `${s} was deleted`;
    case "server.copy":
      return `${s} was copied`;
    case "server.start":
      return `${s} started`;
    case "server.stop":
      return `${s} stopped`;
    case "server.restart":
      return `${s} restarted`;
    case "server.kill":
      return `${s} was force-stopped`;
    case "backup.create":
      return `${s} backup started`;
    case "backup.restore":
      return `${s} was restored from a backup`;
    case "schedule.run":
      return `Scheduled task on ${s}`;
    case "player.ban":
      return `Player banned on ${s}`;
    case "player.unban":
      return `Player unbanned on ${s}`;
    case "player.kick":
      return `Player kicked from ${s}`;
    case "mod.install":
      return `Mod installed on ${s}`;
    case "mod.update":
      return `Mod updated on ${s}`;
    default:
      return actionLabel(action);
  }
}
