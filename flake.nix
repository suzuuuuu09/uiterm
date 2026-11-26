{
  description = "uiterm development environment (macOS + NixOS/Hyprland)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "aarch64-linux"
        "x86_64-linux"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f {
            inherit system;
            pkgs = import nixpkgs { inherit system; };
          }
        );

      herdrCoreVersion = "0.1.22";
      herdrCoreSources = {
        "aarch64-darwin" = {
          url = "https://releases.herdr.dev/core/${herdrCoreVersion}/herdr-core-macos-aarch64.tar.gz";
          sha256 = "90798daadefa8d126b90ef0bcdbe9c7dda9c6cfbf011a247065ab5da348f350e";
        };
        "x86_64-darwin" = {
          url = "https://releases.herdr.dev/core/${herdrCoreVersion}/herdr-core-macos-x86_64.tar.gz";
          sha256 = "35144041969e870ddb4edcd2fd08fd26dfe5cee7d18e6b48adb276f90a48aa19";
        };
        "aarch64-linux" = {
          url = "https://releases.herdr.dev/core/${herdrCoreVersion}/herdr-core-linux-aarch64.tar.gz";
          sha256 = "98757985245d45f8d73dd85e68b139b7f167c28488bb89edd9834c3f5a5cf4cf";
        };
        "x86_64-linux" = {
          url = "https://releases.herdr.dev/core/${herdrCoreVersion}/herdr-core-linux-x86_64.tar.gz";
          sha256 = "3054ec3992ce80c5715d721caa9dcefa6056721d15dc113f4750afce763ba5df";
        };
      };

      mkHerdrCore =
        pkgs: system:
        let
          srcInfo = herdrCoreSources.${system};
        in
        pkgs.stdenvNoCC.mkDerivation {
          pname = "herdr-core";
          version = herdrCoreVersion;
          src = pkgs.fetchurl {
            inherit (srcInfo) url sha256;
          };
          dontUnpack = true;
          dontFixup = true;
          installPhase = ''
            runHook preInstall
            mkdir -p "$out/bin" work
            tar -xzf "$src" -C work
            install -m 0755 work/herdr-core "$out/bin/herdr-core"
            ln -s herdr-core "$out/bin/hc"
            runHook postInstall
          '';
          meta = {
            description = "herdr-core terminal runtime for coding agents (pinned binary)";
            homepage = "https://github.com/herdrdev/herdr-core";
            license = pkgs.lib.licenses.asl20;
            platforms = systems;
          };
        };
    in
    {
      packages = forAllSystems (
        { pkgs, system }:
        {
          herdr-core = mkHerdrCore pkgs system;
        }
      );

      devShells = forAllSystems (
        { pkgs, system }:
        let
          herdrCore = mkHerdrCore pkgs system;
          isLinux = pkgs.stdenv.hostPlatform.isLinux;
          linuxDeps = pkgs.lib.optionals isLinux (
            with pkgs;
            [
              webkitgtk_4_1
              gtk3
              libsoup_3
              glib
              cairo
              pango
              gdk-pixbuf
              atk
              librsvg
              openssl
              dbus
              wayland
              libxkbcommon
              libayatana-appindicator
            ]
          );
        in
        {
          default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                rustc
                cargo
                clippy
                rustfmt
                pkg-config
                nodejs_24
                pnpm
                herdrCore
              ]
              ++ linuxDeps;

            shellHook =
              pkgs.lib.optionalString isLinux ''
                export PKG_CONFIG_PATH="${
                  pkgs.lib.makeSearchPath "lib/pkgconfig" linuxDeps
                }''${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
                export LD_LIBRARY_PATH="${
                  pkgs.lib.makeLibraryPath linuxDeps
                }''${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
                export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
                # Wayland / Hyprland: prefer native Wayland backend for GTK/WebKitGTK
                export GDK_BACKEND="''${GDK_BACKEND:-wayland,x11}"
              ''
              + ''
                export HERDR_CORE_BIN="${herdrCore}/bin/herdr-core"
              '';
          };
        }
      );
    };
}
