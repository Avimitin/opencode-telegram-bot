{ lib, rustPlatform, openssl, pkg-config, makeWrapper, opencode }:

let
  # Must match the major version of the opencode API this bot was written against.
  # See src/opencode.rs header for the full list of endpoints used.
  expectedMajor = "1";
  actualMajor = lib.versions.major opencode.version;
in
assert lib.assertMsg (actualMajor == expectedMajor)
  "opencode major version mismatch: expected ${expectedMajor}.x, got ${opencode.version}";

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
