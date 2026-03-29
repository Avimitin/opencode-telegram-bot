{
  description = "Telegram bot frontend for opencode";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs, ... }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      pkgsFor = system: import nixpkgs { inherit system; };
    in
    {
      nixosModules.default = import ./nix/module.nix;
      nixosModules.opencode-telegram = import ./nix/module.nix;

      packages = forAllSystems (system:
        let pkgs = pkgsFor system;
        in rec {
          opencode-telegram-bot = pkgs.callPackage ./nix/package.nix {};
          default = opencode-telegram-bot;
        }
      );

      devShells = forAllSystems (system:
        let pkgs = pkgsFor system;
        in {
          default = pkgs.mkShell {
            nativeBuildInputs = with pkgs; [ cargo rustc pkg-config clippy rustfmt ];
            buildInputs = with pkgs; [ openssl opencode ];
            shellHook = ''
              echo "opencode-telegram-bot dev shell"
              echo "  cargo build   — build"
              echo "  cargo clippy  — lint"
              echo "  cargo run     — start bot"
            '';
          };
        }
      );
    };
}
