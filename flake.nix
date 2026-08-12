{
  description = "Tiber — standalone Rust development harness";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { nixpkgs, ... }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in
    {
      devShells.${system}.default = pkgs.mkShell {
        name = "tiber";

        packages = with pkgs; [
          actionlint
          bash
          cargo
          clippy
          git
          jq
          just
          nodejs_22
          prettier
          rustc
          rustfmt
        ];

        shellHook = ''
          export TIBER_DEPENDENCIES_DIR="$PWD/.dependencies"
          export CARGO_HOME="$TIBER_DEPENDENCIES_DIR/cargo"
          export CARGO_INSTALL_ROOT="$TIBER_DEPENDENCIES_DIR/cargo-install"
          mkdir -p "$CARGO_HOME" "$CARGO_INSTALL_ROOT"
          export PATH="$CARGO_INSTALL_ROOT/bin:$PATH"

          echo "Tiber devshell ready: $(rustc --version)"
        '';
      };
    };
}
