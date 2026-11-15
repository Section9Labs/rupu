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
        version = "0.88.0";
        # Asset name per rupu's own convention — the same string
        # `rupu update --print-platform` prints on that platform.
        asset = {
          "x86_64-linux"   = "rupu-linux-x64";
          "aarch64-linux"  = "rupu-linux-arm64";
          "aarch64-darwin" = "rupu-darwin-arm64";
        }.${system};
        sha256 = {
          "x86_64-linux"   = "dda32364525cf096056279899fc1eebd72c88fb92edbcf21725dcada2dc6e63b";
          "aarch64-linux"  = "a2daa88f9be1132c2e380689ed15bb9ab5de1ff7fa89469ed54c5db9edab9900";
          "aarch64-darwin" = "8baebac0c41cd05fceae5c5d19a5e1b4c044e9fe5c4e8239bd01b90b6bc7a422";
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
