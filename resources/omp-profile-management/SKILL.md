---
name: omp-profile-management
description: Manage user-owned OMP profiles safely through UltraTerm's local UTP service.
---

# OMP profile management

Use the app-owned UTP CLI first:

```sh
$HOME/.ultraterm/bin/utp profiles list
$HOME/.ultraterm/bin/utp profiles create <name> <model-id> <thinking-level> [--title-model <model-id>]
$HOME/.ultraterm/bin/utp profiles remove <name>
```

The CLI validates names and model settings, keeps profile operations inside the
user's OMP profile root, and refuses to remove an active profile. Valid thinking
levels are `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`, and `auto`.
Profile names are lowercase ASCII letters and digits with internal hyphens, up
to 48 characters.

Removal requires explicit user authorization naming the exact profile. Do not
infer authorization from context, and never fall back to a direct `rm` command.
Creation and removal must use UTP.

## Direct editing boundaries

After listing and confirming the intended target, direct editing may change only
that profile's configuration file under `~/.omp/profiles/<name>/agent/config.yml`.
Do not edit or remove `.env` files, databases, transcripts, blobs, global OMP
configuration, or shared asset targets. Never follow a profile-directory
symlink or use `..`, separators, control characters, or shell expansion in a
profile name.

Do not directly create or remove profile directories. Use UTP so active
`@omp-profile` metadata is checked immediately before removal. User edits are
never overwritten by UltraTerm's installed skill.
