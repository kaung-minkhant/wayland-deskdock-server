{
  description = "Wayland-compatible DeskDock server and development environment";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
      ];
      forEachSystem = function:
        builtins.listToAttrs (map (system: {
          name = system;
          value = function (import nixpkgs { inherit system; });
        }) systems);
    in
    {
      packages = forEachSystem (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage {
          pname = "deskdock-server";
          version = "0.1.0";
          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;
          # Integration tests launch a fake adb shell and need no Android
          # device, but sandboxed Nix builders reject that test harness.
          doCheck = false;

          meta = {
            description = "Headless DeskDock-compatible server with evdev input capture";
            homepage = "https://github.com/kaung-minkhant/wayland-deskdock-server";
            license = pkgs.lib.licenses.mit;
            mainProgram = "deskdock-server";
            platforms = pkgs.lib.platforms.linux;
          };
        };
        deskdock-server = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      });

      devShells = forEachSystem (pkgs: {
        default = pkgs.mkShell {
          packages = [
            # Rust server toolchain.
            pkgs.cargo
            pkgs.rustc
            pkgs.rustfmt
          ];

          shellHook = ''
            # Reuse the already-installed Android SDK build-tools rather than
            # adding the large Android SDK closure to this shell.
            android_sdk="''${ANDROID_HOME:-$HOME/Android/Sdk}"
            if [ -d "$android_sdk/build-tools/37.0.0" ]; then
              export PATH="$android_sdk/build-tools/37.0.0:$PATH"
            fi

            echo "DeskDock development shell: CFR, JADX, Rust, and zipalign are available."
          '';
        };
      });
    };
}
