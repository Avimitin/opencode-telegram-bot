# Deployment Guide

This bot runs as a systemd service managed by a NixOS module. You don't need a full NixOS system — the module can be evaluated standalone to generate a systemd unit file for any Linux machine with nix installed.

## Prerequisites

- A Telegram bot token (from [@BotFather](https://t.me/BotFather))
- An API key for your LLM provider (e.g. Zhipu, OpenAI, Anthropic)
- [Nix](https://nixos.org/download) installed (with flakes enabled)

## Option A: NixOS

Add the bot's flake as an input and import the module:

```nix
# flake.nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    opencode-telegram-bot.url = "github:Avimitin/opencode-telegram-bot";
  };

  outputs = { nixpkgs, opencode-telegram-bot, ... }: {
    nixosConfigurations.myhost = nixpkgs.lib.nixosSystem {
      modules = [
        opencode-telegram-bot.nixosModules.default
        ./configuration.nix
      ];
    };
  };
}
```

Then configure in your `configuration.nix`:

```nix
{ lib, ... }: {
  services.opencode-telegram = {
    enable = true;

    # Where the bot stores its state (sessions, database, access config)
    stateDir = "/var/lib/opencode-telegram";

    # Telegram bot token — point to a file, not the token itself!
    # Use agenix or sops-nix for secret management.
    botTokenFile = "/run/secrets/telegram-bot-token";

    # API keys — a file with KEY=VALUE lines, loaded as systemd EnvironmentFile
    environmentFile = "/run/secrets/opencode-env";

    # opencode configuration — becomes opencode.json
    settings = {
      model = "zhipu/glm-4-plus";
      provider.zhipu = {
        name = "Zhipu AI";
        api = "openai";
        url = "https://open.z.ai/api/paas/v4";
        models."glm-4-plus" = {
          name = "GLM-4-Plus";
          attachment = true;
        };
      };
      permission."*" = "allow";
    };

    # Who can talk to the bot
    accessConfig = {
      dmPolicy = "pairing";  # New DM users get a pairing code
      allowFrom = [];         # Pre-approved user IDs
      groups = {
        "-1001234567890" = {
          requireMention = true;  # Only respond when @mentioned
          allowFrom = [];         # Empty = anyone in the group
        };
      };
      pending = {};
      mentionPatterns = [ "@MyBotName" ];  # The bot also auto-detects its own @username
    };

    # If your server needs a proxy to reach Telegram/LLM APIs
    extraEnvironment = {
      https_proxy = "http://proxy:8080";
      http_proxy = "http://proxy:8080";
      no_proxy = "127.0.0.1,localhost";  # Important! The bot talks to a local opencode server
    };
  };
}
```

Then `nixos-rebuild switch` and you're done.

### Module Options Reference

| Option | Type | Default | Description |
|---|---|---|---|
| `enable` | bool | `false` | Enable the bot service |
| `package` | package | built from source | The bot package (override to pin a version) |
| `stateDir` | path | `/var/lib/opencode-telegram` | State directory for all runtime data |
| `user` / `group` | string | `"opencode-telegram"` | System user/group (auto-created) |
| `botTokenFile` | path | (required) | File containing the Telegram bot token |
| `environmentFile` | path | `null` | Systemd EnvironmentFile for API keys |
| `settings` | attrs | `{}` | Nix attrs serialized to opencode.json |
| `accessConfig` | attrs | default pairing | Access control configuration |
| `extraEnvironment` | attrs | `{}` | Extra environment variables (proxy, etc.) |

## Option B: Non-NixOS (with nix installed)

Create a deploy flake that evaluates the NixOS module and extracts the systemd unit:

```nix
# deploy/flake.nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    opencode-telegram-bot.url = "github:Avimitin/opencode-telegram-bot";
  };

  outputs = { nixpkgs, opencode-telegram-bot, ... }: {
    nixosConfigurations.eval = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        opencode-telegram-bot.nixosModules.default
        ({ lib, ... }: {
          # Minimal NixOS config for evaluation
          system.stateVersion = "24.11";
          boot.loader.grub.device = "nodev";
          fileSystems."/" = { device = "none"; fsType = "tmpfs"; };

          services.opencode-telegram = {
            enable = true;
            package = opencode-telegram-bot.packages.x86_64-linux.default;
            # ... same options as above ...
          };

          # If running as root, disable home protection
          systemd.services.opencode-telegram.serviceConfig = {
            ProtectHome = lib.mkForce "no";
            ProtectSystem = lib.mkForce "no";
          };
        })
      ];
    };
  };
}
```

Then deploy with:

```bash
#!/bin/bash
# deploy.sh
UNIT_PATH=$(nix build \
  .#nixosConfigurations.eval.config.systemd.units.'"opencode-telegram.service"'.unit \
  --no-link --print-out-paths)

cp "${UNIT_PATH}/opencode-telegram.service" /etc/systemd/system/opencode-telegram-bot.service
systemctl daemon-reload
systemctl restart opencode-telegram-bot
```

`nix build` on the unit derivation builds everything — the bot package, opencode binary, bun, pre-start scripts — and produces a ready-to-install .service file with all nix store paths baked in.

## Secrets Setup

The bot needs two secrets:

**Bot token file** (e.g. `/root/deploy/secrets/bot-token`):
```
5568302402:AAH...your-token-here
```

**Environment file** (e.g. `/root/deploy/secrets/env`):
```
ZHIPU_API_KEY=your-key-here
```

On NixOS, use [agenix](https://github.com/ryantm/agenix) or [sops-nix](https://github.com/Mic92/sops-nix) instead of plain files.

## Proxy Considerations

If your server needs a proxy to reach external APIs, set `http_proxy` and `https_proxy` in `extraEnvironment`. **You must also set `no_proxy=127.0.0.1,localhost`** — the bot communicates with a local opencode server process over HTTP, and proxy-routing localhost traffic will break session creation.
