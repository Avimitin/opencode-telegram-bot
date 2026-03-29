{ config, lib, pkgs, ... }:

let
  cfg = config.services.opencode-telegram;

  # Generate opencode.json from Nix attrs
  opencodeConfigFile = pkgs.writeText "opencode.json" (builtins.toJSON cfg.settings);
in
{
  options.services.opencode-telegram = {
    enable = lib.mkEnableOption "OpenCode Telegram bot";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix {};
      description = "The opencode-telegram-bot package.";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "opencode-telegram";
      description = "User to run the service as.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "opencode-telegram";
      description = "Group to run the service as.";
    };

    stateDir = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/opencode-telegram";
      description = "State directory for opencode and Telegram channel data.";
    };

    botTokenFile = lib.mkOption {
      type = lib.types.path;
      description = "Path to a file containing the Telegram bot token.";
    };

    settings = lib.mkOption {
      type = lib.types.attrs;
      default = {};
      description = ''
        OpenCode configuration as Nix attrs, serialized to opencode.json.
        See https://opencode.ai/docs/configuration for all options.
      '';
    };

    accessConfig = lib.mkOption {
      type = lib.types.attrs;
      default = {
        dmPolicy = "pairing";
        allowFrom = [];
        groups = {};
        pending = {};
        mentionPatterns = [];
      };
      description = "Telegram channel access.json configuration.";
    };

    environmentFile = lib.mkOption {
      type = lib.types.path;
      description = "Path to environment file containing API keys (e.g. ZHIPU_API_KEY). Loaded via systemd EnvironmentFile=.";
    };

    sandbox = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Enable systemd sandboxing and hardening.
        Restricts the bot to its stateDir and blocks access to the rest
        of the filesystem. Disable if you want opencode to manage files
        outside the state directory (e.g. full system access).
      '';
    };
  };

  config = lib.mkIf cfg.enable {
    users.users.${cfg.user} = {
      isSystemUser = true;
      home = cfg.stateDir;
      createHome = true;
      group = cfg.group;
    };
    users.groups.${cfg.group} = {};

    systemd.tmpfiles.rules = builtins.map
      (dir: "d ${dir} 0700 ${cfg.user} ${cfg.group} -")
      [
        cfg.stateDir
        "${cfg.stateDir}/.opencode"
        "${cfg.stateDir}/.opencode/channels"
        "${cfg.stateDir}/.opencode/channels/telegram"
        "${cfg.stateDir}/.opencode/channels/telegram/approved"
      ];

    systemd.services.opencode-telegram = {
      description = "OpenCode Telegram Bot";
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = lib.mkMerge [
        {
          Type = "simple";
          User = cfg.user;
          Group = cfg.group;
          WorkingDirectory = cfg.stateDir;
          ExecStart = "${cfg.package}/bin/opencode-telegram-bot";
          Restart = "on-failure";
          RestartSec = 10;
          EnvironmentFile = cfg.environmentFile;
        }

        # Sandboxing and hardening
        (lib.mkIf cfg.sandbox {
          ProtectSystem = lib.mkDefault "strict";
          ProtectHome = lib.mkDefault true;
          PrivateTmp = lib.mkDefault true;
          PrivateDevices = lib.mkDefault true;
          PrivateMounts = lib.mkDefault true;
          ProtectClock = lib.mkDefault true;
          ProtectControlGroups = lib.mkDefault true;
          ProtectHostname = lib.mkDefault true;
          ProtectKernelLogs = lib.mkDefault true;
          ProtectKernelModules = lib.mkDefault true;
          ProtectKernelTunables = lib.mkDefault true;
          ProtectProc = lib.mkDefault "invisible";
          NoNewPrivileges = lib.mkDefault true;
          RestrictNamespaces = lib.mkDefault true;
          RestrictRealtime = lib.mkDefault true;
          RestrictSUIDSGID = lib.mkDefault true;
          RemoveIPC = lib.mkDefault true;
          LockPersonality = lib.mkDefault true;
          UMask = lib.mkDefault "0077";
          CapabilityBoundingSet = lib.mkDefault [ "" ];
          DeviceAllow = lib.mkDefault [ "" ];
          RestrictAddressFamilies = lib.mkDefault [
            "AF_INET"
            "AF_INET6"
            "AF_UNIX"
          ];
          # Note: no SystemCallFilter — opencode is a Go binary that needs
          # sched_setscheduler (@privileged) and other syscalls from its runtime.
          ReadWritePaths = [ cfg.stateDir ];
        })
      ];

      preStart = ''
        cp ${opencodeConfigFile} ${cfg.stateDir}/opencode.json

        cat > ${cfg.stateDir}/.opencode/channels/telegram/access.json <<'ACCESSEOF'
        ${builtins.toJSON cfg.accessConfig}
        ACCESSEOF

        echo "TELEGRAM_BOT_TOKEN=$(cat ${cfg.botTokenFile})" > ${cfg.stateDir}/.opencode/channels/telegram/.env
        chmod 600 ${cfg.stateDir}/.opencode/channels/telegram/.env
      '';

      environment = {
        HOME = cfg.stateDir;
        TELEGRAM_STATE_DIR = "${cfg.stateDir}/.opencode/channels/telegram";
        # The bot talks to a local opencode server over HTTP.
        # Proxy variables must not intercept localhost traffic.
        no_proxy = "127.0.0.1,localhost";
      };
    };
  };
}
