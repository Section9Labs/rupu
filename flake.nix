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
        version = "0.81.0";
        # Asset name per rupu's own convention — the same string
        # `rupu update --print-platform` prints on that platform.
        asset = {
          "x86_64-linux"   = "rupu-linux-x64";
          "aarch64-linux"  = "rupu-linux-arm64";
          "aarch64-darwin" = "rupu-darwin-arm64";
        }.${system};
        sha256 = {
          "x86_64-linux"   = "b6aa01ad4249a8c84d2c3c44b93d40aa33e05ce80dc89a0488474f330a71be18";
          "aarch64-linux"  = "db5ff916e7fa4fb7d3f6934b21984b59873d951fd4f12737d2a014a847878264";
          "aarch64-darwin" = "1bd2d54234222ca1be5c4b90783ab0a67fc835a2b6e2247a0fbca1c676802f77";
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
