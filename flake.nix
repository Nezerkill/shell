{
  description = "Native Wayland desktop shell for Hyprland and Niri (Rust)";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          name = "shell-dev";
          packages = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            rust-analyzer
            pkg-config
            wayland
            wayland-protocols
            wayland-scanner
            libxkbcommon
            vulkan-loader
            vulkan-tools
            linux-pam
            pipewire
            libpulseaudio
            sqlite
            nixfmt
          ];
          RUST_BACKTRACE = "1";
        };
      });
    };
}
