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
        in
        {
          default = pkgs.mkShell {
            packages =
              [
                (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml)
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
      );
    };
}
