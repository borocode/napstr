{
  description = "Napstr Tauri development shell";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
    in {
      devShells.${system}.default = pkgs.mkShell {
        packages = with pkgs; [
          nodejs_22
          rustc
          cargo
          rustfmt
          appimage-run
          alsa-lib
          curl
          file
          tor
          zenity
          pkg-config
          dbus
          gtk3
          webkitgtk_4_1
          wayland
          librsvg
          patchelf
        ];
        nativeBuildInputs = with pkgs; [
          alsa-lib.dev
          dbus.dev
          gtk3.dev
          webkitgtk_4_1.dev
          wayland.dev
        ];
      };
    };
}
