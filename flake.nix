# flake.nix at the repo root is GENERATED from packaging/templates/flake.nix.in
# by packaging/sync-community.sh. Edit the template; edits to the generated
# file are overwritten on every stable release.
{
  description = "rupu — agentic code-development CLI";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachSystem [
      "x86_64-linux" "aarch64-linux" "aarch64-darwin"
    ] (system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        version = "0.87.0";
        # Asset name per rupu's own convention — the same string
        # `rupu update --print-platform` prints on that platform.
        asset = {
          "x86_64-linux"   = "rupu-linux-x64";
          "aarch64-linux"  = "rupu-linux-arm64";
          "aarch64-darwin" = "rupu-darwin-arm64";
        }.${system};
        sha256 = {
          "x86_64-linux"   = "67957c7246b953a020a9cc1703321a7b0e8a37ecb83f1ca07d58682b632b597e";
          "aarch64-linux"  = "5da6a8f37ebc9617aff393cd011d6cf10496ea8724d0852d8c842d76dcf2f1a0";
          "aarch64-darwin" = "4aa2c9f98c5670406421a03a59a6d1e5e66070de7d96c3b777eded214b0bb8a8";
        }.${system};
      in {
        packages.default = pkgs.stdenv.mkDerivation {
          pname = "rupu";
          inherit version;
          src = pkgs.fetchurl {
            url = "https://github.com/Section9Labs/rupu/releases/download/v${version}/${asset}";
            inherit sha256;
          };
          dontUnpack = true;
          # The Linux binaries are static musl — no interpreter to patch.
          # Setting this stops nixpkgs' fixup phase from trying.
          dontPatchELF = true;
          dontStrip = true;
          # rupu shells out to ripgrep (hard requirement) and git.
          nativeBuildInputs = [ pkgs.makeWrapper ];
          installPhase = ''
            mkdir -p $out/bin
            install -m755 $src $out/bin/rupu
            wrapProgram $out/bin/rupu \
              --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.ripgrep pkgs.git ]}
          '';
          meta = with pkgs.lib; {
            description = "Agentic code-development CLI";
            homepage = "https://github.com/Section9Labs/rupu";
            license = licenses.asl20;
            platforms = [ "x86_64-linux" "aarch64-linux" "aarch64-darwin" ];
            mainProgram = "rupu";
          };
        };
      });
}
