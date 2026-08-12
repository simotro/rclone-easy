{
  description = "Rclone Easy — interfaccia grafica per rclone (Tauri + SvelteKit)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };
        lib = pkgs.lib;

        # Letta da Cargo.toml invece di ripetuta qui a mano: un posto in
        # meno da ricordarsi di aggiornare ad ogni release (insieme a
        # package.json/Cargo.toml/tauri.conf.json, vedi il flusso di
        # release del progetto).
        version = (builtins.fromTOML (builtins.readFile ./src-tauri/Cargo.toml)).package.version;

        # Il frontend SvelteKit (adapter-static) è una build Node separata:
        # `npm run build` produce `build/`, che `src-tauri/tauri.conf.json`
        # si aspetta in `../build` rispetto a `src-tauri` (vedi sotto). Non
        # serve `cargo tauri build`/il suo bundler: sotto Nix il "bundle" è
        # la derivation stessa, la build grezza del binario basta.
        frontend = pkgs.buildNpmPackage {
          pname = "rclone-easy-frontend";
          inherit version;
          src = ./.;

          npmDepsHash = "sha256-l3CbIj5EitRIVUrv0WuV3weOLlstPuuONHw5j252KFI=";

          # SvelteKit gira `svelte-kit sync` come parte della build: serve
          # scrivere in .svelte-kit/, quindi niente `dontNpmBuild`/sandbox
          # più stretta del default di buildNpmPackage.
          installPhase = ''
            runHook preInstall
            mkdir -p $out
            cp -r build/. $out/
            runHook postInstall
          '';
        };
      in
      {
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "rclone-easy";
          inherit version;

          # `src` è direttamente `src-tauri` (non l'intero repo): il
          # controllo di coerenza di `cargoSetupPostPatchHook` di nixpkgs
          # cerca `Cargo.lock` alla radice di `$src` a prescindere da
          # `buildAndTestSubdir`, quindi con l'intero repo come `src` non lo
          # troverebbe. `../build` (vedi sotto) finisce quindi un livello
          # sopra la radice della sorgente scompattata, non dentro al repo.
          src = ./src-tauri;
          cargoLock.lockFile = ./src-tauri/Cargo.lock;

          # La test suite (vedi il repo) presuppone un vero binario rclone
          # eseguibile e in alcuni casi rete/filesystem reale (avvia
          # `rclone rcd`, `rclone bisync` per davvero) — non è pensata per
          # girare nella sandbox di build di Nix (niente rete, il
          # placeholder sotto non è un eseguibile vero) e non serve
          # comunque qui: viene già eseguita normalmente durante lo
          # sviluppo (`cargo test`), questa è solo la build del pacchetto.
          doCheck = false;

          # Mette il risultato della build del frontend esattamente dove
          # `tauri.conf.json` (`frontendDist: "../build"`) e quindi
          # `tauri-build` (compilato dentro `src-tauri/build.rs`) se lo
          # aspettano: Tauri v2 incorpora gli asset nel binario in fase di
          # compilazione, quindi devono esistere PRIMA di `cargo build`.
          postPatch = ''
            mkdir -p ../build
            cp -r ${frontend}/. ../build/

            # `tauri-build` (compilato dentro `build.rs`) verifica che il
            # sidecar dichiarato in `externalBin` esista SEMPRE a compile
            # time, anche per una `cargo build` grezza come questa che non
            # passa mai dal bundler — genera solo i file di permessi ACL
            # per il sidecar, non lo incorpora nel binario. Un placeholder
            # vuoto basta: mai copiato in `$out`, mai eseguito (il nostro
            # `resolve_rclone_binary()` cerca il sidecar accanto
            # all'eseguibile A RUNTIME, non qui, e qui ripiega comunque sul
            # `rclone` di sistema aggiunto al PATH — vedi `preFixup`).
            mkdir -p binaries
            touch binaries/rclone-${pkgs.stdenv.hostPlatform.rust.rustcTarget}
            chmod +x binaries/rclone-${pkgs.stdenv.hostPlatform.rust.rustcTarget}
          '';

          nativeBuildInputs = with pkgs; [
            pkg-config
            wrapGAppsHook3
          ];

          buildInputs = with pkgs; [
            glib
            gtk3
            webkitgtk_4_1
            libsoup_3
            openssl
            librsvg
            libayatana-appindicator
          ];

          # Niente sidecar rclone bundlato (`src-tauri/binaries/`, scaricato
          # da `scripts/fetch-rclone-sidecar.sh` solo per i bundle
          # AppImage/deb/rpm "tradizionali"): `resolve_rclone_binary()`
          # (`src-tauri/src/rclone_bin.rs`) ripiega già da sola su `rclone`
          # nel PATH quando non trova un sidecar accanto all'eseguibile —
          # qui basta garantirlo a runtime col wrapper, senza vendorizzare
          # un binario esterno prebuilt (contrario allo spirito di Nix).
          preFixup = ''
            gappsWrapperArgs+=(--prefix PATH : ${lib.makeBinPath [ pkgs.rclone ]})
          '';

          postInstall =
            let
              desktopItem = pkgs.makeDesktopItem {
                name = "rclone-easy";
                exec = "rclone-easy";
                icon = "rclone-easy";
                desktopName = "rclone-easy";
                comment = "Interfaccia grafica per rclone";
                categories = [ "Utility" ];
              };
            in
            ''
              install -Dm644 ${./src-tauri/icons/icon.svg} $out/share/icons/hicolor/scalable/apps/rclone-easy.svg
              install -Dm644 ${desktopItem}/share/applications/*.desktop $out/share/applications/rclone-easy.desktop

              # `tauri-build` copia da sé il placeholder di `postPatch`
              # dentro `target/release/rclone` (rinominato, senza il
              # suffisso della tripletta — così Tauri lo troverebbe come
              # sidecar in un bundle vero), e `cargoInstallHook` lo installa
              # di conseguenza come un secondo binario in $out/bin/rclone.
              # Va tolto SEMPRE: è vuoto/inutilizzabile, e se restasse
              # rischierebbe di finire prima del vero rclone (aggiunto al
              # PATH da `preFixup`) nel PATH di sistema di chi installa
              # questo pacchetto, rompendo il comando "rclone" ovunque.
              rm -f $out/bin/rclone
            '';

          meta = with lib; {
            description = "Interfaccia grafica per rclone (mount, backup, sincronizzazione bidirezionale)";
            homepage = "https://github.com/simotro/rclone-easy";
            license = licenses.mit;
            platforms = platforms.linux;
            mainProgram = "rclone-easy";
          };
        };

        devShells.default = pkgs.mkShell {
          inputsFrom = [ self.packages.${system}.default ];
          packages = with pkgs; [ nodejs cargo-tauri ];
        };
      });
}
