{
  description = "Tiber — standalone Rust development harness";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      lib = pkgs.lib;

      source = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./crates
        ];
      };

      tiber = pkgs.rustPlatform.buildRustPackage {
        pname = "tiber";
        version = "0.1.0";
        src = source;

        cargoLock = {
          lockFile = ./Cargo.lock;
          outputHashes."codex-agent-extension-0.148.0" = "sha256-g288ZJaf7hNUqJQ7hpVHWcVBZIbkrIRUDFz+k9QYQoE=";
          outputHashes."crossterm-0.29.0" = "sha256-cQxQQuV+YEutuQiPurXVISq6F/99vCEk8qe5PU8BCSo=";
          outputHashes."nucleo-0.5.0" = "sha256-Hm4SxtTSBrcWpXrtSqeO0TACbUxq3gizg1zD/6Yw/sI=";
          outputHashes."tokio-tungstenite-0.28.0" = "sha256-V1xmnrfRWOcZZogelZEA4vvyMj2awCfHVA5/glQ6KAI=";
          outputHashes."tungstenite-0.27.0" = "sha256-VVHhk7l9J/sEmG3q/UuV/sQ3f+fGsmq5vumSy8vbMvw=";
        };
        cargoBuildFlags = [
          "--workspace"
          "--bins"
        ];
        nativeBuildInputs = [
          pkgs.makeWrapper
          pkgs.pkg-config
        ];
        buildInputs = [ pkgs.openssl ];

        # Canonical integration tests exercise real Bubblewrap in `just ci`.
        # Nested Nix sandboxes cannot provide that execution environment.
        doCheck = false;

        # Installing only the shipping binaries avoids buildRustPackage's default
        # post-build copy of the entire (very large) embedded-Codex target tree.
        installPhase = ''
          runHook preInstall

          release_dir="target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/$cargoBuildType"
          install -d "$out/libexec/tiber"
          install -Dm755 "$release_dir/tiber" "$out/libexec/tiber/tiber"
          install -Dm755 "$release_dir/tiber-process-launcher" "$out/libexec/tiber/tiber-process-launcher"
          install -Dm755 "$release_dir/tiber-repository-worker" "$out/libexec/tiber/tiber-repository-worker"
          ln -s "${lib.getExe pkgs.bubblewrap}" "$out/libexec/tiber/bwrap"

          makeWrapper "$out/libexec/tiber/tiber" "$out/bin/tiber" \
            --prefix PATH : "${
              lib.makeBinPath [
                pkgs.git
                pkgs.coreutils
              ]
            }"

          runHook postInstall
        '';

        meta = {
          description = "Tiber local development harness";
          mainProgram = "tiber";
          platforms = [ "x86_64-linux" ];
        };
      };

      packageSmoke = pkgs.runCommand "tiber-package-smoke" { } ''
        empty_home="$TMPDIR/empty-home"
        mkdir -p "$empty_home"

        ambient_bin="$TMPDIR/ambient-bin"
        mkdir -p "$ambient_bin"
        printf '%s\n' '#!${pkgs.runtimeShell}' 'exit 99' > "$ambient_bin/codex"
        chmod +x "$ambient_bin/codex"
        env -i HOME="$empty_home" PATH="$ambient_bin" \
          "${tiber}/bin/tiber" --help \
          | "${lib.getExe pkgs.gnugrep}" --fixed-strings "tiber [session active" >/dev/null

        env -i HOME="$empty_home" PATH="" "${tiber}/bin/tiber" --help >/dev/null

        test -x "${tiber}/libexec/tiber/tiber"
        test -x "${tiber}/libexec/tiber/tiber-process-launcher"
        test -x "${tiber}/libexec/tiber/tiber-repository-worker"
        test ! -e "${tiber}/bin/tiber-process-launcher"
        test ! -e "${tiber}/bin/tiber-repository-worker"
        test -L "${tiber}/libexec/tiber/bwrap"
        test -x "$(readlink -f "${tiber}/libexec/tiber/bwrap")"

        if env -i HOME="$empty_home" PATH="" \
          "${tiber}/libexec/tiber/tiber-repository-worker" </dev/null; then
          echo "tiber-repository-worker accepted empty stdin" >&2
          exit 1
        fi

        handshake="$TMPDIR/launcher-handshake"
        env -i HOME="$empty_home" PATH="" TIBER_LAUNCH_HANDSHAKE="$handshake" \
          "${tiber}/libexec/tiber/tiber-process-launcher" \
          -- "${pkgs.coreutils}/bin/true"
        test "$(cat "$handshake")" = "launched"

        touch "$out"
      '';
    in
    {
      packages.${system} = {
        inherit tiber;
        default = tiber;
      };

      apps.${system} = {
        tiber = {
          type = "app";
          program = "${tiber}/bin/tiber";
        };
        default = {
          type = "app";
          program = "${tiber}/bin/tiber";
        };
      };

      checks.${system}.package-smoke = packageSmoke;

      devShells.${system}.default = pkgs.mkShell {
        name = "tiber";

        packages = with pkgs; [
          actionlint
          bash
          bubblewrap
          cargo
          clippy
          git
          jq
          just
          lefthook
          nodejs_22
          openssl
          pkg-config
          prettier
          util-linux
          rust-analyzer
          rustc
          rustfmt
        ];

        shellHook = ''
          export TIBER_DEPENDENCIES_DIR="$PWD/.dependencies"
          export CARGO_HOME="$TIBER_DEPENDENCIES_DIR/cargo"
          export CARGO_INSTALL_ROOT="$TIBER_DEPENDENCIES_DIR/cargo-install"
          export TIBER_TEST_BASH="${pkgs.bash}/bin/bash"
          export TIBER_TEST_BWRAP="${pkgs.bubblewrap}/bin/bwrap"
          export PKG_CONFIG_PATH="${pkgs.openssl.dev}/lib/pkgconfig"
          mkdir -p "$CARGO_HOME" "$CARGO_INSTALL_ROOT"
          export PATH="$CARGO_INSTALL_ROOT/bin:$PATH"

          echo "Tiber devshell ready: $(rustc --version)"
        '';
      };
    };
}
