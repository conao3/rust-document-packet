{
  description = "Reproducible PDF inspection, overlays, rendering, and verification";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-parts.url = "github:hercules-ci/flake-parts";
    treefmt-nix.url = "github:numtide/treefmt-nix";
  };

  outputs =
    inputs@{ flake-parts, ... }:
    flake-parts.lib.mkFlake { inherit inputs; } {
      systems = [
        "x86_64-linux"
        "aarch64-darwin"
      ];

      imports = [ inputs.treefmt-nix.flakeModule ];

      perSystem =
        { pkgs, ... }:
        {
          packages.default = pkgs.rustPlatform.buildRustPackage {
            pname = "document-packet";
            version = "0.1.0";
            src = pkgs.lib.cleanSource ./.;
            cargoLock.lockFile = ./Cargo.lock;
            nativeBuildInputs = [ pkgs.makeWrapper ];
            postInstall = ''
              wrapProgram "$out/bin/document-packet" \
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.poppler-utils ]}
            '';
            meta = {
              description = "Build and verify reproducible PDF document packets";
              homepage = "https://github.com/conao3/rust-document-packet";
              license = pkgs.lib.licenses.mit;
              mainProgram = "document-packet";
              platforms = pkgs.lib.platforms.unix;
            };
          };

          devShells.default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              poppler-utils
              rustc
              rustfmt
            ];
          };

          treefmt.programs = {
            nixfmt.enable = true;
            rustfmt.enable = true;
          };
        };
    };
}
