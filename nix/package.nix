{ lib, stdenvNoCC, bun, makeWrapper, opencode, cacert }:

let
  src = lib.cleanSource ./..;

  node_modules = stdenvNoCC.mkDerivation {
    pname = "opencode-telegram-bot-node_modules";
    version = "0.1.0";
    inherit src;

    impureEnvVars = lib.fetchers.proxyImpureEnvVars;
    nativeBuildInputs = [ bun ];

    buildPhase = ''
      runHook preBuild
      export HOME=$TMPDIR
      export SSL_CERT_FILE=${cacert}/etc/ssl/certs/ca-bundle.crt
      bun install --frozen-lockfile --no-progress
      runHook postBuild
    '';

    installPhase = ''
      mkdir -p $out
      cp -r node_modules $out/
    '';

    outputHashMode = "recursive";
    outputHashAlgo = "sha256";
    outputHash = "sha256-TThHNsW0JiF5ubSbKeXJc2/XNUSIwACu7H9SxDMM1sQ=";
  };
in
stdenvNoCC.mkDerivation {
  pname = "opencode-telegram-bot";
  version = "0.1.0";
  inherit src;

  nativeBuildInputs = [ makeWrapper ];

  installPhase = ''
    runHook preInstall

    mkdir -p $out/lib/opencode-telegram-bot
    cp -r src package.json bun.lock $out/lib/opencode-telegram-bot/
    ln -s ${node_modules}/node_modules $out/lib/opencode-telegram-bot/node_modules

    mkdir -p $out/bin
    makeWrapper ${bun}/bin/bun $out/bin/opencode-telegram-bot \
      --prefix PATH : ${lib.makeBinPath [ opencode ]} \
      --add-flags "run $out/lib/opencode-telegram-bot/src/index.ts"

    runHook postInstall
  '';

  meta = {
    description = "Telegram bot frontend for opencode";
    license = lib.licenses.mit;
    mainProgram = "opencode-telegram-bot";
  };
}
