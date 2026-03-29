{ lib, rustPlatform, openssl, pkg-config, makeWrapper, opencode }:

rustPlatform.buildRustPackage {
  pname = "opencode-telegram-bot";
  version = "0.1.0";

  src = lib.cleanSource ./..;

  cargoLock.lockFile = ../Cargo.lock;

  nativeBuildInputs = [ pkg-config makeWrapper ];
  buildInputs = [ openssl ];

  postInstall = ''
    wrapProgram $out/bin/opencode-telegram-bot \
      --prefix PATH : ${lib.makeBinPath [ opencode ]}
  '';

  meta = {
    description = "Telegram bot frontend for opencode";
    license = lib.licenses.mit;
    mainProgram = "opencode-telegram-bot";
  };
}
