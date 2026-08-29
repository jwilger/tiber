{
  description = "Tiber TypeScript and Rust development shell";

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
          git
          lefthook

          # TypeScript
          nodejs
          typescript
          typescript-language-server

          # Rust
          cargo
          rustc
          clippy
          rustfmt
          cargo-nextest
          mutagen
        ];
        shellHook = ''
          echo "Tiber devshell ready"
          echo "  TypeScript $(tsc --version), Node.js $(node --version), npm $(npm --version)"
          echo "  Rust $(rustc --version), Cargo $(cargo --version)"
          echo "  Nextest $(cargo nextest --version), Mutagen $(mutagen version)"
        '';
      };
    };
}
