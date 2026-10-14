---
title: Users and sign-in
description: Roles, one-time passwords, two-factor sign-in, and sessions.
---

## Roles

| Role | Can |
| --- | --- |
| Viewer | See servers, players, backups and the dashboard |
| Moderator | …and use the console and manage players |
| Administrator | …and manage servers, files, mods, backups, schedules, the proxy, settings and notifications; read the audit log |
| Owner | …and manage users. There is always at least one owner. |

## Adding people

Owners add people on the **Users** page. NetherNode makes a one-time
password, shown once; share it privately. The person signs in with it and
must choose their own password before anything else.

From the same page owners can change roles, reset a password (which signs
the person out everywhere), turn off someone's two-factor sign-in (for a
lost phone), sign them out everywhere, or delete the account.

## Two-factor sign-in

Anyone can turn it on in **Settings › Sign-in and security**: scan the QR
code with an authenticator app (1Password, Google Authenticator, Aegis…),
enter a code, and save the ten recovery codes. Each recovery code works
once, for when the phone isn't available. A code from the app can't be
reused, and repeated wrong codes are slowed down like wrong passwords.

## Sessions

Signing in lasts 7 days without activity and at most 30 days. The same
settings page lists your signed-in devices and can sign them out. Changing
your password signs out your other devices.

## Locked out

If the only owner has lost their password and phone and recovery codes, run
on the machine:

```sh
nethernode reset-admin                     # or: --username NAME
docker compose exec nethernode /nethernode reset-admin   # in Docker
```

It prints a one-time password, turns off two-factor sign-in for that
account, and signs it out everywhere.
