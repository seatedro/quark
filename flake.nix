{
  description = "Quark development shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # Reads rust-toolchain.toml directly, so the dated nightly pin lives in
    # one place.
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems =
        f:
        nixpkgs.lib.genAttrs systems (
          system:
          f (
            import nixpkgs {
              inherit system;
              overlays = [ rust-overlay.overlays.default ];
            }
          )
        );
    in
    {
      devShells = forAllSystems (
        pkgs:
        let
          # winit, wgpu, and AccessKit dlopen these at run time, and NixOS
          # has no global library path to find them on.
          runtimeLibs = with pkgs; [
            wayland
            libxkbcommon
            vulkan-loader
            libGL
            libx11
            libxcursor
            libxi
            libxrandr
            dbus
          ];
          rustToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        in
        {
          default = pkgs.mkShell {
            packages =
              [
                rustToolchain
                # quark-terminal builds libghostty-vt with Zig from sources
                # it downloads with curl.
                pkgs.zig_0_16
                pkgs.curl
                pkgs.pkg-config
              ]
              ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isLinux (runtimeLibs ++ [ pkgs.dbus.dev ]);
            LD_LIBRARY_PATH = pkgs.lib.optionalString pkgs.stdenv.hostPlatform.isLinux (
              pkgs.lib.makeLibraryPath runtimeLibs
            );
          };
        }
        # quark-app's `webview` feature: WebKitGTK 4.1 (GTK 3) and what it
        # loads at run time. `nix develop .#webview`.
        // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          webview = pkgs.mkShell {
            packages =
              [
                rustToolchain
                pkgs.zig_0_16
                pkgs.curl
                pkgs.pkg-config
                pkgs.dbus.dev
              ]
              ++ runtimeLibs
              ++ (with pkgs; [
                gtk3
                webkitgtk_4_1
                libsoup_3
                glib-networking
                gsettings-desktop-schemas
              ]);
            LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath runtimeLibs;
            # TLS for libsoup, and the GSettings schemas GTK reads; what
            # wrapGAppsHook3 sets for an installed binary.
            GIO_EXTRA_MODULES = "${pkgs.glib-networking}/lib/gio/modules";
            shellHook = ''
              export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}''${XDG_DATA_DIRS:+:$XDG_DATA_DIRS}"
            '';
          };
        }
      );
    };
}
