{
  description = "Seam dynamic Material You and Base16 theme generator";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
    in {
      packages = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
        in {
          seam = pkgs.rustPlatform.buildRustPackage {
            pname = "seam-cli";
            version = "0.1.0";
            src = pkgs.lib.cleanSourceWith {
              src = ./.;
              filter = path: type:
                let
                  name = builtins.baseNameOf path;
                  relative = pkgs.lib.removePrefix "${toString ./.}/" (toString path);
                  top = builtins.head (pkgs.lib.splitString "/" relative);
                in !builtins.elem top [ ".git" ".agents" ".codex" "target" ] && (type == "directory"
                  || name == "Cargo.toml"
                  || name == "Cargo.lock"
                  || name == "vscode-colors"
                  || pkgs.lib.hasSuffix ".rs" name
                  || pkgs.lib.hasSuffix ".json" name
                  || pkgs.lib.hasSuffix ".lua" name
                  || pkgs.lib.hasSuffix ".css" name
                  || pkgs.lib.hasSuffix ".conf" name
                  || pkgs.lib.hasSuffix ".colors" name
                  || pkgs.lib.hasSuffix ".yaml" name);
            };
            cargoLock.lockFile = ./Cargo.lock;

            meta = {
              description = "Dynamic Material You and Base16 theme generator";
              mainProgram = "seam";
              platforms = pkgs.lib.platforms.linux;
            };
          };

          default = self.packages.${system}.seam;
        });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.seam}/bin/seam";
        };
      });

      devShells = forAllSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in {
          default = pkgs.mkShell {
            packages = with pkgs; [ cargo clippy rustc rustfmt ];
          };
        });
    };
}
