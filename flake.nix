{
  description = "Tiber — standalone Rust development harness";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      lib = pkgs.lib;

      reviewedCodex = pkgs.stdenvNoCC.mkDerivation {
        pname = "codex";
        version = "0.147.0";

        src = pkgs.fetchurl {
          url = "https://registry.npmjs.org/@openai/codex/-/codex-0.147.0-linux-x64.tgz";
          hash = "sha256-yWl0DPgpfkwxkFzVUe/rLJmvUIDBLCNr34JVmLJQE5o=";
        };

        sourceRoot = "package/vendor/x86_64-unknown-linux-musl";

        installPhase = ''
          runHook preInstall
          mkdir -p "$out/bin" "$out/libexec/codex"
          cp -R . "$out/libexec/codex"
          ln -s ../libexec/codex/bin/codex "$out/bin/codex"
          runHook postInstall
        '';

        meta = {
          description = "Reviewed Codex CLI used by Tiber";
          mainProgram = "codex";
          platforms = [ "x86_64-linux" ];
        };
      };

      source = lib.fileset.toSource {
        root = ./.;
        fileset = lib.fileset.unions [
          ./Cargo.toml
          ./Cargo.lock
          ./crates
          ./config
        ];
      };

      tiber = pkgs.rustPlatform.buildRustPackage {
        pname = "tiber";
        version = "0.1.0";
        src = source;

        cargoLock.lockFile = ./Cargo.lock;
        cargoBuildFlags = [
          "--workspace"
          "--bins"
        ];
        nativeBuildInputs = [ pkgs.makeWrapper ];

        # Canonical integration tests exercise real Bubblewrap in `just ci`.
        # Nested Nix sandboxes cannot provide that execution environment.
        doCheck = false;

        postInstall = ''
          install -d "$out/libexec/tiber"
          mv "$out/bin/tiber" "$out/libexec/tiber/tiber"
          mv "$out/bin/tiber-process-launcher" "$out/libexec/tiber/tiber-process-launcher"
          mv "$out/bin/tiber-repository-worker" "$out/libexec/tiber/tiber-repository-worker"
          ln -s "${lib.getExe pkgs.bubblewrap}" "$out/libexec/tiber/bwrap"

          makeWrapper "$out/libexec/tiber/tiber" "$out/bin/tiber" \
            --prefix PATH : "${
              lib.makeBinPath [
                reviewedCodex
                pkgs.git
                pkgs.coreutils
              ]
            }"
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

        codex_version="$(env -i HOME="$empty_home" PATH="" \
          "${lib.getExe reviewedCodex}" --version)"
        test "$codex_version" = "codex-cli 0.147.0"
        env -i HOME="$empty_home" PATH="" \
          "${lib.getExe reviewedCodex}" --help \
          | "${lib.getExe pkgs.gnugrep}" --fixed-strings -- "--remote" >/dev/null

        ambient_bin="$TMPDIR/ambient-bin"
        mkdir -p "$ambient_bin"
        printf '%s\n' '#!${pkgs.runtimeShell}' 'exit 99' > "$ambient_bin/codex"
        chmod +x "$ambient_bin/codex"
        env -i HOME="$empty_home" PATH="$ambient_bin" \
          "${tiber}/bin/tiber" auth status \
          | "${lib.getExe pkgs.gnugrep}" --fixed-strings "signed out" >/dev/null

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
          prettier
          util-linux
          rust-analyzer
          rustc
          rustfmt
        ] ++ [ reviewedCodex ];

        shellHook = ''
          export TIBER_DEPENDENCIES_DIR="$PWD/.dependencies"
          export CARGO_HOME="$TIBER_DEPENDENCIES_DIR/cargo"
          export CARGO_INSTALL_ROOT="$TIBER_DEPENDENCIES_DIR/cargo-install"
          export TIBER_TEST_BASH="${pkgs.bash}/bin/bash"
          export TIBER_TEST_BWRAP="${pkgs.bubblewrap}/bin/bwrap"
          mkdir -p "$CARGO_HOME" "$CARGO_INSTALL_ROOT"
          export PATH="$CARGO_INSTALL_ROOT/bin:$PATH"

          echo "Tiber devshell ready: $(rustc --version)"
        '';
      };
    };
}
