---
title: Backups and schedules
description: Snapshots, restores, retention, and scheduled tasks.
---

## What a backup contains

A backup is the server's whole folder (worlds, mods, configuration,
player data) as a compressed `.tar.zst` archive, minus files the server
downloads again by itself (libraries, cached versions, logs). Backups are
stored in `<data>/backups` and listed on the **Backups** page and each
server's **Backups** tab.

Backing up a running server is safe and players stay connected: NetherNode
pauses world saving, flushes everything to disk, archives, and turns saving
back on.

## Restoring

**Restore** stops the server if it's running (players are disconnected),
makes a **Before restore** backup of what's there, swaps in the backup's
files, and starts the server again if you ask it to. If a restore isn't what
you wanted, restore the Before restore backup.

Backups of a server you delete are kept; download them, or delete them
yourself.

## Downloading

Admins can download any backup. Downloads resume if interrupted. To open
one: `tar --zstd -xf <file>.tar.zst`.

## Schedules

A schedule runs one action on a timetable in a time zone you choose
(daylight-saving changes are handled):

| Action | Notes |
| --- | --- |
| Back up | Optionally keep only the newest scheduled backup per day for *N* days and per week for *M* weeks. Manual backups are never deleted automatically. |
| Restart / Stop | Players get chat warnings beforehand (15, 10, 5 and 1 minutes, as you choose). |
| Send a message | Shown in chat to everyone online. |
| Run a command | Any console command; the reply is kept with the result. |
| Start | Useful with Stop for servers that should only run some hours. |

Pick **Every day**, **Every week** or **Every few hours**, or write a cron
expression (minute, hour, day, month, weekday). The editor shows the next
runs before you save.

If NetherNode wasn't running when a schedule was due, missed backups and
starts run when it comes back; missed restarts, stops, commands and messages
are skipped, so nobody is kicked without warning.
