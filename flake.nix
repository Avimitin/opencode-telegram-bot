{
  description = "Telegram bot frontend for opencode";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs, ... }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in
    {
      nixosModules.default = import ./nix/module.nix;
      nixosModules.opencode-telegram = import ./nix/module.nix;

      packages = forAllSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = pkgs.callPackage ./nix/package.nix {};
          opencode-telegram-bot = pkgs.callPackage ./nix/package.nix {};
        }
      );

      devShells = forAllSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [ bun opencode ];
            shellHook = ''
              echo "opencode-telegram-bot dev shell"
              echo "  bun install   — install deps"
              echo "  bun run dev   — start bot"
            '';
          };
        }
      );
    };
}
