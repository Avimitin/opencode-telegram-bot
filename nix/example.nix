# Example NixOS configuration
{ config, pkgs, ... }:

{
  services.opencode-telegram = {
    enable = true;

    botTokenFile = "/run/secrets/telegram-bot-token";
    environmentFile = "/run/secrets/opencode-env";

    settings = {
      model = "zai-coding-plan/glm-4.7";
      permission = { "*" = "allow"; };
    };

    accessConfig = {
      dmPolicy = "pairing";
      allowFrom = [ "123456789" ];
      groups = {
        "-1009876543210" = {
          requireMention = true;
          allowFrom = [];
        };
      };
      pending = {};
      mentionPatterns = [ "@YourBotName" ];
    };
  };
}
