{
  description = "MCP server for semantic search, a link graph and editing over Obsidian vaults";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "aarch64-linux" "x86_64-linux" "aarch64-darwin" ];
      forAll = f: nixpkgs.lib.genAttrs systems (s: f nixpkgs.legacyPackages.${s});
      # ort links the nixpkgs ONNX Runtime instead of downloading its own.
      ortEnv = pkgs: {
        ORT_LIB_LOCATION = "${pkgs.onnxruntime}/lib";
        ORT_PREFER_DYNAMIC_LINK = "1";
      };
    in
    {
      packages = forAll (pkgs: rec {
        obsidian-brain = pkgs.rustPlatform.buildRustPackage {
          pname = "obsidian-brain";
          version = "2.0.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          nativeBuildInputs = [ pkgs.pkg-config pkgs.makeWrapper ];
          buildInputs = [ pkgs.onnxruntime ];
          env = ortEnv pkgs;
          postInstall = ''
            wrapProgram $out/bin/obsidian-brain \
              --set-default ORT_DYLIB_PATH ${pkgs.onnxruntime}/lib/libonnxruntime${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}
          '';
          meta.mainProgram = "obsidian-brain";
        };
        default = obsidian-brain;
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell ({
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            pkg-config
            onnxruntime
            sqlite
            nodejs
          ];
          ORT_DYLIB_PATH = "${pkgs.onnxruntime}/lib/libonnxruntime${pkgs.stdenv.hostPlatform.extensions.sharedLibrary}";
        } // ortEnv pkgs);
      });
    };
}
